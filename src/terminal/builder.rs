//! spawning shell and pumping alacritty's events into terminal

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use alacritty_terminal::{
    Term,
    event::{Event as AlacTermEvent, EventListener},
    event_loop::{EventLoop, Notifier},
    sync::FairMutex,
    term::{Config, Osc52},
    tty,
    vte::ansi::CursorStyle,
};
use anyhow::{Context as _, Result};
use futures::{
    FutureExt, StreamExt,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
};
use gpui::{Context, Task};

use super::{Content, Event, Terminal, TerminalBounds};
use crate::settings::{Profile, Shell, TerminalSettings};

#[derive(Clone)]
pub(super) struct ZedListener {
    events: UnboundedSender<AlacTermEvent>,
    /// shared with `Terminal`, lets `sync` skip snapshots while the grid is unchanged
    dirty: Arc<AtomicBool>,
}

impl EventListener for ZedListener {
    fn send_event(&self, event: AlacTermEvent) {
        // set before the event is queued, so the redraw it triggers always sees the flag
        if matches!(event, AlacTermEvent::Wakeup) {
            self.dirty.store(true, Ordering::Release);
        }
        self.events.unbounded_send(event).ok();
    }
}

/// alacritty options from settings
pub(super) fn term_config(settings: &TerminalSettings) -> Config {
    Config {
        default_cursor_style: CursorStyle {
            shape: settings.cursor_shape.into(),
            blinking: false,
        },
        scrolling_history: match settings.max_history_length {
            0 => crate::settings::MAX_HISTORY_LENGTH,
            lines => lines,
        },
        // a block tells the vi mode cursor apart from the shell's one
        vi_mode_cursor_style: Some(CursorStyle {
            shape: alacritty_terminal::vte::ansi::CursorShape::Block,
            blinking: false,
        }),
        semantic_escape_chars: settings.word_separators.clone(),
        // `clipboard` settings decide in `process_event`, so asking is possible
        osc52: Osc52::CopyPaste,
        // programs turn it on themselves with `CSI > flags u`
        kitty_keyboard: true,
    }
}

/// no signal blocked on this thread until dropped. the shell inherits the mask of the thread
/// forking it, and threads of the ui block job control signals, so ctrl+z would never stop a
/// program in the tab
#[cfg(any(target_os = "linux", target_os = "macos"))]
struct EmptySignalMask(libc::sigset_t);

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl EmptySignalMask {
    fn new() -> Self {
        // SAFETY: both sets are plain c structs filled by the calls before being read
        unsafe {
            let mut empty: libc::sigset_t = std::mem::zeroed();
            let mut old: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut empty);
            libc::pthread_sigmask(libc::SIG_SETMASK, &empty, &mut old);
            Self(old)
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Drop for EmptySignalMask {
    fn drop(&mut self) {
        // SAFETY: the set was filled by pthread_sigmask in new
        unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &self.0, std::ptr::null_mut());
        }
    }
}

// pane ids are unique in the app, `kuterm msg --pane` finds a pane by it
static NEXT_PANE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub struct TerminalBuilder {
    pub(crate) terminal: Terminal,
    pub(super) events_rx: UnboundedReceiver<AlacTermEvent>,
}

impl TerminalBuilder {
    /// terminal without its event pump, the pty thread would wake gpui's test scheduler
    #[cfg(test)]
    pub(crate) fn into_terminal(self) -> Terminal {
        self.terminal
    }

