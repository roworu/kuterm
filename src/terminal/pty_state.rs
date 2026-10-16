//! pty states that make a pane look stuck: a program waiting for an answer or a secret,
//! output paused with ctrl+s, and programs stopped with ctrl+z

use std::time::{Duration, Instant};

use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Point as AlacPoint},
};
use regex::Regex;

use super::{PtyModes, StoppedJob, Terminal, blocks::visible_text};

// a running command quiet this long, its last line a question, waits for an answer
pub const QUESTION_QUIET: Duration = Duration::from_secs(2);
// output read before ctrl+s took hold still arrives this long after it
pub(super) const XOFF_GRACE: Duration = Duration::from_millis(300);

/// what the workspace read about a pane's pty on the last title refresh
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PtyState {
    pub modes: Option<PtyModes>,
    /// the shell itself is in front, no program runs
    pub shell_in_front: bool,
    pub stopped: Vec<StoppedJob>,
}

/// a question a program waits on
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    /// the line asking it
    pub line: String,
    /// echo is off, the program reads a secret
    pub secret: bool,
    /// it asks yes or no, like "[Y/n]"
    pub yes_no: bool,
}

/// true when `line` reads like a prompt waiting for an answer
pub fn asks(line: &str, patterns: &[Regex]) -> bool {
    let line = line.trim_end();
    !line.is_empty() && patterns.iter().any(|pattern| pattern.is_match(line))
}

/// true when `line` asks yes or no
pub fn asks_yes_no(line: &str) -> bool {
    let lower = line.to_lowercase();
    ["[y/n]", "(y/n)", "[yes/no]", "(yes/no)"]
        .iter()
        .any(|choice| lower.contains(choice))
}

impl Terminal {
    /// take what was read about the pty off the main thread, true when it changed
    pub fn set_pty_state(&mut self, state: PtyState) -> bool {
        if self.pty_state == state {
            return false;
        }
        // ctrl+q typed elsewhere, like by `stty`, also lets output go again
        if !state.modes.is_some_and(|modes| modes.ixon) {
            self.xoff_at = None;
        }
        self.pty_state = state;
        true
    }

    /// tty settings of the pty from the last refresh
    pub fn pty_modes(&self) -> Option<PtyModes> {
        self.pty_state.modes
    }

    /// programs of the shell stopped with ctrl+z, from the last refresh
    pub fn stopped_jobs(&self) -> &[StoppedJob] {
        &self.pty_state.stopped
    }

    /// a program reads a secret, like sudo or ssh asking for a password: echo is off and
    /// input comes in lines, which full screen programs never ask for
    pub fn asking_secret(&self) -> bool {
        !self.pty_state.shell_in_front
            && self
                .pty_state
                .modes
                .is_some_and(|modes| !modes.echo && modes.icanon)
    }

    /// output was paused with ctrl+s and nothing resumed it yet
    pub fn xoff_paused(&self) -> bool {
        self.xoff_at.is_some()
    }

    /// true while a command the shell marked runs
    pub fn command_running(&self) -> bool {
        self.command_started.is_some()
    }

    /// the question a program waits on: a secret prompt right away, or a running command quiet
    /// for a while whose cursor line matches `patterns`
    pub fn question(&self, patterns: &[Regex], now: Instant) -> Option<Question> {
        let secret = self.asking_secret();
        let quiet = now.saturating_duration_since(self.last_output) >= QUESTION_QUIET;
        if !secret && !(self.command_running() && quiet) {
            return None;
        }
        let line = self.cursor_line_text();
        if !secret && !asks(&line, patterns) {
            return None;
        }
        Some(Question {
            yes_no: !secret && asks_yes_no(&line),
            line: line.trim().to_string(),
            secret,
        })
    }

    /// text of the line the cursor is on, up to the cursor
    fn cursor_line_text(&self) -> String {
        let term = self.term.lock_unfair();
        let cursor = term.grid().cursor.point;
        let end = AlacPoint::new(cursor.line, term.last_column());
        let (text, _) = visible_text(&term, AlacPoint::new(cursor.line, Column(0)), end);
        text
    }

