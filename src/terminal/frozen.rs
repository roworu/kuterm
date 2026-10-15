//! the frame a mouse drag started on, held while the button is down so text doesn't move away
//! under the mouse. the pty is still read, the view catches up on release

use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use alacritty_terminal::{
    Term,
    event::VoidListener,
    grid::Dimensions,
    index::{Line, Point as AlacPoint, Side},
    selection::{Selection, SelectionRange},
    term::Config,
};

use super::{Terminal, builder::ZedListener};

// a held frame lets go on its own after this, in case the release is never seen
const MAX_HOLD: Duration = Duration::from_secs(30);

/// the frame shown when the drag started, as a term of its own so selections work on it
pub(super) struct Frozen {
    /// the visible lines, line 0 is the top row
    pub(super) term: Term<VoidListener>,
    since: Instant,
    /// display offset and history size when the frame was taken
    display_offset: usize,
    history_size: usize,
    /// where the selection on the frame started
    pub(super) anchor: Option<(AlacPoint, Side)>,
}

impl Frozen {
    /// selection on the frame in grid coordinates of the shown content
    fn shown_selection(&self) -> Option<SelectionRange> {
        let range = self.term.selection.as_ref()?.to_range(&self.term)?;
        let shift = |point: AlacPoint| {
            AlacPoint::new(point.line - self.display_offset as i32, point.column)
        };
        Some(SelectionRange::new(
            shift(range.start),
            shift(range.end),
            range.is_block,
        ))
    }
}

impl Frozen {
    /// the selection moved onto the live grid, none when its text is not where it moved to
    fn live_selection(&self, live: &mut Term<ZedListener>) -> Option<Selection> {
        let selection = self.term.selection.as_ref()?;
        // output pushed lines into history, the shown text moved up with them
        let moved = live.grid().history_size() as i32 - self.history_size as i32;
        let shift = self.display_offset as i32 + moved;
        let point = |p: AlacPoint| AlacPoint::new(Line(p.line.0 - shift), p.column);
        let Some(range) = selection.to_range(&self.term) else {
            // a click selects nothing yet, its point stays the anchor a shift+click extends
            let (anchor, side) = self.anchor?;
            let anchor = point(anchor);
            return (anchor.line >= live.topmost_line())
                .then(|| Selection::new(selection.ty, anchor, side));
        };
        let text = self.term.selection_to_string()?;
        let (start, end) = (point(range.start), point(range.end));
        if start.line < live.topmost_line() {
            return None;
        }
        // the same kind, so line and word selections expand over the same text again
        let mut moved = Selection::new(selection.ty, start, Side::Left);
        moved.update(end, Side::Right);
        live.selection = Some(moved.clone());
        (live.selection_to_string().as_deref() == Some(text.as_str())).then_some(moved)
    }
}

impl Terminal {
    /// keep showing the current frame until `unfreeze`, the selection then works on it
    pub fn freeze(&mut self) {
        let content = &self.last_content;
        let bounds = content.terminal_bounds;
        // words end where the live terminal's separators say
        let config = Config {
            semantic_escape_chars: self.term.lock_unfair().semantic_escape_chars().to_string(),
            ..Config::default()
        };
        let mut term = Term::new(config, &bounds, VoidListener);
        for indexed in &content.cells {
            let row = indexed.point.line.0 + content.display_offset as i32;
            if (0..term.screen_lines() as i32).contains(&row)
                && indexed.point.column.0 < term.columns()
            {
                term.grid_mut()[Line(row)][indexed.point.column] = indexed.cell.clone();
            }
        }
        self.frozen = Some(Frozen {
            term,
            since: Instant::now(),
            display_offset: content.display_offset,
            history_size: content.history_size,
            anchor: None,
        });
    }

    /// show the live grid again. the selection moves to where its text went, or is dropped
    /// when that text is no longer there
    pub fn unfreeze(&mut self) {
        let Some(frozen) = self.frozen.take() else {
            return;
        };
        let mut term = self.term.lock();
        term.selection = frozen.live_selection(&mut term);
        drop(term);
        self.dirty.store(true, Ordering::Release);
    }

    /// true while a frame is held, letting it go once it was held too long
    pub(super) fn holds_frame(&mut self) -> bool {
        if self
            .frozen
            .as_ref()
            .is_some_and(|frozen| frozen.since.elapsed() > MAX_HOLD)
        {
            self.unfreeze();
        }
        self.frozen.is_some()
    }

    /// show the selection made on the held frame
    pub(super) fn show_frozen_selection(&mut self) {
        if let Some(frozen) = &self.frozen {
            self.last_content.selection = frozen.shown_selection();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use alacritty_terminal::selection::SelectionType;
    use gpui::{Pixels, Point, point, px};

    use crate::{
        settings::{Shell, TerminalSettings},
        terminal::tests::{profile, screen_text, spawn_with, wait_for_text},
    };

    // spawn's grid starts at the window origin with 10px cells and 20px lines
    fn cell(line: usize, column: usize, right: bool) -> Point<Pixels> {
        let x = column as f32 * 10. + if right { 8. } else { 2. };
        point(px(x), px(line as f32 * 20. + 10.))
    }

    #[test]
    fn held_frame_stays_until_let_go_and_keeps_the_selected_text() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 'one\\ntwo\\n'; sleep 0.5; seq 1 60; sleep 5".into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "two");
        let shown = screen_text(terminal);

        terminal.freeze();
        terminal.start_selection(cell(0, 0, false), SelectionType::Simple);
        terminal.extend_selection(cell(0, 2, true));
        // the output scrolls "one" away while the frame is held
        let deadline = Instant::now() + Duration::from_secs(15);
        while terminal.history_size() == 0 {
            assert!(Instant::now() < deadline, "no output");
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(100));
        terminal.sync();
        assert_eq!(screen_text(terminal), shown);
        assert_eq!(terminal.selection_text().as_deref(), Some("one"));
        assert!(terminal.last_content.selection.is_some());

        // let go, the view catches up and the selection follows its text into history
        terminal.unfreeze();
        terminal.sync();
        assert!(screen_text(terminal).contains("60"));
        assert_eq!(terminal.selection_text().as_deref(), Some("one"));

        // a line selection, like a triple click, stays one after letting go
        terminal.freeze();
        terminal.start_selection(cell(5, 0, false), SelectionType::Lines);
        let held = terminal.selection_text();
        terminal.unfreeze();
        assert!(
            held.as_deref().is_some_and(|text| text.ends_with('\n')),
            "{held:?}"
        );
        assert_eq!(terminal.selection_text(), held);
    }
}
