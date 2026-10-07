//! terminal model: runs shell and keeps screen state

mod bounds;
mod builder;
mod content;
mod events;
mod keys;
mod mouse;
mod process;
mod selection;

use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use alacritty_terminal::{
    Term,
    event::Notify,
    event_loop::{Msg, Notifier},
    grid::{Dimensions, Scroll},
    sync::FairMutex,
    term::TermMode,
};
use gpui::{App, EventEmitter, Keystroke, Task, WindowAppearance};

pub use bounds::TerminalBounds;
pub use builder::TerminalBuilder;
pub use content::{Content, IndexedCell};
pub use mouse::{MouseAction, MouseButton};
#[cfg(test)]
pub(crate) use process::tests::{Kill, spawn};
pub use process::{ForegroundProcess, children, foreground_process, process_info};

use builder::ZedListener;
use keys::to_esc_str;

use crate::{settings::ThemeSettings, theme::Theme};

/// events emitted to terminal view
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    TitleChanged,
    Wakeup,
    CloseTerminal,
}

enum InternalEvent {
    Resize(TerminalBounds),
    Scroll(Scroll),
}

pub struct Terminal {
    pty_tx: Notifier,
    term: Arc<FairMutex<Term<ZedListener>>>,
    events: Vec<InternalEvent>,
    pub last_content: Content,
    /// set by alacritty on new pty output and by local selection changes
    dirty: Arc<AtomicBool>,
    title: String,
    /// pid of the shell running in the pty
    pub shell_pid: u32,
    /// color scheme from the profile, global theme when none
    theme_settings: Option<ThemeSettings>,
    theme: Option<Theme>,
    _event_loop_task: Task<()>,
}

impl EventEmitter<Event> for Terminal {}
impl Terminal {
    /// title set by the running program, falls back to `default_title`
    pub fn title(&self, default_title: &str) -> String {
        if self.title.is_empty() {
            default_title.to_string()
        } else {
            self.title.clone()
        }
    }

    /// profile theme when it has one, otherwise the global theme
    pub fn theme<'a>(&'a self, cx: &'a App) -> &'a Theme {
        self.theme.as_ref().unwrap_or_else(|| Theme::get(cx))
    }

    /// load the profile theme for this system appearance
    pub fn apply_theme(&mut self, appearance: WindowAppearance) {
        self.theme = self
            .theme_settings
            .as_ref()
            .map(|settings| Theme::for_appearance(settings, appearance));
    }

    fn write_to_pty(&self, input: impl Into<Cow<'static, [u8]>>) {
        self.pty_tx.notify(input.into());
    }

    /// send user input, dropping any selection and jumping back to bottom of scrollback
    pub fn input(&mut self, input: impl Into<Cow<'static, [u8]>>) {
        self.clear_selection();
        self.events.push(InternalEvent::Scroll(Scroll::Bottom));
        self.write_to_pty(input);
    }

    /// queue a resize, applied on next `sync`
    pub fn set_size(&mut self, new_bounds: TerminalBounds) {
        let old_bounds = self.last_content.terminal_bounds;
        self.last_content.terminal_bounds = new_bounds;
        // skip pixel-only changes so dragging window does not spam SIGWINCH
        if old_bounds.num_lines() == new_bounds.num_lines()
            && old_bounds.num_columns() == new_bounds.num_columns()
        {
            return;
        }
        self.events.push(InternalEvent::Resize(new_bounds));
    }

    /// scroll the viewport by lines, positive is up into history
    pub fn scroll(&mut self, lines: i32) {
        self.events
            .push(InternalEvent::Scroll(Scroll::Delta(lines)));
    }

    /// lines scrolled out above the screen right now, without waiting for `sync`
    pub fn history_size(&self) -> usize {
        self.term.lock_unfair().grid().history_size()
    }

    /// show history `offset` lines above the bottom
    pub fn scroll_to(&mut self, offset: usize) {
        // alacritty only scrolls relative, so start from a known position
        self.events.push(InternalEvent::Scroll(Scroll::Bottom));
        let lines = offset.min(i32::MAX as usize) as i32;
        self.events
            .push(InternalEvent::Scroll(Scroll::Delta(lines)));
    }

