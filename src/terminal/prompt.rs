//! the command line being typed at the prompt, and text drawn over the grid next to it that
//! never reaches the pty: ghost text after the cursor and a hint line under the prompt

use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Point as AlacPoint},
};
use gpui::Hsla;

use super::{Terminal, blocks::visible_text, integration::mark_cells, scanner::PROMPT_MARK};

/// text drawn right after the cursor, render only
#[derive(Clone, Debug, PartialEq)]
pub struct GhostText {
    pub text: String,
    pub color: Hsla,
}

/// text drawn on the line under the prompt, render only
pub type HintLine = GhostText;

/// cell after `point`, wrapping to the next line, none past the end of the grid
fn after(point: AlacPoint, last_column: Column, bottom: i32) -> Option<AlacPoint> {
    if point.column < last_column {
        Some(AlacPoint::new(point.line, point.column + 1))
    } else if point.line.0 < bottom {
        Some(AlacPoint::new(point.line + 1, Column(0)))
    } else {
        None
    }
}

/// cell before `point`, wrapping to the end of the line above
fn before(point: AlacPoint, last_column: Column) -> Option<AlacPoint> {
    if point.column.0 > 0 {
        Some(AlacPoint::new(point.line, point.column - 1))
    } else {
        Some(AlacPoint::new(point.line - 1, last_column))
    }
}

impl Terminal {
    /// true while the shell shows its prompt, from shell integration marks
    pub fn at_prompt(&self) -> bool {
        self.at_prompt
    }

    /// what is typed after the prompt up to the cursor, none while no prompt is marked
    pub fn prompt_text(&self) -> Option<String> {
        let mark = self.prompt_mark.filter(|_| self.at_prompt)?;
        let term = self.term.lock_unfair();
        let (_, end) = mark_cells(&term, &format!("{PROMPT_MARK}{mark}"))?;
        let cursor = term.grid().cursor.point;
        let last_column = term.last_column();
        let Some(start) = after(end, last_column, term.bottommost_line().0) else {
            return Some(String::new());
        };
        let Some(stop) = before(cursor, last_column).filter(|stop| *stop >= start) else {
            return Some(String::new());
        };
        let (text, _) = visible_text(&term, start, stop);
        Some(text)
    }

    /// show `ghost` after the cursor until the next command starts, none takes it down
    #[allow(dead_code)]
    pub fn set_ghost_text(&mut self, ghost: Option<GhostText>) {
        if self.ghost_text != ghost {
            self.ghost_text = ghost;
            self.dirty.store(true, std::sync::atomic::Ordering::Release);
        }
    }

    /// show `hint` under the prompt line until the next command starts, none takes it down
    #[allow(dead_code)]
    pub fn set_hint_line(&mut self, hint: Option<HintLine>) {
        if self.hint_line != hint {
            self.hint_line = hint;
            self.dirty.store(true, std::sync::atomic::Ordering::Release);
        }
    }

    /// text drawn after the cursor
    pub fn ghost_text(&self) -> Option<&GhostText> {
        self.ghost_text.as_ref()
    }

    /// text drawn under the prompt line
    pub fn hint_line(&self) -> Option<&HintLine> {
        self.hint_line.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::{
        settings::{Shell, TerminalSettings},
        terminal::{
            tests::{profile, spawn_with, wait_for_text},
            with_shell_integration,
        },
    };

    #[test]
    fn typed_line_is_read_back_without_enter() {
        let bash = with_shell_integration(&profile(Shell::Program("/bin/bash".into())));
        let mut builder = spawn_with(&TerminalSettings::default(), &bash);
        let terminal = &mut builder.terminal;
        terminal.input(
            crate::terminal::InputSource::Keyboard,
            b"echo ready_$((1+1))\r".to_vec(),
        );
        wait_for_text(terminal, "ready_2");
        terminal.input(
            crate::terminal::InputSource::Keyboard,
            b"echo half typed".to_vec(),
        );
        wait_for_text(terminal, "half typed");
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            terminal.sync();
            terminal.handle_scan_events();
            if terminal.prompt_text().as_deref() == Some("echo half typed") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "prompt text: {:?}",
                terminal.prompt_text()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(terminal.at_prompt());
        // a running command has no prompt to read
        terminal.input(
            crate::terminal::InputSource::Keyboard,
            b"; sleep 3\r".to_vec(),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        while terminal.prompt_text().is_some() {
            terminal.sync();
            terminal.handle_scan_events();
            assert!(Instant::now() < deadline, "prompt stayed");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn ghost_text_and_hint_line_go_away_when_a_command_starts() {
        let script = concat!(
            "printf '\\033]133;A\\007$ \\033]133;B\\007'; read x; ",
            "printf '\\033]133;C\\007started\\n'; sleep 5"
        );
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "$");
        let ghost = super::GhostText {
            text: "next".into(),
            color: gpui::black(),
        };
        terminal.set_ghost_text(Some(ghost.clone()));
        terminal.set_hint_line(Some(ghost.clone()));
        assert_eq!(terminal.ghost_text(), Some(&ghost));
        terminal.input(crate::terminal::InputSource::Keyboard, b"\r".to_vec());
        wait_for_text(terminal, "started");
        terminal.handle_scan_events();
        assert_eq!(terminal.ghost_text(), None);
        assert_eq!(terminal.hint_line(), None);
    }
}
