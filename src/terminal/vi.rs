//! keyboard selection mode: alacritty's vi mode moves a cursor of its own over the history

use std::sync::atomic::Ordering;

use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    index::{Column, Point as AlacPoint, Side},
    selection::{Selection, SelectionType},
    term::TermMode,
    vi_mode::ViMotion,
};
use gpui::Keystroke;

use super::Terminal;

// a typed count past this is a mistake, and huge counts would spin for a long time
const MAX_COUNT: usize = 10_000;

/// what a key did in vi mode, for the view to finish
#[derive(Debug, PartialEq)]
pub enum ViKey {
    /// the key moved, selected or did nothing
    Handled,
    /// vi mode ended
    Exit,
    /// `y` copied this text and ended vi mode
    Copy(String),
    /// `/` asks for the search bar
    Search,
}

/// typed count and the `g` waiting for its second key
#[derive(Default)]
pub(super) struct ViState {
    count: usize,
    pending_g: bool,
}

impl Terminal {
    /// start or end vi mode, ending it drops its selection
    pub fn toggle_vi_mode(&mut self) {
        let mut term = self.term.lock();
        term.toggle_vi_mode();
        if !term.mode().contains(TermMode::VI) {
            term.selection = None;
        }
        drop(term);
        self.vi = Default::default();
        self.dirty.store(true, Ordering::Release);
    }

    /// true while vi mode is on
    #[cfg(test)]
    pub(crate) fn vi_mode(&self) -> bool {
        self.term.lock_unfair().mode().contains(TermMode::VI)
    }

    /// run a key in vi mode
    pub fn vi_key(&mut self, keystroke: &Keystroke) -> ViKey {
        let m = &keystroke.modifiers;
        // vi keys tell upper case apart, like `w` and `W`
        let key = match keystroke.key_char.as_deref() {
            Some(c) if !m.control && !m.alt && !m.platform => c.to_string(),
            _ => keystroke.key.clone(),
        };
        let count = std::mem::take(&mut self.vi.count);
        let pending_g = std::mem::take(&mut self.vi.pending_g);
        let mut term = self.term.lock();
        let motion = |term: &mut alacritty_terminal::Term<_>, motion: ViMotion, times: usize| {
            for _ in 0..times.max(1) {
                term.vi_motion(motion);
            }
            let point = term.vi_mode_cursor.point;
            term.scroll_to_point(point);
        };
        let lines = term.screen_lines();
        let result = match (key.as_str(), m.control) {
            ("escape", _) | ("i", false) => {
                drop(term);
                self.toggle_vi_mode();
                return ViKey::Exit;
            }
            (digit, false)
                if digit.len() == 1
                    && digit.as_bytes()[0].is_ascii_digit()
                    && (count > 0 || digit != "0") =>
            {
                self.vi.count = (count * 10 + (digit.as_bytes()[0] - b'0') as usize).min(MAX_COUNT);
                ViKey::Handled
            }
            ("h" | "left", false) => {
                motion(&mut term, ViMotion::Left, count);
                ViKey::Handled
            }
            ("j" | "down", false) => {
                motion(&mut term, ViMotion::Down, count);
                ViKey::Handled
            }
            ("k" | "up", false) => {
                motion(&mut term, ViMotion::Up, count);
                ViKey::Handled
            }
            ("l" | "right", false) => {
                motion(&mut term, ViMotion::Right, count);
                ViKey::Handled
            }
            ("w", false) => {
                motion(&mut term, ViMotion::SemanticRight, count);
                ViKey::Handled
            }
            ("b", false) => {
                motion(&mut term, ViMotion::SemanticLeft, count);
                ViKey::Handled
            }
            ("e", false) => {
                motion(&mut term, ViMotion::SemanticRightEnd, count);
                ViKey::Handled
            }
            ("W", false) => {
                motion(&mut term, ViMotion::WordRight, count);
                ViKey::Handled
            }
            ("B", false) => {
                motion(&mut term, ViMotion::WordLeft, count);
                ViKey::Handled
            }
            ("E", false) => {
                motion(&mut term, ViMotion::WordRightEnd, count);
                ViKey::Handled
            }
            ("0" | "home", false) => {
                motion(&mut term, ViMotion::First, 1);
                ViKey::Handled
            }
            ("^", false) => {
                motion(&mut term, ViMotion::FirstOccupied, 1);
                ViKey::Handled
            }
            ("$" | "end", false) => {
                motion(&mut term, ViMotion::Last, 1);
                ViKey::Handled
            }
            ("H", false) => {
                motion(&mut term, ViMotion::High, 1);
                ViKey::Handled
            }
            ("M", false) => {
                motion(&mut term, ViMotion::Middle, 1);
                ViKey::Handled
            }
            ("L", false) => {
                motion(&mut term, ViMotion::Low, 1);
                ViKey::Handled
            }
            ("%", false) => {
                motion(&mut term, ViMotion::Bracket, 1);
                ViKey::Handled
            }
            ("{", false) => {
                motion(&mut term, ViMotion::ParagraphUp, count);
                ViKey::Handled
            }
            ("}", false) => {
                motion(&mut term, ViMotion::ParagraphDown, count);
                ViKey::Handled
            }
            ("g", false) if pending_g => {
                let top = AlacPoint::new(term.topmost_line(), Column(0));
                term.vi_goto_point(top);
                ViKey::Handled
            }
            ("g", false) => {
                self.vi.pending_g = true;
                ViKey::Handled
            }
            ("G", false) => {
                let bottom = AlacPoint::new(term.bottommost_line(), Column(0));
                term.vi_goto_point(bottom);
                ViKey::Handled
            }
            // half and whole pages move the view and the cursor with it
            ("u" | "d" | "b" | "f", true) => {
                let page = match key.as_str() {
                    "u" | "d" => lines / 2,
                    _ => lines,
                } as i32;
                let up = matches!(key.as_str(), "u" | "b");
                let delta = if up { page } else { -page };
                term.scroll_display(Scroll::Delta(delta));
                let mut point = term.vi_mode_cursor.point;
                point.line = (point.line - delta)
                    .max(term.topmost_line())
                    .min(term.bottommost_line());
                term.vi_goto_point(point);
                ViKey::Handled
            }
            ("v", ctrl) => {
                let ty = if ctrl {
                    SelectionType::Block
                } else {
                    SelectionType::Simple
                };
                toggle_selection(&mut term, ty);
                ViKey::Handled
            }
            ("V", false) => {
                toggle_selection(&mut term, SelectionType::Lines);
                ViKey::Handled
            }
            ("y", false) => {
                let text = term.selection_to_string().filter(|text| !text.is_empty());
                drop(term);
                self.toggle_vi_mode();
                return text.map_or(ViKey::Exit, ViKey::Copy);
            }
            ("/", false) => ViKey::Search,
            ("n" | "N", false) => {
                drop(term);
                // `n` goes on the way `/` searches, up from the bottom
                self.search_next(key == "n");
                self.dirty.store(true, Ordering::Release);
                return ViKey::Handled;
            }
            _ => ViKey::Handled,
        };
        drop(term);
        self.dirty.store(true, Ordering::Release);
        result
    }
}