    /// map a keystroke to an escape sequence and write it, returns false if unmapped
    pub fn try_keystroke(&mut self, keystroke: &Keystroke) -> bool {
        match to_esc_str(keystroke, self.last_content.mode, false) {
            Some(Cow::Borrowed(esc)) => self.input(esc.as_bytes()),
            Some(Cow::Owned(esc)) => self.input(esc.into_bytes()),
            None => return false,
        }
        true
    }

    /// paste text, wrapping it when program asked for bracketed paste
    pub fn paste(&mut self, text: &str) {
        let text = if self.last_content.mode.contains(TermMode::BRACKETED_PASTE) {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.input(text.into_bytes());
    }

    /// report focus changes to programs that ask for that
    pub fn focus_changed(&self, focused: bool) {
        if self.last_content.mode.contains(TermMode::FOCUS_IN_OUT) {
            self.write_to_pty(if focused { "\x1b[I" } else { "\x1b[O" }.as_bytes());
        }
    }

    /// apply queued events and refresh the grid snapshot if anything changed
    pub fn sync(&mut self) {
        // many frames are repaints for focus or tab changes, skip the copy for those
        if !self.dirty.swap(false, Ordering::Acquire) && self.events.is_empty() {
            return;
        }
        let mut term = self.term.lock_unfair();
        for event in self.events.drain(..) {
            match event {
                InternalEvent::Resize(bounds) => {
                    self.pty_tx.0.send(Msg::Resize(bounds.into())).ok();
                    term.resize(bounds);
                }
                InternalEvent::Scroll(scroll) => term.scroll_display(scroll),
            }
        }
        self.last_content.refresh(&term);
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.pty_tx.0.send(Msg::Shutdown).ok();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use alacritty_terminal::event::Event as AlacTermEvent;
    use futures::{FutureExt, StreamExt};
    use gpui::{AppContext, Bounds, point, px, size};

    use super::{
        Terminal, TerminalBounds, TerminalBuilder, foreground_process, process::ForegroundProcess,
    };
    use crate::settings::{CursorShape, Profile, Settings, Shell, TerminalSettings};

    pub(super) fn spawn(settings: &TerminalSettings) -> TerminalBuilder {
        spawn_with(settings, Settings::default().default_profile())
    }

    /// default profile running `command`
    pub(super) fn profile(command: Shell) -> Profile {
        Profile {
            command,
            ..Settings::default().default_profile().clone()
        }
    }

    pub(super) fn spawn_with(settings: &TerminalSettings, profile: &Profile) -> TerminalBuilder {
        let mut builder =
            TerminalBuilder::new(settings, profile, 0).expect("failed to spawn shell");
        // 80x24 grid, a real window would size it in prepaint
        builder.terminal.set_size(TerminalBounds::new(
            px(20.),
            px(10.),
            Bounds::new(point(px(0.), px(0.)), size(px(800.), px(480.))),
        ));
        builder.terminal.sync();
        builder
    }

    pub(super) fn screen_text(terminal: &Terminal) -> String {
        let mut text = String::new();
        let mut last_line = None;
        for indexed in &terminal.last_content.cells {
            if last_line.is_some_and(|line| line != indexed.point.line) {
                text.push('\n');
            }
            last_line = Some(indexed.point.line);
            text.push(indexed.cell.c);
        }
        text
    }

    pub(super) fn wait_for_text(terminal: &mut Terminal, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            terminal.sync();
            let text = screen_text(terminal);
            if text.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "no {needle:?} on screen:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn shell_runs_input() {
        let mut builder = spawn(&TerminalSettings::default());
        builder.terminal.input(b"echo out_$((6*7))\r".to_vec());
        wait_for_text(&mut builder.terminal, "out_42");
    }

    #[test]
    fn term_program_version_is_own() {
        let mut builder = spawn(&TerminalSettings::default());
        builder
            .terminal
            .input(b"echo ver=$TERM_PROGRAM_VERSION=\r".to_vec());
        wait_for_text(
            &mut builder.terminal,
            &format!("ver={}=", env!("CARGO_PKG_VERSION")),
        );
    }

    #[test]
    fn configured_shell_is_launched() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo from_settings_$((2+3)); sleep 5".into()],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        wait_for_text(&mut builder.terminal, "from_settings_5");
    }

    #[test]
    fn profile_env_and_working_directory_reach_shell() {
        let home = std::env::var("HOME").unwrap();
        let mut profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "echo \"env=$KUTERM_TEST:$TERM pwd=$(pwd)=\"; sleep 5".into(),
            ],
        });
        profile.working_directory = Some("~".into());
        profile
            .env
            .insert("KUTERM_TEST".into(), "from_profile".into());
        // profile env overrides kuterm's own
        profile.env.insert("TERM".into(), "dumb".into());
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        wait_for_text(
            &mut builder.terminal,
            &format!("env=from_profile:dumb pwd={home}="),
        );

        profile.working_directory = Some("/tmp".into());
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        wait_for_text(&mut builder.terminal, "pwd=/tmp=");
    }

    #[test]
    fn profile_theme_follows_appearance() {
        use gpui::WindowAppearance;

        use crate::{
            settings::{ThemeMode, ThemeSettings},
            theme::Theme,
        };

        let mut builder = spawn(&TerminalSettings::default());
        builder.terminal.apply_theme(WindowAppearance::Light);
        assert_eq!(builder.terminal.theme, None);

        let mut profile = profile(Shell::System);
        profile.theme = Some(ThemeSettings {
            mode: ThemeMode::System,
            dark: None,
            light: None,
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        builder.terminal.apply_theme(WindowAppearance::Light);
        assert_eq!(builder.terminal.theme, Some(Theme::bundled(false)));
        builder.terminal.apply_theme(WindowAppearance::Dark);
        assert_eq!(builder.terminal.theme, Some(Theme::bundled(true)));
    }

    #[test]
    fn configured_cursor_shape_is_used() {
        let settings = TerminalSettings {
            cursor_shape: CursorShape::Bar,
            ..TerminalSettings::default()
        };
        let builder = spawn(&settings);
        assert_eq!(
            builder.terminal.last_content.cursor.shape,
            alacritty_terminal::vte::ansi::CursorShape::Beam
        );
    }

    #[test]
    fn program_can_ask_for_blinking_cursor() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "read _; printf '\\033[1 qblink_on'; read _; printf '\\033[2 qblink_off'; sleep 5"
                    .into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        assert!(!terminal.last_content.cursor_blinking);
        terminal.input(b"\r".to_vec());
        wait_for_text(terminal, "blink_on");
        assert!(terminal.last_content.cursor_blinking);
        terminal.input(b"\r".to_vec());
        wait_for_text(terminal, "blink_off");
        assert!(!terminal.last_content.cursor_blinking);
    }

    #[test]
    fn resize_reaches_shell() {
        let mut builder = spawn(&TerminalSettings::default());
        builder.terminal.set_size(TerminalBounds::new(
            px(20.),
            px(10.),
            Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(500.))),
        ));
        builder.terminal.input(b"echo size=$(stty size)\r".to_vec());
        wait_for_text(&mut builder.terminal, "size=25 100");
    }

    #[test]
    fn exit_emits_exit_event() {
        let mut builder = spawn(&TerminalSettings::default());
        builder.terminal.input(b"exit\r".to_vec());
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            while let Some(event) = builder.events_rx.next().now_or_never() {
                match event {
                    Some(AlacTermEvent::Exit | AlacTermEvent::ChildExit(_)) => return,
                    None => panic!("event channel closed without an exit event"),
                    Some(_) => {}
                }
            }
            assert!(Instant::now() < deadline, "no exit event after `exit`");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn foreground_process_follows_shell() {
        let mut builder = spawn(&TerminalSettings::default());
        builder.terminal.input(b"cd /tmp && sleep 5\r".to_vec());
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let process = foreground_process(builder.terminal.shell_pid);
            if let Some(ForegroundProcess { name, cwd, .. }) = &process
                && name == "sleep"
            {
                assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/tmp")));
                return;
            }
            assert!(
                Instant::now() < deadline,
                "sleep is not in foreground: {process:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// terminal that prints 200 numbered lines, then waits
    fn spawn_long_output(settings: &TerminalSettings) -> TerminalBuilder {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "read _; i=0; while [ $i -lt 200 ]; do echo line_$i; i=$((i+1)); done; echo long_done; sleep 5"
                    .into(),
            ],
        });
        let mut builder = spawn_with(settings, &profile);
        // print only after the resize, growing the screen later pulls lines out of history
        builder.terminal.input(b"\r".to_vec());
        wait_for_text(&mut builder.terminal, "long_done");
        builder
    }

    #[test]
    fn max_history_length_limits_scrollback() {
        let settings = TerminalSettings {
            max_history_length: 50,
            ..TerminalSettings::default()
        };
        let builder = spawn_long_output(&settings);
        assert_eq!(builder.terminal.last_content.history_size, 50);

        // 0 keeps everything
        let settings = TerminalSettings {
            max_history_length: 0,
            ..TerminalSettings::default()
        };
        let builder = spawn_long_output(&settings);
        assert!(builder.terminal.last_content.history_size >= 170);
    }

    #[test]
    fn scroll_to_moves_to_absolute_offset() {
        let mut builder = spawn_long_output(&TerminalSettings::default());
        let terminal = &mut builder.terminal;
        let history = terminal.last_content.history_size;

        terminal.scroll_to(10);
        terminal.sync();
        assert_eq!(terminal.last_content.display_offset, 10);
        // absolute, so repeating it does not scroll further
        terminal.scroll_to(10);
        terminal.sync();
        assert_eq!(terminal.last_content.display_offset, 10);

        terminal.scroll_to(usize::MAX);
        terminal.sync();
        assert_eq!(terminal.last_content.display_offset, history);
        assert!(screen_text(terminal).contains("line_0"));

        terminal.scroll_to(0);
        terminal.sync();
        assert_eq!(terminal.last_content.display_offset, 0);
    }

    #[test]
    fn sync_skips_unchanged_grid_and_reuses_cell_buffer() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo quiet_ready; sleep 5".into()],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "quiet_ready");
        // let the last wakeup land, nothing prints after that
        std::thread::sleep(Duration::from_millis(200));
        terminal.sync();

        let buffer = terminal.last_content.cells.as_ptr();
        // a clean sync must leave the snapshot alone, so a wiped one stays wiped
        let cells = std::mem::take(&mut terminal.last_content.cells);
        terminal.sync();
        assert!(
            terminal.last_content.cells.is_empty(),
            "clean sync copied the grid"
        );

        // a local change marks it dirty and the old buffer is filled again in place
        terminal.last_content.cells = cells;
        terminal.scroll(1);
        terminal.sync();
        assert!(screen_text(terminal).contains("quiet_ready"));
        assert_eq!(terminal.last_content.cells.as_ptr(), buffer);
    }

    #[gpui::test]
    fn process_event_handles_alacritty_events(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(crate::theme::Theme::default()));
        let builder = spawn(&TerminalSettings::default());
        let terminal = cx.new(|_| builder.terminal);

        terminal.update(cx, |terminal, cx| {
            terminal.process_event(AlacTermEvent::Title("hello".into()), cx);
        });
        assert_eq!(
            terminal.read_with(cx, |terminal, _| terminal.title.clone()),
            "hello"
        );
        assert_eq!(
            terminal.read_with(cx, |terminal, _| terminal.title("default")),
            "hello"
        );
        terminal.update(cx, |terminal, cx| {
            terminal.process_event(AlacTermEvent::ResetTitle, cx);
        });
        assert!(terminal.read_with(cx, |terminal, _| terminal.title.is_empty()));
        assert_eq!(
            terminal.read_with(cx, |terminal, _| terminal.title("default")),
            "default"
        );

        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                AlacTermEvent::ClipboardStore(
                    alacritty_terminal::term::ClipboardType::Clipboard,
                    "stored".into(),
                ),
                cx,
            );
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("stored".into())
        );

        let load = std::sync::Arc::new(|text: &str| format!("load:{text}"));
        let size = std::sync::Arc::new(|size: alacritty_terminal::event::WindowSize| {
            format!("{}x{}", size.num_lines, size.num_cols)
        });
        let color = std::sync::Arc::new(|rgb: alacritty_terminal::vte::ansi::Rgb| {
            format!("{},{},{}", rgb.r, rgb.g, rgb.b)
        });
        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                AlacTermEvent::ClipboardLoad(
                    alacritty_terminal::term::ClipboardType::Clipboard,
                    load,
                ),
                cx,
            );
            terminal.process_event(AlacTermEvent::PtyWrite("p".into()), cx);
            terminal.process_event(AlacTermEvent::TextAreaSizeRequest(size), cx);
            terminal.process_event(AlacTermEvent::ColorRequest(258, color.clone()), cx);
            terminal.process_event(AlacTermEvent::ColorRequest(0, color), cx);
        });

        terminal.update(cx, |terminal, cx| {
            terminal.process_event(AlacTermEvent::MouseCursorDirty, cx);
            terminal.process_event(AlacTermEvent::CursorBlinkingChange, cx);
            terminal.process_event(AlacTermEvent::Bell, cx);
        });

        let status = std::process::Command::new("true").status().unwrap();
        terminal.update(cx, |terminal, cx| {
            terminal.process_event(AlacTermEvent::Exit, cx);
            terminal.process_event(AlacTermEvent::ChildExit(status), cx);
            terminal.process_event(AlacTermEvent::Wakeup, cx);
        });
    }
}

