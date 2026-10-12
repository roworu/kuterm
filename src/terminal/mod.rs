//! terminal model: runs shell and keeps screen state

mod bounds;
mod builder;
mod content;
mod events;
mod graphics;
mod integration;
mod keys;
mod links;
mod mouse;
mod process;
mod scanner;
mod scrollback;
mod search;
mod selection;
mod vi;

use std::{
    borrow::Cow,
    collections::HashMap,
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
    vte::ansi::{ClearMode, Handler},
};
use gpui::{App, ClipboardItem, Context, EventEmitter, Keystroke, Task, WindowAppearance};

pub use bounds::TerminalBounds;
pub use builder::TerminalBuilder;
pub use content::{Content, IndexedCell};
pub use integration::with_shell_integration;
pub use links::{Link, TextPatterns};
pub use mouse::{MouseAction, MouseButton};
#[cfg(test)]
pub(crate) use process::tests::{Kill, spawn};
pub use process::{ForegroundProcess, children, foreground_process, process_info};
pub use scanner::ScanEvent;
pub use search::{SearchQuery, SearchStatus};
pub use vi::ViKey;

use builder::{ZedListener, term_config};
pub use keys::KeyEventKind;
use keys::{to_esc_str, to_kitty_str};

use crate::{
    settings::{ClipboardSettings, ProfileTheme, TerminalSettings, ThemeSettings},
    theme::Theme,
};

/// events emitted to terminal view
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    TitleChanged,
    Wakeup,
    CloseTerminal,
    /// the program rang the bell
    Bell,
    /// the program wants the clipboard and settings say to ask, `text` is what it would
    /// copy, none when it wants to read. answered with `answer_clipboard`
    ClipboardAsk {
        id: usize,
        text: Option<String>,
    },
    /// a command the shell marked finished, after running this long
    CommandFinished {
        duration: std::time::Duration,
        exit: Option<i32>,
    },
    /// the program sent a notification with osc 9 or osc 777
    Notify {
        title: String,
        body: String,
    },
}

