//! mouse selection over the grid, kept in alacritty's `Term` so it follows scrollback

use std::sync::atomic::Ordering;

use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line, Point as AlacPoint, Side},
    selection::{Selection, SelectionType},
    term::TermMode,
};
use gpui::{Pixels, Point};

use super::{Terminal, TerminalBounds};

/// grid cell and its half under a window position, clamped into the visible grid
fn grid_point(
    position: Point<Pixels>,
    bounds: &TerminalBounds,
    display_offset: usize,
) -> (AlacPoint, Side) {
    let relative = position - bounds.bounds.origin;
    let last_column = bounds.num_columns().saturating_sub(1);
    let last_line = bounds.num_lines().saturating_sub(1);

    let column = (relative.x / bounds.cell_width).max(0.) as usize;
    let side = if column > last_column {
        // right of the last column still selects up to the line end
        Side::Right
    } else {
        let x_in_cell = relative.x - bounds.cell_width * column as f32;
        if x_in_cell > bounds.cell_width / 2. {
            Side::Right
        } else {
            Side::Left
        }
    };
    let line = ((relative.y / bounds.line_height).max(0.) as usize).min(last_line);

    let point = AlacPoint::new(
        Line(line as i32 - display_offset as i32),
        Column(column.min(last_column)),
    );
    (point, side)
}

impl Terminal {
    fn mouse_point(&self, position: Point<Pixels>) -> (AlacPoint, Side) {
        let content = &self.last_content;
        grid_point(position, &content.terminal_bounds, content.display_offset)
    }

    /// visible `(column, line)` under a window position, clamped into the grid
    pub fn mouse_cell(&self, position: Point<Pixels>) -> (usize, usize) {
        // no display offset, programs count lines from the top of what is shown
        let (point, _) = grid_point(position, &self.last_content.terminal_bounds, 0);
        (point.column.0, point.line.0 as usize)
    }

    /// start a new selection at a window position, replacing the old one
    pub fn start_selection(&mut self, position: Point<Pixels>, ty: SelectionType) {
        let (point, side) = self.mouse_point(position);
        self.term.lock().selection = Some(Selection::new(ty, point, side));
        self.dirty.store(true, Ordering::Release);
    }

    /// move the free end of the selection to a window position
    pub fn extend_selection(&mut self, position: Point<Pixels>) {
        let (point, side) = self.mouse_point(position);
        let mut term = self.term.lock();
        match term.selection.as_mut() {
            Some(selection) => selection.update(point, side),
            None => term.selection = Some(Selection::new(SelectionType::Simple, point, side)),
        }
        self.dirty.store(true, Ordering::Release);
    }

    /// select the whole scrollback and screen
    pub fn select_all(&mut self) {
        let mut term = self.term.lock();
        let start = AlacPoint::new(term.topmost_line(), Column(0));
        // lines below a shell's cursor are empty, so they would only add blank lines
        let last_line = if term.mode().contains(TermMode::ALT_SCREEN) {
            term.bottommost_line()
        } else {
            term.grid().cursor.point.line
        };
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(AlacPoint::new(last_line, term.last_column()), Side::Right);
        term.selection = Some(selection);
        self.dirty.store(true, Ordering::Release);
    }

    /// drop the selection, if any
    pub fn clear_selection(&mut self) {
        if self.term.lock().selection.take().is_some() {
            self.dirty.store(true, Ordering::Release);
        }
    }

    /// selected text, none when nothing is selected
    pub fn selection_text(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, px, size};

    use super::*;
    use crate::{
        settings::{Shell, TerminalSettings},
        terminal::tests::{profile, spawn_with, wait_for_text},
    };

    // 10px cells, 20px lines, grid starts at (5, 40) like below a tab bar
    fn bounds() -> TerminalBounds {
        TerminalBounds::new(
            px(20.),
            px(10.),
            Bounds::new(point(px(5.), px(40.)), size(px(800.), px(480.))),
        )
    }