    /// note output paused by ctrl+s, or resumed by ctrl+q, when the pty pauses on them.
    /// none drops the keys: ctrl+s with `send_xoff` off while the pty would pause
    pub(super) fn xoff_filter(&mut self, input: &[u8]) -> Option<()> {
        let ixon = self.pty_state.modes.is_some_and(|modes| modes.ixon);
        if input.contains(&0x11) {
            self.xoff_at = None;
        }
        if input == [0x13] && ixon {
            if !self.send_xoff {
                return None;
            }
            self.xoff_at = Some(Instant::now());
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use regex::Regex;

    use super::{PtyState, asks, asks_yes_no};
    use crate::{
        settings::TerminalSettings,
        terminal::{InputSource, PtyModes, tests::spawn},
    };

    fn patterns() -> Vec<Regex> {
        [
            r"\?$",
            r"\[[^\]]*\]\s*:?$",
            r"(?i)password[^:]*:$",
            r"(?i)press any key",
        ]
        .iter()
        .map(|pattern| Regex::new(pattern).unwrap())
        .collect()
    }

    #[test]
    fn question_lines_are_told_apart() {
        let patterns = patterns();
        assert!(asks("continue? [y/N] ", &patterns));
        assert!(asks("Do you want to continue? ", &patterns));
        assert!(asks("[sudo] password for me: ", &patterns));
        assert!(asks("Press any key to go on", &patterns));
        assert!(!asks("compiling foo v1.0", &patterns));
        assert!(!asks("   ", &patterns));
        assert!(asks_yes_no("continue? [Y/n]"));
        assert!(asks_yes_no("Proceed (y/n)?"));
        assert!(!asks_yes_no("Enter value:"));
    }

    #[test]
    fn secret_prompts_and_quiet_questions_are_questions() {
        let mut builder = spawn(&TerminalSettings::default());
        let terminal = &mut builder.terminal;
        let echo_off = PtyModes {
            echo: false,
            icanon: true,
            ixon: true,
        };
        terminal.set_pty_state(PtyState {
            modes: Some(echo_off),
            shell_in_front: false,
            stopped: Vec::new(),
        });
        assert!(terminal.asking_secret());
        let question = terminal.question(&patterns(), Instant::now()).unwrap();
        assert!(question.secret && !question.yes_no);
        // the shell itself with echo off is no secret prompt, like zsh's line editor
        terminal.set_pty_state(PtyState {
            modes: Some(echo_off),
            shell_in_front: true,
            stopped: Vec::new(),
        });
        assert!(!terminal.asking_secret());
        assert_eq!(terminal.question(&patterns(), Instant::now()), None);
    }

    #[test]
    fn secret_prompts_refuse_automation_but_not_the_keyboard() {
        let mut builder = spawn(&TerminalSettings::default());
        let terminal = &mut builder.terminal;
        terminal.set_pty_state(PtyState {
            modes: Some(PtyModes {
                echo: false,
                icanon: true,
                ixon: false,
            }),
            shell_in_front: false,
            stopped: Vec::new(),
        });
        for source in [
            InputSource::Macro,
            InputSource::Broadcast,
            InputSource::Remote,
            InputSource::Automation,
            InputSource::Mcp,
        ] {
            assert!(!terminal.input(source, b"x".to_vec()), "{source:?}");
        }
        assert!(terminal.input(InputSource::Keyboard, b"x".to_vec()));
        assert!(terminal.input(InputSource::Paste, b"x".to_vec()));
        terminal.set_pty_state(PtyState::default());
        assert!(terminal.input(InputSource::Remote, b"x".to_vec()));
    }

    #[test]
    fn ctrl_s_pauses_until_ctrl_q_or_is_dropped() {
        let mut builder = spawn(&TerminalSettings::default());
        let terminal = &mut builder.terminal;
        let ixon = PtyState {
            modes: Some(PtyModes {
                echo: true,
                icanon: true,
                ixon: true,
            }),
            ..PtyState::default()
        };
        terminal.set_pty_state(ixon.clone());
        assert!(terminal.input(InputSource::Keyboard, b"\x13".to_vec()));
        assert!(terminal.xoff_paused());
        assert!(terminal.input(InputSource::Keyboard, b"\x11".to_vec()));
        assert!(!terminal.xoff_paused());
        // turning ixon off lets output go, nothing can pause it now
        terminal.input(InputSource::Keyboard, b"\x13".to_vec());
        terminal.set_pty_state(PtyState::default());
        assert!(!terminal.xoff_paused());
        // with send_xoff off ctrl+s never reaches a pty that would pause
        terminal.send_xoff = false;
        terminal.set_pty_state(ixon);
        assert!(!terminal.input(InputSource::Keyboard, b"\x13".to_vec()));
        assert!(!terminal.xoff_paused());
        terminal.set_pty_state(PtyState::default());
        assert!(terminal.input(InputSource::Keyboard, b"\x13".to_vec()));
    }

    #[test]
    fn a_quiet_command_asking_is_a_question() {
        use crate::{
            settings::Shell,
            terminal::tests::{profile, spawn_with, wait_for_text},
        };
        let script =
            "printf '\\033]133;C\\007'; printf 'continue? [y/N] '; read x; echo got_$x; sleep 5";
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "continue?");
        terminal.handle_scan_events();
        assert!(terminal.command_running());
        // not quiet long enough yet
        terminal.last_output = Instant::now();
        assert_eq!(terminal.question(&patterns(), Instant::now()), None);
        let later = Instant::now() + Duration::from_secs(3);
        let question = terminal.question(&patterns(), later).unwrap();
        assert_eq!(question.line, "continue? [y/N]");
        assert!(question.yes_no && !question.secret);
        terminal.input(InputSource::Keyboard, b"y\r".to_vec());
        wait_for_text(terminal, "got_y");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_real_password_prompt_turns_echo_off() {
        use crate::{
            settings::Shell,
            terminal::{
                foreground_process, pty_modes,
                tests::{profile, spawn_with, wait_for_text},
            },
        };
        let script = "python3 -c \"import getpass; getpass.getpass('Pw: ')\"; sleep 5";
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "Pw:");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let front = foreground_process(terminal.shell_pid).unwrap();
            let modes = pty_modes(front.pid);
            if front.name.starts_with("python") && modes.is_some_and(|m| !m.echo && m.icanon) {
                break;
            }
            assert!(Instant::now() < deadline, "{front:?} {modes:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