/// clipboard access waiting for the user to allow it
enum ClipboardRequest {
    Write(String),
    /// builds the reply to the program from the clipboard text
    Read(Arc<dyn Fn(&str) -> String + Sync + Send + 'static>),
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
    theme_settings: Option<ProfileTheme>,
    theme: Option<Theme>,
    /// what programs may do with the clipboard
    clipboard: ClipboardSettings,
    /// clipboard accesses asked about, by id
    clipboard_requests: HashMap<usize, ClipboardRequest>,
    next_clipboard_request: usize,
    /// running search, none while the search bar is closed
    search: Option<search::SearchState>,
    /// count and pending keys of vi mode
    vi: vi::ViState,
    /// shell integration marks and notifications found in the output
    scan_events: scanner::ScanEvents,
    /// folder the shell reported with osc 7
    reported_cwd: Option<std::path::PathBuf>,
    /// when the running command started, from shell integration marks
    command_started: Option<std::time::Instant>,
    /// images the program showed
    graphics: graphics::SharedGraphics,
    /// cell size for sizing images, shared with the pty thread
    cell_size: Arc<graphics::CellSize>,
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

    /// load the profile theme for this system appearance, its names come from `themes`
    pub fn apply_theme(&mut self, appearance: WindowAppearance, themes: &ThemeSettings) {
        self.theme = self
            .theme_settings
            .as_ref()
            .map(|profile| Theme::for_appearance(&themes.with_profile(profile), appearance));
    }

    /// apply changed settings, like word separators, to the running terminal
    pub fn apply_settings(&mut self, settings: &TerminalSettings) {
        self.clipboard = settings.clipboard;
        self.term.lock().set_options(term_config(settings));
        self.dirty.store(true, Ordering::Release);
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
        self.cell_size.set(
            f32::from(new_bounds.cell_width),
            f32::from(new_bounds.line_height),
        );
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
    pub fn try_keystroke(
        &mut self,
        keystroke: &Keystroke,
        kind: KeyEventKind,
        option_as_meta: bool,
    ) -> bool {
        let mode = self.last_content.mode;
        if let Some(esc) = to_kitty_str(keystroke, mode, kind) {
            self.input(esc.into_bytes());
            return true;
        }
        // only kitty's protocol tells about repeats and releases
        if kind == KeyEventKind::Release {
            return false;
        }
        match to_esc_str(keystroke, mode, option_as_meta) {
            Some(Cow::Borrowed(esc)) => self.input(esc.as_bytes()),
            Some(Cow::Owned(esc)) => self.input(esc.into_bytes()),
            None => return false,
        }
        true
    }

    /// paste text, wrapping it when program asked for bracketed paste
    pub fn paste(&mut self, text: &str) {
        let bracketed = self.last_content.mode.contains(TermMode::BRACKETED_PASTE);
        self.input(pasted(text, bracketed).into_bytes());
    }

    /// true when pasting `text` should be confirmed: without bracketed paste a newline runs
    /// it right away and control characters reach the program as keys
    pub fn paste_is_unsafe(&self, text: &str) -> bool {
        !self.last_content.mode.contains(TermMode::BRACKETED_PASTE)
            && text.chars().any(|c| c.is_control() && c != '\t')
    }

    /// let or refuse a clipboard access the program asked for
    pub fn answer_clipboard(&mut self, id: usize, allow: bool, cx: &mut Context<Self>) {
        let Some(request) = self.clipboard_requests.remove(&id) else {
            return;
        };
        if allow {
            self.use_clipboard(request, cx);
        }
    }

    fn use_clipboard(&mut self, request: ClipboardRequest, cx: &mut Context<Self>) {
        match request {
            ClipboardRequest::Write(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            ClipboardRequest::Read(format) => {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                self.write_to_pty(format(&text).into_bytes());
            }
        }
    }

    /// turn images on or off and limit the memory they take
    pub fn configure_graphics(&self, enable: bool, max_memory_mb: usize) {
        self.graphics
            .lock()
            .unwrap()
            .configure(enable, max_memory_mb);
        self.dirty.store(true, Ordering::Release);
    }

    /// images no longer shown, their textures should be freed
    pub fn take_dropped_images(&self) -> Vec<Arc<gpui::RenderImage>> {
        self.graphics.lock().unwrap().take_dropped()
    }

    /// folder the shell reported through shell integration, none when it never did
    pub fn reported_cwd(&self) -> Option<&std::path::Path> {
        self.reported_cwd.as_deref()
    }

    /// take what the scanner found in the output, keeping track of the shell, and return
    /// the events the workspace should hear about
    pub(crate) fn handle_scan_events(&mut self) -> Vec<Event> {
        let found = std::mem::take(&mut *self.scan_events.lock().unwrap());
        let mut events = Vec::new();
        for (at, event) in found {
            match event {
                ScanEvent::PromptStart => {}
                ScanEvent::CommandStart => self.command_started = Some(at),
                ScanEvent::CommandEnd(exit) => {
                    // a prompt drawn without running anything ends nothing
                    if let Some(started) = self.command_started.take() {
                        events.push(Event::CommandFinished {
                            duration: at.saturating_duration_since(started),
                            exit,
                        });
                    }
                }
                ScanEvent::Cwd(path) => self.reported_cwd = Some(path),
                ScanEvent::Notify { title, body } => events.push(Event::Notify { title, body }),
                ScanEvent::Reply(bytes) => self.write_to_pty(bytes),
            }
        }
        events
    }

    /// drop scrollback and everything above the cursor line, which moves to the top
    pub fn clear(&mut self) {
        let mut term = self.term.lock();
        // full screen programs own their screen, they redraw it themselves
        if !term.mode().contains(TermMode::ALT_SCREEN) {
            // moving the cursor line to the top keeps the prompt and typed text. asking the
            // shell to redraw with ctrl-l would also reach any program running in it
            let cursor = term.grid().cursor.point;
            term.scroll_up(cursor.line.0.max(0) as usize);
            term.goto(0, cursor.column.0);
            term.clear_screen(ClearMode::Saved);
            term.selection = None;
        }
        drop(term);
        self.events.push(InternalEvent::Scroll(Scroll::Bottom));
        self.dirty.store(true, Ordering::Release);
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
        let graphics = self.graphics.lock().unwrap();
        self.last_content
            .refresh(&term, self.search.as_mut(), &graphics);
    }
}

/// bytes a paste sends: bracketed paste wraps it and drops control characters, so the text
/// can't fake the end of the paste, otherwise newlines become enter presses
fn pasted(text: &str, bracketed: bool) -> String {
    if bracketed {
        let text: String = text
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
            .collect();
        format!("\x1b[200~{text}\x1b[201~")
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r")
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

    use alacritty_terminal::term::TermMode;

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
            settings::{ProfileTheme, Settings, ThemeMode},
            theme::Theme,
        };

        let mut builder = spawn(&TerminalSettings::default());
        builder
            .terminal
            .apply_theme(WindowAppearance::Light, &Settings::default().theme);
        assert_eq!(builder.terminal.theme, None);

        let mut profile = profile(Shell::System);
        profile.theme = Some(ProfileTheme {
            mode: ThemeMode::System,
            dark: None,
            light: None,
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        builder
            .terminal
            .apply_theme(WindowAppearance::Light, &Settings::default().theme);
        assert_eq!(builder.terminal.theme, Some(Theme::bundled(false)));
        builder
            .terminal
            .apply_theme(WindowAppearance::Dark, &Settings::default().theme);
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
    fn history_cap_survives_resizes() {
        let settings = TerminalSettings {
            max_history_length: crate::settings::MAX_HISTORY_LENGTH,
            ..TerminalSettings::default()
        };
        let mut builder = spawn_long_output(&settings);
        for (width, height) in [(1600., 1200.), (100., 40.)] {
            builder.terminal.set_size(TerminalBounds::new(
                px(20.),
                px(10.),
                Bounds::new(point(px(0.), px(0.)), size(px(width), px(height))),
            ));
            builder.terminal.sync();
        }
        assert!(builder.terminal.history_size() > 0);
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

    fn search_for(text: &str, regex: bool, whole_word: bool) -> super::SearchQuery {
        super::SearchQuery {
            text: text.into(),
            regex,
            whole_word,
            case: crate::settings::SearchCase::Smart,
        }
    }

    #[test]
    fn search_counts_matches_and_walks_them_up_from_the_bottom() {
        let mut builder = spawn_long_output(&TerminalSettings::default());
        let terminal = &mut builder.terminal;
        // line_1, line_10..line_19 and line_100..line_199
        let status = terminal
            .search(&search_for("line_1", false, false))
            .unwrap();
        assert_eq!(status.count, 111);
        // the closest match above the bottom of the screen
        assert_eq!(status.active, Some(110));
        terminal.sync();
        let active = terminal.last_content.active_match.clone().unwrap();
        let line: String = terminal
            .last_content
            .cells
            .iter()
            .filter(|cell| cell.point.line == active.start().line)
            .map(|cell| cell.cell.c)
            .collect();
        assert!(line.starts_with("line_199"), "{line:?}");
        assert!(!terminal.last_content.search_matches.is_empty());

        assert_eq!(terminal.search_next(true).active, Some(109));
        assert_eq!(terminal.search_next(false).active, Some(110));
        // wraps around at the ends
        assert_eq!(terminal.search_next(false).active, Some(0));
        assert_eq!(terminal.search_next(true).active, Some(110));

        terminal.end_search();
        terminal.sync();
        assert!(terminal.last_content.search_matches.is_empty());
        assert_eq!(terminal.last_content.active_match, None);
    }

    #[test]
    fn search_scrolls_to_a_match_in_history() {
        let mut builder = spawn_long_output(&TerminalSettings::default());
        let terminal = &mut builder.terminal;
        let status = terminal.search(&search_for("line_3", false, true)).unwrap();
        // whole words only: not line_30 and the others
        assert_eq!(status.count, 1);
        terminal.sync();
        assert!(terminal.last_content.display_offset > 0);
        let active = terminal.last_content.active_match.clone().unwrap();
        let visible = -(terminal.last_content.display_offset as i32);
        assert!(
            active.start().line.0 >= visible,
            "{active:?} above the view"
        );
        assert!(
            screen_text(terminal)
                .lines()
                .any(|line| line.trim_end() == "line_3")
        );
    }

    #[test]
    fn search_reports_broken_regex_and_no_matches() {
        let mut builder = spawn_long_output(&TerminalSettings::default());
        let terminal = &mut builder.terminal;
        assert!(terminal.search(&search_for("line_(", true, false)).is_err());
        let status = terminal
            .search(&search_for(r"line_1\d\d", true, false))
            .unwrap();
        assert_eq!(status.count, 100);
        let status = terminal
            .search(&search_for("nowhere", false, false))
            .unwrap();
        assert_eq!((status.count, status.active), (0, None));
        assert_eq!(terminal.search_next(true).active, None);
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

    #[test]
    fn bracketed_paste_drops_control_characters() {
        assert_eq!(
            super::pasted("a\x1b[201~b\x03c\u{9b}d\te\nf\r", true),
            "\x1b[200~a[201~bcd\te\nf\r\x1b[201~"
        );
        assert_eq!(super::pasted("a\nb\r\nc", false), "a\rb\rc");
    }

    #[test]
    fn paste_is_unsafe_only_without_bracketed_paste() {
        let mut builder = spawn(&TerminalSettings::default());
        let terminal = &mut builder.terminal;
        terminal.last_content.mode = TermMode::empty();
        assert!(!terminal.paste_is_unsafe("ls -la\tdone"));
        assert!(terminal.paste_is_unsafe("ls\n"));
        assert!(terminal.paste_is_unsafe("ls\r"));
        assert!(terminal.paste_is_unsafe("\x1b[A"));
        terminal.last_content.mode = TermMode::BRACKETED_PASTE;
        assert!(!terminal.paste_is_unsafe("ls\nrm -rf x\x1b[201~"));
    }

    #[gpui::test]
    fn clipboard_access_follows_settings(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        use alacritty_terminal::term::ClipboardType;
        use gpui::ClipboardItem;

        use super::Event;
        use crate::settings::{ClipboardAccess, ClipboardSettings};

        cx.update(|cx| cx.set_global(crate::theme::Theme::default()));
        let settings = TerminalSettings {
            clipboard: ClipboardSettings {
                write: ClipboardAccess::Deny,
                read: ClipboardAccess::Ask,
            },
            ..TerminalSettings::default()
        };
        let builder = spawn(&settings);
        let terminal = cx.new(|_| builder.terminal);
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&terminal, move |_, event: &Event, _| {
                events.borrow_mut().push(event.clone())
            })
            .detach()
        });
        cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
        let store =
            |text: &str| AlacTermEvent::ClipboardStore(ClipboardType::Clipboard, text.into());
        let clipboard =
            |cx: &mut gpui::TestAppContext| cx.read_from_clipboard().and_then(|item| item.text());

        terminal.update(cx, |terminal, cx| {
            terminal.process_event(store("denied"), cx)
        });
        assert_eq!(clipboard(cx), Some("before".into()));
        let load = std::sync::Arc::new(|text: &str| format!("load:{text}"));
        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                AlacTermEvent::ClipboardLoad(ClipboardType::Clipboard, load),
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(
            *events.borrow(),
            [Event::ClipboardAsk { id: 0, text: None }]
        );

        terminal.update(cx, |terminal, cx| {
            terminal.clipboard.write = ClipboardAccess::Ask;
            terminal.process_event(store("asked"), cx);
        });
        cx.run_until_parked();
        assert_eq!(
            events.borrow().last(),
            Some(&Event::ClipboardAsk {
                id: 1,
                text: Some("asked".into())
            })
        );
        assert_eq!(clipboard(cx), Some("before".into()));
        terminal.update(cx, |terminal, cx| terminal.answer_clipboard(1, true, cx));
        assert_eq!(clipboard(cx), Some("asked".into()));
        // an answer counts once
        cx.write_to_clipboard(ClipboardItem::new_string("after".into()));
        terminal.update(cx, |terminal, cx| terminal.answer_clipboard(1, true, cx));
        assert_eq!(clipboard(cx), Some("after".into()));

        terminal.update(cx, |terminal, cx| {
            terminal.clipboard.write = ClipboardAccess::Allow;
            terminal.process_event(store("allowed"), cx);
        });
        assert_eq!(clipboard(cx), Some("allowed".into()));
    }

    #[test]
    fn program_turns_on_kitty_keyboard_protocol() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "stty raw -echo; printf '\\033[>1ukitty_on'; dd bs=1 count=8 2>/dev/null | od -An -tx1 | tr -d ' \\n'; echo =got; sleep 5"
                    .into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "kitty_on");
        assert!(
            terminal
                .last_content
                .mode
                .contains(TermMode::DISAMBIGUATE_ESC_CODES)
        );
        let ctrl_i = gpui::Keystroke::parse("ctrl-i").unwrap();
        assert!(terminal.try_keystroke(&ctrl_i, super::KeyEventKind::Press, false));
        // ESC [ 1 0 5 ; 5 u, ctrl-i is no longer the same as tab
        wait_for_text(terminal, "1b5b3130353b3575=got");
    }

    #[test]
    fn synchronized_output_is_not_shown_half_done() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                // the update is never ended, alacritty gives up waiting after a timeout
                "read _; printf 'sync_before\\n\\033[?2026hsync_inside'; sleep 5".into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        terminal.input(b"\r".to_vec());
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            // a local change makes `sync` take a snapshot even without new output
            terminal.scroll(0);
            terminal.sync();
            let text = screen_text(terminal);
            if text.contains("sync_before") {
                assert!(
                    !text.contains("sync_inside"),
                    "half done update shown:\n{text}"
                );
                break;
            }
            assert!(Instant::now() < deadline, "no output:\n{text}");
            std::thread::sleep(Duration::from_millis(2));
        }
        // the update shows up whole once it ends or times out
        wait_for_text(terminal, "sync_inside");
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