// pressing the key of the running selection type ends it, another type replaces it
fn toggle_selection<T>(term: &mut alacritty_terminal::Term<T>, ty: SelectionType) {
    let point = term.vi_mode_cursor.point;
    if term
        .selection
        .as_ref()
        .is_some_and(|selection| selection.ty == ty)
    {
        term.selection = None;
        return;
    }
    let mut selection = Selection::new(ty, point, Side::Left);
    selection.include_all();
    term.selection = Some(selection);
}

#[cfg(test)]
mod tests {
    use gpui::{Keystroke, Modifiers};

    use super::ViKey;
    use crate::{
        settings::{Shell, TerminalSettings},
        terminal::tests::{profile, spawn_with, wait_for_text},
    };

    /// a typed key the way gpui reports it, with its character
    fn key(text: &str) -> Keystroke {
        let ctrl = text.strip_prefix("ctrl-");
        let key = ctrl.unwrap_or(text);
        Keystroke {
            modifiers: Modifiers {
                control: ctrl.is_some(),
                shift: key.chars().any(char::is_uppercase),
                ..Modifiers::default()
            },
            key: key.to_lowercase(),
            key_char: (ctrl.is_none() && key.chars().count() == 1).then(|| key.to_string()),
        }
    }

    #[test]
    fn vi_keys_move_select_and_copy() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 'alpha beta gamma\\ndelta epsilon\\n'; echo vi_ready; sleep 5".into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "vi_ready");
        terminal.toggle_vi_mode();
        assert!(terminal.vi_mode());
        // to the top line, then the second word, select to the end of the third
        for k in ["g", "g", "0", "w", "v", "2", "e"] {
            assert_eq!(terminal.vi_key(&key(k)), ViKey::Handled, "{k}");
        }
        assert_eq!(terminal.vi_key(&key("y")), ViKey::Copy("beta gamma".into()));
        assert!(!terminal.vi_mode());

        // line selection, then escape leaves without copying
        terminal.toggle_vi_mode();
        for k in ["g", "g", "j", "V"] {
            terminal.vi_key(&key(k));
        }
        assert_eq!(
            terminal.selection_text().as_deref(),
            Some("delta epsilon\n")
        );
        assert_eq!(terminal.vi_key(&key("escape")), ViKey::Exit);
        assert!(!terminal.vi_mode());
        assert_eq!(terminal.selection_text(), None);

