//! terminal model: runs shell and keeps screen state

mod blocks;
mod bounds;
mod builder;
mod clipboard;
mod content;
mod context;
mod decorations;
mod events;
mod file_links;
mod frozen;
mod integration;
mod keys;
mod links;
mod mouse;
pub mod paste_check;
mod process;
mod prompt;
mod pty_state;
mod queue;
mod sampler;
mod scanner;
mod scrollback;
mod search;
mod selection;
mod sources;
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
use gpui::{App, EventEmitter, Keystroke, Task, WindowAppearance};

pub use bounds::TerminalBounds;
pub use builder::TerminalBuilder;
use clipboard::ClipboardRequest;
pub use content::{Content, IndexedCell};
pub use context::{PaneContext, git_root};
pub use decorations::{Decorations, Popover};
pub use file_links::{FileLine, parse_file_line, resolve as resolve_file, snippet};
pub use integration::with_shell_integration;
pub use links::{Link, TextPatterns};
pub use mouse::{MouseAction, MouseButton};
#[cfg(test)]
pub(crate) use process::tests::{Kill, spawn};
pub use process::{
    ForegroundProcess, PtyModes, StoppedJob, children, foreground_process, process_info, pty_modes,
    stopped_jobs,
};
pub use prompt::HintLine;
pub use pty_state::{PtyState, Question};
pub use queue::{CommandQueue, parse_entry as parse_queue_entry};
pub use sampler::{CPU_HISTORY, ProcessSampler, TreeSample};
pub use scanner::{Progress, ScanEvent};
pub use search::{FoundLine, SearchQuery, SearchScope, SearchStatus, search_pattern};
pub use sources::{InputSource, Sources};
#[cfg(test)]
pub(crate) use tests::parsed_term;
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
    /// a command started, block `block` holds it
    CommandStarted {
        block: u64,
    },
    /// the shell reported another folder
    CwdChanged(std::path::PathBuf),
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
    /// a shell of this tab reported its command line, for completions
    Shell(crate::completion::ShellEvent),
    /// the program started or stopped reporting progress with osc 9;4. a stop that came
    /// with the end of its command is not told
    BusyChanged(bool),
    /// the queue typed its next command, or a failure paused it
    QueueChanged,
    /// a command failed with `exit` while commands were queued, the queue waits
    QueuePaused {
        exit: Option<i32>,
    },
}

/// a source that wrote input, first and last time since the prompt
#[derive(Clone, Copy, Debug)]
struct TypedSource {
    first: std::time::Instant,
    last: std::time::Instant,
    source: InputSource,
}