    /// spawn the profile command in a new pty of `bounds`
    pub fn new(
        settings: &TerminalSettings,
        profile: &Profile,
        bounds: TerminalBounds,
        window_id: u64,
    ) -> Result<TerminalBuilder> {
        let mut env = HashMap::new();
        if std::env::var("LANG").is_err() {
            env.insert("LANG".to_string(), "en_US.UTF-8".to_string());
        }
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("TERM_PROGRAM".to_string(), "kuterm".to_string());
        env.insert(
            "TERM_PROGRAM_VERSION".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        );
        // `kuterm msg --pane $KUTERM_PANE` and kout reach the pane they run in
        let pane_id = NEXT_PANE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        env.insert("KUTERM_PANE".to_string(), pane_id.to_string());
        if let Ok(exe) = std::env::current_exe() {
            env.insert("KUTERM_BIN".to_string(), exe.to_string_lossy().into_owned());
        }
        if profile.incognito {
            // the shell integration scripts keep no history when they see it
            env.insert("KUTERM_INCOGNITO".to_string(), "1".to_string());
        }
        // profile values go last, so they can override ours
        env.extend(profile.env.clone());

        let working_directory = profile.working_directory.as_ref().map(|dir| {
            match (dir.strip_prefix("~"), std::env::var_os("HOME")) {
                (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
                _ => dir.clone(),
            }
        });
        let command = match &profile.sandbox {
            Some(sandbox) => {
                let cwd = match &working_directory {
                    Some(dir) => dir.clone(),
                    None => std::env::current_dir().context("no folder to sandbox")?,
                };
                // shell integration and kuterm itself stay readable under a private /tmp
                let keep: Vec<PathBuf> = crate::settings::state_dir()
                    .into_iter()
                    .chain(
                        std::env::current_exe()
                            .ok()
                            .and_then(|exe| Some(exe.parent()?.to_path_buf())),
                    )
                    .collect();
                super::sandbox::sandboxed(&profile.command, sandbox, &cwd, &keep)
                    .map_err(anyhow::Error::msg)?
            }
            None => profile.command.clone(),
        };

        let shell = match &command {
            Shell::System => None,
            Shell::Program(program) => Some(tty::Shell::new(program.clone(), Vec::new())),
            Shell::WithArguments { program, args } => {
                Some(tty::Shell::new(program.clone(), args.clone()))
            }
        };
        let pty_options = tty::Options {
            shell,
            working_directory,
            drain_on_exit: true,
            env,
            ..Default::default()
        };

        let config = term_config(settings);

        // alacritty's event loop talks to us with that channel
        let (events_tx, events_rx) = unbounded();
        let dirty = Arc::new(AtomicBool::new(true));
        let listener = ZedListener {
            events: events_tx,
            dirty: dirty.clone(),
        };
        let term = Term::new(config, &bounds, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        let pty = {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            let _mask = EmptySignalMask::new();
            tty::new(&pty_options, bounds.into(), window_id).context("failed to open pty")?
        };
        let scan_events = super::scanner::ScanEvents::default();
        // settings may change these right after, the bundled ones hold until then
        let defaults = crate::settings::Settings::default().graphics;
        let graphics = Arc::new(std::sync::Mutex::new(super::graphics::Graphics::new(
            defaults.enable,
            defaults.max_memory_mb,
        )));
        let cell_size = Arc::new(super::graphics::CellSize::default());
        #[cfg(unix)]
        let pty = super::scanner::ScannedPty::new(
            pty,
            scan_events.clone(),
            super::graphics::GraphicsParser::new(graphics.clone(), cell_size.clone()),
        )
        .context("failed to open pty")?;
        #[cfg(unix)]
        let shell_pid = pty.child_id();
        #[cfg(windows)]
        let shell_pid = pty.child_watcher().pid().map_or(0, |pid| pid.get());

        let event_loop = EventLoop::new(
            term.clone(),
            listener,
            pty,
            pty_options.drain_on_exit,
            false,
        )
        .context("failed to create event loop")?;
        let pty_tx = event_loop.channel();
        let _io_thread = event_loop.spawn();

        // the pty already has this size, so a prepaint with it sends no resize
        let last_content = Content {
            terminal_bounds: bounds,
            ..Content::default()
        };
        let terminal = Terminal {
            pty_tx: Notifier(pty_tx),
            term,
            events: Vec::new(),
            last_content,
            dirty,
            title: String::new(),
            shell_pid,
            pane_id,
            incognito: profile.incognito,
            sandboxed: profile.sandbox.is_some(),
            theme_settings: profile.theme.clone(),
            theme: None,
            clipboard: settings.clipboard,
            clipboard_requests: HashMap::new(),
            next_clipboard_request: 0,
            search: None,
            vi: Default::default(),
            scan_events,
            reported_cwd: None,
            progress: None,
            command_started: None,
            inputs: 0,
            last_command: None,
            frozen: None,
            blocks: Default::default(),
            at_prompt: false,
            prompt_mark: None,
            saw_prompt_end: false,
            hint_line: None,
            queue: Default::default(),
            queue_stop_on_failure: settings.queue_stop_on_failure,
            pty_state: Default::default(),
            decorations: Default::default(),
            xoff_at: None,
            send_xoff: settings.send_xoff,
            last_output: std::time::Instant::now(),
            typed_sources: Default::default(),
            graphics,
            cell_size,
            #[cfg(unix)]
            completion: None,
            _event_loop_task: Task::ready(()),
            _completion_task: Task::ready(()),
        };

        Ok(TerminalBuilder {
            terminal,
            events_rx,
        })
    }

    /// start pumping alacritty events into terminal entity
    pub fn subscribe(mut self, cx: &Context<Terminal>) -> Terminal {
        #[cfg(unix)]
        if let Some(mut shell_events) = self
            .terminal
            .completion
            .as_mut()
            .and_then(|bridge| bridge.take_events())
        {
            self.terminal._completion_task = cx.spawn(async move |terminal, cx| {
                while let Some(event) = shell_events.next().await {
                    let emitted = terminal.update(cx, |_, cx| cx.emit(Event::Shell(event)));
                    if emitted.is_err() {
                        break;
                    }
                }
            });
        }
        self.terminal._event_loop_task = cx.spawn(async move |terminal, cx| {
            while let Some(event) = self.events_rx.next().await {
                let Ok(()) = terminal.update(cx, |terminal, cx| {
                    // process the first event right away for lower latency
                    terminal.process_event(event, cx);
                }) else {
                    break;
                };

                // then batch rest in 4ms windows
                loop {
                    let mut events = Vec::new();
                    let mut timer = cx
                        .background_executor()
                        .timer(Duration::from_millis(4))
                        .fuse();
                    let mut wakeup = false;
                    loop {
                        futures::select_biased! {
                            _ = timer => break,
                            event = self.events_rx.next() => {
                                let Some(event) = event else { break };
                                if matches!(event, AlacTermEvent::Wakeup) {
                                    wakeup = true;
                                } else {
                                    events.push(event);
                                }
                                if events.len() > 100 {
                                    break;
                                }
                            },
                        }
                    }

                    if events.is_empty() && !wakeup {
                        break;
                    }

                    let Ok(()) = terminal.update(cx, |this, cx| {
                        if wakeup {
                            this.process_event(AlacTermEvent::Wakeup, cx);
                        }
                        for event in events {
                            this.process_event(event, cx);
                        }
                    }) else {
                        return;
                    };
                }
            }
        });
        self.terminal
    }
}