        terminal.toggle_vi_mode();
        assert_eq!(terminal.vi_key(&key("/")), ViKey::Search);
        // y with nothing selected just leaves
        assert_eq!(terminal.vi_key(&key("y")), ViKey::Exit);
    }

    #[test]
    fn vi_cursor_scrolls_the_view_through_history() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "i=0; while [ $i -lt 100 ]; do echo row_$i; i=$((i+1)); done; echo vi_ready; sleep 5"
                    .into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "vi_ready");
        terminal.toggle_vi_mode();
        terminal.vi_key(&key("g"));
        terminal.vi_key(&key("g"));
        terminal.sync();
        assert_eq!(
            terminal.last_content.display_offset,
            terminal.last_content.history_size
        );
        terminal.vi_key(&key("G"));
        terminal.sync();
        assert_eq!(terminal.last_content.display_offset, 0);
        terminal.vi_key(&key("ctrl-u"));
        terminal.sync();
        assert!(terminal.last_content.display_offset > 0);
    }

    /// text `keys` select in vi mode, starting at the top left
    fn select_with(terminal: &mut crate::terminal::Terminal, keys: &[&str]) -> Option<String> {
        terminal.toggle_vi_mode();
        for k in ["g", "g", "0"].iter().chain(keys) {
            assert_eq!(terminal.vi_key(&key(k)), ViKey::Handled, "{k}");
        }
        match terminal.vi_key(&key("y")) {
            ViKey::Copy(text) => Some(text),
            _ => None,
        }
    }

    #[test]
    fn vi_motions_move_by_chars_words_lines_and_screens() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 'alpha (beta) gamma\\n  delta.x epsilon\\n\\nzeta gamma\\n'; echo vi_ready; sleep 5"
                    .into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "vi_ready");
        for (keys, want) in [
            (&["l", "l", "v", "l"][..], "ph"),
            (&["l", "l", "h", "v", "l"], "lp"),
            (&["$", "v", "h", "h"], "mma"),
            (&["end", "B", "v", "$"], "gamma"),
            // W, B and E split at blanks only, b at punctuation too
            (&["W", "v", "E"], "(beta)"),
            (&["W", "%", "v", "%"], "(beta)"),
            (&["j", "^", "v", "E"], "delta.x"),
            (&["j", "$", "b", "v", "$"], "epsilon"),
            (&["G", "H", "V"], "alpha (beta) gamma\n"),
            (&["j", "j", "j", "{", "V"], "alpha (beta) gamma\n"),
            (&["}", "j", "V"], "zeta gamma\n"),
            // pressing the running selection's key again ends it
            (&["v", "l", "v"], ""),
        ] {
            let got = select_with(terminal, keys).unwrap_or_default();
            assert_eq!(got, want, "{keys:?}");
            assert!(!terminal.vi_mode());
        }
        // M and L land on the middle and last rows of the screen, below the text here
        assert_eq!(select_with(terminal, &["M", "V"]).as_deref(), Some("\n"));
        assert_eq!(select_with(terminal, &["L", "V"]).as_deref(), Some("\n"));
        // ctrl-v selects a block, the same columns of each line
        terminal.toggle_vi_mode();
        for k in ["g", "g", "0", "ctrl-v", "j", "j", "j", "l"] {
            terminal.vi_key(&key(k));
        }
        let selection = terminal.term.lock().selection.clone().unwrap();
        assert_eq!(
            selection.ty,
            alacritty_terminal::selection::SelectionType::Block
        );
        // an unknown key does nothing and stays in vi mode
        assert_eq!(terminal.vi_key(&key("z")), ViKey::Handled);
        assert!(terminal.vi_mode());
    }

    #[test]
    fn vi_n_walks_search_matches_with_the_cursor() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 'one gamma\\ntwo\\nthree gamma\\n'; echo vi_ready; sleep 5".into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "vi_ready");
        terminal.toggle_vi_mode();
        let query = crate::terminal::SearchQuery {
            text: "gamma".into(),
            regex: false,
            whole_word: false,
            case: crate::settings::SearchCase::Smart,
            scope: Default::default(),
        };
        // the search puts the vi cursor on the lowest match, n goes up to the one above
        terminal.search(&query).unwrap();
        assert_eq!(terminal.vi_key(&key("V")), ViKey::Handled);
        assert_eq!(terminal.selection_text().as_deref(), Some("three gamma\n"));
        terminal.vi_key(&key("V"));
        assert_eq!(terminal.vi_key(&key("n")), ViKey::Handled);
        terminal.vi_key(&key("V"));
        assert_eq!(terminal.selection_text().as_deref(), Some("one gamma\n"));
        terminal.vi_key(&key("V"));
        // N goes down again
        terminal.vi_key(&key("N"));
        assert_eq!(
            terminal.vi_key(&key("y")),
            ViKey::Exit,
            "nothing selected, y just leaves"
        );
    }

    #[test]
    fn vi_whole_pages_move_the_view() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "i=0; while [ $i -lt 100 ]; do echo row_$i; i=$((i+1)); done; echo vi_ready; sleep 5"
                    .into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "vi_ready");
        terminal.toggle_vi_mode();
        terminal.vi_key(&key("ctrl-b"));
        terminal.sync();
        let lines = {
            use alacritty_terminal::grid::Dimensions;
            terminal.term.lock().screen_lines()
        };
        // a whole screen up, where ctrl-u goes half of it
        assert!(terminal.last_content.display_offset >= lines);
        terminal.vi_key(&key("ctrl-f"));
        terminal.sync();
        assert_eq!(terminal.last_content.display_offset, 0);
    }
}