#[cfg(test)]
mod history_cap_terminal {

    use gpui::{Bounds, point, px, size};

    use super::TerminalBounds;
    use super::tests::{profile, spawn_with, wait_for_text};
    use crate::settings::{MAX_HISTORY_LENGTH, Settings, Shell, TerminalSettings};

    fn run(settings: &TerminalSettings, lines: usize) -> super::TerminalBuilder {
        let p = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                format!(
                    "read go; i=0; while [ $i -lt {lines} ]; do echo row_$i; i=$((i+1)); done; echo agent_done; sleep 5"
                ),
            ],
        });
        let mut b = spawn_with(settings, &p);
        b.terminal.input(b"\r".to_vec());
        wait_for_text(&mut b.terminal, "agent_done");
        b
    }

    fn resize(b: &mut super::TerminalBuilder, w: f32, h: f32) {
        b.terminal.set_size(TerminalBounds::new(
            px(20.),
            px(10.),
            Bounds::new(point(px(0.), px(0.)), size(px(w), px(h))),
        ));
        b.terminal.sync();
    }

    fn huge() -> TerminalSettings {
        let json = format!(
            r#"{{"terminal": {{"max_history_length": {}}}}}"#,
            usize::MAX
        );
        Settings::parse(&json).unwrap().terminal
    }

    #[test]
    fn huge_history_setting_spawns_and_prints() {
        let b = run(&huge(), 200);
        assert!(b.terminal.last_content.history_size > 100);
    }

    #[test]
    fn huge_history_setting_survives_resizes() {
        let mut b = run(&huge(), 200);
        resize(&mut b, 400., 200.);
        resize(&mut b, 1600., 1200.);
        resize(&mut b, 100., 40.);
        assert!(b.terminal.history_size() > 0);
    }

    #[test]
    fn cap_value_survives_resizes() {
        let s = TerminalSettings {
            max_history_length: MAX_HISTORY_LENGTH,
            ..TerminalSettings::default()
        };
        let mut b = run(&s, 50);
        resize(&mut b, 1600., 1200.);
        resize(&mut b, 200., 100.);
    }

    #[test]
    fn zero_history_survives_resizes() {
        let s = TerminalSettings {
            max_history_length: 0,
            ..TerminalSettings::default()
        };
        let mut b = run(&s, 50);
        resize(&mut b, 1600., 1200.);
        resize(&mut b, 200., 100.);
    }
}