    /// window position inside cell `column` of screen `line`, `right` picks the right half
    fn at(line: usize, column: usize, right: bool) -> Point<Pixels> {
        let x = 5. + column as f32 * 10. + if right { 8. } else { 2. };
        point(px(x), px(40. + line as f32 * 20. + 10.))
    }

    #[test]
    fn grid_point_maps_cells_and_halves() {
        let b = bounds();
        assert_eq!(
            grid_point(at(0, 0, false), &b, 0),
            (AlacPoint::new(Line(0), Column(0)), Side::Left)
        );
        assert_eq!(
            grid_point(at(3, 7, true), &b, 0),
            (AlacPoint::new(Line(3), Column(7)), Side::Right)
        );
        // scrolled back 5 lines, the top screen line is 5 lines into history
        assert_eq!(
            grid_point(at(0, 2, false), &b, 5),
            (AlacPoint::new(Line(-5), Column(2)), Side::Left)
        );
    }

    #[test]
    fn grid_point_clamps_outside_the_grid() {
        let b = bounds();
        // left of and above the grid
        assert_eq!(
            grid_point(point(px(0.), px(0.)), &b, 0),
            (AlacPoint::new(Line(0), Column(0)), Side::Left)
        );
        // right of and below the grid: 80 columns, 24 lines
        assert_eq!(
            grid_point(point(px(2000.), px(2000.)), &b, 0),
            (AlacPoint::new(Line(23), Column(79)), Side::Right)
        );
    }

    fn two_lines() -> crate::terminal::TerminalBuilder {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 'hello world\\nsecond line'; sleep 5".into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        wait_for_text(&mut builder.terminal, "second line");
        builder
    }

    // spawn's grid starts at the window origin with 10px cells and 20px lines
    fn cell(line: usize, column: usize, right: bool) -> Point<Pixels> {
        let x = column as f32 * 10. + if right { 8. } else { 2. };
        point(px(x), px(line as f32 * 20. + 10.))
    }

    #[test]
    fn drag_selects_text_and_input_clears_it() {
        let mut builder = two_lines();
        let terminal = &mut builder.terminal;
        assert_eq!(terminal.selection_text(), None);

        terminal.start_selection(cell(0, 0, false), SelectionType::Simple);
        terminal.extend_selection(cell(0, 4, true));
        assert_eq!(terminal.selection_text().as_deref(), Some("hello"));

        // dragging backwards past the start selects the other way
        terminal.start_selection(cell(1, 5, true), SelectionType::Simple);
        terminal.extend_selection(cell(0, 6, false));
        assert_eq!(terminal.selection_text().as_deref(), Some("world\nsecond"));

        terminal.sync();
        let range = terminal
            .last_content
            .selection
            .expect("no selection in snapshot");
        assert_eq!(range.start, AlacPoint::new(Line(0), Column(6)));
        assert_eq!(range.end, AlacPoint::new(Line(1), Column(5)));

        terminal.input(b"x".to_vec());
        assert_eq!(terminal.selection_text(), None);
        terminal.sync();
        assert_eq!(terminal.last_content.selection, None);
    }

    #[test]
    fn click_without_drag_selects_nothing() {
        let mut builder = two_lines();
        let terminal = &mut builder.terminal;
        terminal.start_selection(cell(0, 2, false), SelectionType::Simple);
        terminal.extend_selection(cell(0, 2, false));
        assert_eq!(terminal.selection_text(), None);
    }

    #[test]
    fn double_click_selects_word_and_triple_click_line() {
        let mut builder = two_lines();
        let terminal = &mut builder.terminal;

        terminal.start_selection(cell(0, 8, false), SelectionType::Semantic);
        assert_eq!(terminal.selection_text().as_deref(), Some("world"));

        terminal.start_selection(cell(1, 3, false), SelectionType::Lines);
        assert_eq!(terminal.selection_text().as_deref(), Some("second line\n"));
    }
}