/// `typed` without what was written before `at`
fn typed_after(typed: &mut Vec<TypedSource>, at: std::time::Instant) {
    typed.retain(|typed| typed.last > at);
    for typed in typed {
        typed.first = typed.first.max(at);
    }
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
    /// id of the pane for remote control, its shell has it in $KUTERM_PANE
    pub pane_id: u64,
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
    /// progress the running program reported with osc 9;4
    progress: Option<Progress>,
    /// when the running command started, from shell integration marks
    command_started: Option<std::time::Instant>,
    /// writes of user input so far, so others can tell whether input came since
    inputs: u64,
    /// numbers of the prompt and output marks of the last command started
    last_command: Option<(Option<usize>, usize)>,
    /// frame held while the mouse selects, none shows the live grid
    frozen: Option<frozen::Frozen>,
    /// commands the shell ran
    blocks: blocks::Blocks,
    /// the shell shows its prompt, between the `A` and `C` marks
    at_prompt: bool,
    /// number of the mark of the prompt shown, set at `B`
    prompt_mark: Option<usize>,
    /// the shell marks the end of its prompts with `B`
    saw_prompt_end: bool,
    /// the queue typed an entry the shell has not started or ended yet
    queue_typed: bool,
    /// drawn under the prompt line until the next command
    hint_line: Option<HintLine>,
    /// commands typed at the next prompts
    queue: CommandQueue,
    /// a failed command pauses the queue
    queue_stop_on_failure: bool,
    /// tty settings and stopped jobs from the last title refresh
    pty_state: PtyState,
    /// marks and popovers drawn with the grid
    decorations: Decorations,
    /// when ctrl+s paused output, none once something let it go
    xoff_at: Option<std::time::Instant>,
    /// ctrl+s from the keyboard may pause output
    send_xoff: bool,
    /// when the program last printed something
    last_output: std::time::Instant,
    /// what wrote input and when, since the prompt was drawn. timed, since the marks of the
    /// prompt may be handled after input already came
    typed_sources: Vec<TypedSource>,
    /// socket the shells of this tab report their command line on, none without autocomplete
    #[cfg(unix)]
    completion: Option<crate::completion::Bridge>,
    _event_loop_task: Task<()>,
    _completion_task: Task<()>,
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

    /// theme of the profile, none when the tab uses the global theme
    pub fn profile_theme(&self) -> Option<&Theme> {
        self.theme.as_ref()
    }

    /// load the profile theme for this system appearance, its names come from `themes`
    pub fn apply_theme(&mut self, appearance: WindowAppearance, themes: &ThemeSettings) {
        self.theme = self
            .theme_settings
            .as_ref()
            .map(|profile| Theme::for_appearance(&themes.with_profile(profile), appearance));
    }

    /// let the shells of this tab report their command line on `bridge`
    #[cfg(unix)]
    pub fn set_completion_bridge(&mut self, bridge: crate::completion::Bridge) {
        self.completion = Some(bridge);
    }

    /// send a line to the shell of `conn`, false when it is gone
    pub fn send_to_shell(&self, conn: u64, line: &str) -> bool {
        #[cfg(unix)]
        return self
            .completion
            .as_ref()
            .is_some_and(|bridge| bridge.send(conn, line));
        #[cfg(not(unix))]
        return false;
    }

    /// apply changed settings, like word separators, to the running terminal
    pub fn apply_settings(&mut self, settings: &TerminalSettings) {
        self.clipboard = settings.clipboard;
        self.queue_stop_on_failure = settings.queue_stop_on_failure;
        self.send_xoff = settings.send_xoff;
        self.term.lock().set_options(term_config(settings));
        self.dirty.store(true, Ordering::Release);
    }

    fn write_to_pty(&self, input: impl Into<Cow<'static, [u8]>>) {
        self.pty_tx.notify(input.into());
    }

    /// send input from `source`, dropping any selection and jumping back to bottom of
    /// scrollback. false when it was refused, like automation typing into a password prompt
    pub fn input(&mut self, source: InputSource, input: impl Into<Cow<'static, [u8]>>) -> bool {
        let input = input.into();
        // the last refresh may be old for a hidden tab, so the pty is read again
        if !source.may_answer_secret() && (self.asking_secret() || self.asking_secret_now()) {
            return false;
        }
        if source == InputSource::Keyboard && self.xoff_filter(&input).is_none() {
            return false;
        }
        self.note_source(source);
        self.inputs += 1;
        self.clear_selection();
        self.events.push(InternalEvent::Scroll(Scroll::Bottom));
        self.write_to_pty(input);
        true
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

    fn note_source(&mut self, source: InputSource) {
        let now = std::time::Instant::now();
        // a long session at one prompt only needs each source once
        match self
            .typed_sources
            .iter_mut()
            .find(|typed| typed.source == source)
        {
            Some(typed) => typed.last = now,
            None => self.typed_sources.push(TypedSource {
                first: now,
                last: now,
                source,
            }),
        }
    }

    /// map a keystroke from `source` to an escape sequence and write it, returns false if
    /// unmapped
    pub fn try_keystroke(
        &mut self,
        keystroke: &Keystroke,
        kind: KeyEventKind,
        option_as_meta: bool,
        source: InputSource,
    ) -> bool {
        let mode = self.last_content.mode;
        if let Some(esc) = to_kitty_str(keystroke, mode, kind) {
            self.input(source, esc.into_bytes());
            return true;
        }
        // only kitty's protocol tells about repeats and releases
        if kind == KeyEventKind::Release {
            return false;
        }
        match to_esc_str(keystroke, mode, option_as_meta) {
            Some(Cow::Borrowed(esc)) => self.input(source, esc.as_bytes()),
            Some(Cow::Owned(esc)) => self.input(source, esc.into_bytes()),
            None => return false,
        };
        true
    }

    /// progress the running program reported, none when it reports nothing
    pub fn progress(&self) -> Option<Progress> {
        self.progress
    }

    /// writes of user input so far
    pub fn inputs(&self) -> u64 {
        self.inputs
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
                ScanEvent::PromptStart => {
                    typed_after(&mut self.typed_sources, at);
                    // the line editor redraws the prompt with its marks, like on a resize, while
                    // it still reads an entry the queue typed. the entry's result comes first
                    self.at_prompt = !self.queue_typed;
                    self.prompt_mark = None;
                    // shells that never mark the prompt's end get the queue at its start,
                    // the line editor reads what was typed once it is up
                    if !self.saw_prompt_end && self.run_queue() {
                        events.push(Event::QueueChanged);
                    }
                }
                ScanEvent::PromptEnd(mark) => {
                    typed_after(&mut self.typed_sources, at);
                    self.saw_prompt_end = true;
                    self.prompt_mark = mark;
                    if !self.queue_typed {
                        self.at_prompt = true;
                        if self.run_queue() {
                            events.push(Event::QueueChanged);
                        }
                    }
                }
                ScanEvent::CommandStart { prompt, output } => {
                    self.at_prompt = false;
                    self.queue_typed = false;
                    self.prompt_mark = None;
                    self.hint_line = None;
                    self.command_started = Some(at);
                    self.last_command = Some((prompt, output));
                    let term = self.term.lock_unfair();
                    let cwd = self.reported_cwd.clone();
                    // what came after the command started is input to the program, not its line
                    let sources: Sources = self
                        .typed_sources
                        .iter()
                        .filter(|typed| typed.first <= at)
                        .map(|typed| typed.source)
                        .collect();
                    typed_after(&mut self.typed_sources, at);
                    let block = self.blocks.start(&term, prompt, output, cwd, at, sources);
                    drop(term);
                    events.push(Event::CommandStarted { block });
                }
                ScanEvent::CommandLine(line) => self.blocks.command_line(line),
                ScanEvent::CommandEnd(exit) => {
                    // a typed entry cancelled at the prompt ends without starting
                    self.queue_typed = false;
                    // a program killed mid way never takes its progress down
                    self.progress = None;
                    // an idle right before the end is the program finishing, not waiting
                    let last = events
                        .iter()
                        .rposition(|event| matches!(event, Event::BusyChanged(_)));
                    if let Some(at) = last
                        && matches!(events[at], Event::BusyChanged(false))
                    {
                        events.remove(at);
                    }
                    self.blocks.end(exit, at);
                    if self.queue.ended(exit, self.queue_stop_on_failure) {
                        events.push(Event::QueuePaused { exit });
                    }
                    // a prompt drawn without running anything ends nothing
                    if let Some(started) = self.command_started.take() {
                        events.push(Event::CommandFinished {
                            duration: at.saturating_duration_since(started),
                            exit,
                        });
                    }
                }
                ScanEvent::Cwd(path) => {
                    if self.reported_cwd.as_ref() != Some(&path) {
                        events.push(Event::CwdChanged(path.clone()));
                    }
                    self.reported_cwd = Some(path);
                }
                ScanEvent::Notify { title, body } => events.push(Event::Notify { title, body }),
                ScanEvent::Progress(progress) => {
                    if progress.is_some() != self.progress.is_some() {
                        events.push(Event::BusyChanged(progress.is_some()));
                    }
                    self.progress = progress;
                }
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
        // output keeps the dirty flag set, so the view catches up once the frame is let go
        if self.holds_frame() {
            return;
        }
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
        self.last_content.refresh(&term, self.search.as_mut());
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.pty_tx.0.send(Msg::Shutdown).ok();
    }
}

#[cfg(test)]
impl Terminal {
    /// type `text` like the user at the keyboard
    pub(crate) fn type_text(&mut self, text: &[u8]) -> bool {
        self.input(InputSource::Keyboard, text.to_vec())
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

    /// a bare alacritty term `columns` by `lines` after it parsed `bytes`
    pub(crate) fn parsed_term(
        columns: usize,
        lines: usize,
        bytes: impl AsRef<[u8]>,
    ) -> alacritty_terminal::Term<alacritty_terminal::event::VoidListener> {
        use alacritty_terminal::{
            Term,
            event::VoidListener,
            term::{Config, test::TermSize},
            vte::ansi::Processor,
        };
        let size = TermSize::new(columns, lines);
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, bytes.as_ref());
        term
    }

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
        let mut builder = TerminalBuilder::new(settings, profile, TerminalBounds::default(), 0)
            .expect("failed to spawn shell");
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

    #[cfg(target_os = "linux")]
    #[test]
    fn shells_start_with_no_signal_blocked() {
        // ui threads block job control signals, the shell must not inherit that
        let blocked = std::thread::spawn(|| {
            unsafe {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGTSTP);
                libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            }
            let profile = profile(Shell::WithArguments {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "grep SigBlk /proc/self/status | tr -d ' \\t'; sleep 5".into(),
                ],
            });
            let mut builder = spawn_with(&TerminalSettings::default(), &profile);
            wait_for_text(&mut builder.terminal, "SigBlk:0000000000000000");
        });
        blocked.join().unwrap();
    }

    #[test]
    fn shell_runs_input() {
        let mut builder = spawn(&TerminalSettings::default());
        builder.terminal.type_text(b"echo out_$((6*7))\r");
        wait_for_text(&mut builder.terminal, "out_42");
    }

    #[test]
    fn term_program_version_is_own() {
        let mut builder = spawn(&TerminalSettings::default());
        builder
            .terminal
            .type_text(b"echo ver=$TERM_PROGRAM_VERSION=\r");
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
        terminal.type_text(b"\r");
        wait_for_text(terminal, "blink_on");
        assert!(terminal.last_content.cursor_blinking);
        terminal.type_text(b"\r");
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
        builder.terminal.type_text(b"echo size=$(stty size)\r");
        wait_for_text(&mut builder.terminal, "size=25 100");
    }

    #[test]
    fn exit_emits_exit_event() {
        let mut builder = spawn(&TerminalSettings::default());
        builder.terminal.type_text(b"exit\r");
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
        builder.terminal.type_text(b"cd /tmp && sleep 5\r");
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
    pub(super) fn spawn_long_output(settings: &TerminalSettings) -> TerminalBuilder {
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
        builder.terminal.type_text(b"\r");
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
        // a clean sync must leave the snapshot alone
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
        assert!(terminal.try_keystroke(
            &ctrl_i,
            super::KeyEventKind::Press,
            false,
            super::InputSource::Keyboard
        ));
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
        terminal.type_text(b"\r");
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

    #[test]
    fn clear_drops_history_and_keeps_the_cursor_line() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "seq 1 60; printf 'keep_me'; sleep 5".into()],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "keep_me");
        assert!(terminal.history_size() > 0);
        terminal.clear();
        terminal.sync();
        assert_eq!(terminal.history_size(), 0);
        let text = screen_text(terminal);
        // the line with the cursor, like a prompt and what is typed, moved to the top
        assert_eq!(
            text.lines().next().map(str::trim_end),
            Some("keep_me"),
            "{text}"
        );
        assert!(!text.contains("60"), "{text}");
    }

    #[test]
    fn clear_leaves_full_screen_programs_alone() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf '\\033[?1049h\\033[5;1Hin_alt'; sleep 5".into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "in_alt");
        let before = screen_text(terminal);
        terminal.clear();
        terminal.sync();
        assert_eq!(screen_text(terminal), before);
    }
}