#[cfg(test)]
mod profiles_terminal {

    use gpui::WindowAppearance;

    use super::tests::{profile, spawn_with, wait_for_text};
    use crate::{
        settings::{Shell, TerminalSettings, ThemeMode, ThemeSettings},
        theme::Theme,
    };

    fn sh(script: &str) -> Shell {
        Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), format!("{script}; sleep 5")],
        }
    }

    #[test]
    fn profile_program_command_runs() {
        let p = profile(Shell::Program("/bin/sh".into()));
        let mut b = spawn_with(&TerminalSettings::default(), &p);
        b.terminal.input(b"echo prog_$((6*7))\r".to_vec());
        wait_for_text(&mut b.terminal, "prog_42");
    }

    #[test]
    fn several_env_vars_reach_command() {
        let mut p = profile(sh("echo \"vars=$AGENT_A-$AGENT_B-$AGENT_C=\""));
        p.env.insert("AGENT_A".into(), "one".into());
        p.env.insert("AGENT_B".into(), "two words".into());
        p.env.insert("AGENT_C".into(), "".into());
        let mut b = spawn_with(&TerminalSettings::default(), &p);
        wait_for_text(&mut b.terminal, "vars=one-two words-=");
    }

    #[test]
    fn profile_env_overrides_term_program() {
        let mut p = profile(sh("echo \"tp=$TERM_PROGRAM=\""));
        p.env.insert("TERM_PROGRAM".into(), "agent_override".into());
        let mut b = spawn_with(&TerminalSettings::default(), &p);
        wait_for_text(&mut b.terminal, "tp=agent_override=");
    }

    #[test]
    fn env_of_one_profile_does_not_leak_into_another() {
        let mut p = profile(sh("echo \"leak=${AGENT_LEAK:-none}=\""));
        p.env.insert("AGENT_LEAK".into(), "yes".into());
        let mut b = spawn_with(&TerminalSettings::default(), &p);
        wait_for_text(&mut b.terminal, "leak=yes=");
        let p = profile(sh("echo \"leak=${AGENT_LEAK:-none}=\""));
        let mut b = spawn_with(&TerminalSettings::default(), &p);
        wait_for_text(&mut b.terminal, "leak=none=");
    }

    #[test]
    fn absolute_working_directory_is_used() {
        let dir = std::env::temp_dir().join(format!("kuterm_agent_wd_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let mut p = profile(sh("echo \"wd=$(pwd -P)=\""));
        p.working_directory = Some(dir.clone());
        let mut b = spawn_with(&TerminalSettings::default(), &p);
        wait_for_text(&mut b.terminal, &format!("wd={}=", dir.display()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tilde_subfolder_is_expanded_to_home() {
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
        let name = format!("kuterm_agent_home_{}", std::process::id());
        let dir = home.join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = profile(sh("echo \"hw=$(pwd)=\""));
        p.working_directory = Some(format!("~/{name}").into());
        let mut b = spawn_with(&TerminalSettings::default(), &p);
        wait_for_text(&mut b.terminal, &format!("hw={}=", dir.display()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_working_directory_uses_kuterm_folder() {
        let cwd = std::env::current_dir().unwrap();
        let p = profile(sh("echo \"cw=$(pwd)=\""));
        let mut b = spawn_with(&TerminalSettings::default(), &p);
        wait_for_text(&mut b.terminal, &format!("cw={}=", cwd.display()));
    }

    #[test]
    fn profile_theme_mode_overrides_appearance() {
        let mut p = profile(Shell::System);
        p.theme = Some(ThemeSettings {
            mode: ThemeMode::Dark,
            dark: None,
            light: None,
        });
        let mut b = spawn_with(&TerminalSettings::default(), &p);
        b.terminal.apply_theme(WindowAppearance::Light);
        assert_eq!(b.terminal.theme, Some(Theme::bundled(true)));

        p.theme = Some(ThemeSettings {
            mode: ThemeMode::Light,
            dark: None,
            light: None,
        });
        let mut b = spawn_with(&TerminalSettings::default(), &p);
        b.terminal.apply_theme(WindowAppearance::Dark);
        assert_eq!(b.terminal.theme, Some(Theme::bundled(false)));
    }

    #[test]
    fn profile_without_theme_has_no_own_theme() {
        let p = profile(Shell::System);
        let mut b = spawn_with(&TerminalSettings::default(), &p);
        for appearance in [WindowAppearance::Dark, WindowAppearance::Light] {
            b.terminal.apply_theme(appearance);
            assert_eq!(b.terminal.theme, None);
        }
    }
}

#[cfg(test)]
mod scrollbar_terminal {

    use super::tests::{profile, screen_text, spawn_with, wait_for_text};
    use crate::settings::{Shell, TerminalSettings};

    fn printing(lines: usize, settings: &TerminalSettings) -> super::TerminalBuilder {
        let p = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                format!(
                    "read go; i=0; while [ $i -lt {lines} ]; do echo row_$i; i=$((i+1)); done; echo agent_done; sleep 5"
                ),
            ],
        });
        let mut b = spawn_with(settings, &p);
        b.terminal.input(b"\r".to_vec());
        wait_for_text(&mut b.terminal, "agent_done");
        b
    }

    fn with_history(n: usize) -> TerminalSettings {
        TerminalSettings {
            max_history_length: n,
            ..TerminalSettings::default()
        }
    }

    #[test]
    fn short_output_has_no_history() {
        let b = printing(1, &with_history(100));
        assert_eq!(b.terminal.last_content.history_size, 0);
    }

    #[test]
    fn history_capped_at_setting() {
        let b = printing(300, &with_history(10));
        assert_eq!(b.terminal.last_content.history_size, 10);
    }

    #[test]
    fn history_below_cap_is_not_padded() {
        let b = printing(300, &with_history(100_000));
        let h = b.terminal.last_content.history_size;
        assert!(h > 200 && h < 400, "history {h}");
    }

    #[test]
    fn zero_history_is_unlimited() {
        let b = printing(1500, &with_history(0));
        assert!(b.terminal.last_content.history_size >= 1400);
    }

    #[test]
    fn scroll_to_top_shows_first_line() {
        let mut b = printing(300, &with_history(0));
        let t = &mut b.terminal;
        let h = t.last_content.history_size;
        t.scroll_to(h);
        t.sync();
        assert_eq!(t.last_content.display_offset, h);
        assert!(screen_text(t).contains("row_0"));
    }

    #[test]
    fn scroll_to_is_absolute_not_relative() {
        let mut b = printing(300, &with_history(0));
        let t = &mut b.terminal;
        t.scroll_to(50);
        t.sync();
        t.scroll_to(20);
        t.sync();
        assert_eq!(t.last_content.display_offset, 20);
        t.scroll(5);
        t.sync();
        t.scroll_to(7);
        t.sync();
        assert_eq!(t.last_content.display_offset, 7);
    }

    #[test]
    fn scroll_to_past_history_clamps() {
        let mut b = printing(300, &with_history(40));
        let t = &mut b.terminal;
        t.scroll_to(10_000);
        t.sync();
        assert_eq!(t.last_content.display_offset, 40);
    }

    #[test]
    fn scroll_to_zero_returns_to_bottom() {
        let mut b = printing(300, &with_history(0));
        let t = &mut b.terminal;
        t.scroll_to(100);
        t.sync();
        t.scroll_to(0);
        t.sync();
        assert_eq!(t.last_content.display_offset, 0);
        assert!(screen_text(t).contains("agent_done"));
    }

    #[test]
    fn scroll_to_without_history_stays_at_bottom() {
        let mut b = printing(1, &with_history(100));
        let t = &mut b.terminal;
        t.scroll_to(5);
        t.sync();
        assert_eq!(t.last_content.display_offset, 0);
    }
}
