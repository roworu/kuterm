//! snapshot of visible grid, read by painter

use alacritty_terminal::{
    Term,
    grid::Dimensions,
    index::{Column, Line, Point as AlacPoint},
    selection::SelectionRange,
    term::{RenderableCursor, TermMode, cell::Cell, search::Match},
};

use super::{
    TerminalBounds,
    builder::ZedListener,
    graphics::{Graphics, IMAGE_MARK, VisibleImage},
    search::{SearchState, active_match, visible_matches},
};

pub struct IndexedCell {
    pub point: AlacPoint,
    pub cell: Cell,
}

/// snapshot of grid, refreshed by `sync` when the grid changed
pub struct Content {
    pub cells: Vec<IndexedCell>,
    /// selected range in grid coordinates, same as `IndexedCell::point`
    pub selection: Option<SelectionRange>,
    pub mode: TermMode,
    pub display_offset: usize,
    pub history_size: usize,
    pub cursor: RenderableCursor,
    pub cursor_char: char,
    /// program asked for a blinking cursor
    pub cursor_blinking: bool,
    pub terminal_bounds: TerminalBounds,
    /// search matches in the visible lines, top to bottom
    pub search_matches: Vec<Match>,
    /// the match search is on
    pub active_match: Option<Match>,
    /// images reaching into the view, with the cell each one starts at
    pub images: Vec<(AlacPoint, VisibleImage)>,
}

impl Default for Content {
    fn default() -> Self {
        Content {
            cells: Vec::new(),
            selection: None,
            mode: TermMode::empty(),
            display_offset: 0,
            history_size: 0,
            cursor: RenderableCursor {
                shape: alacritty_terminal::vte::ansi::CursorShape::Block,
                point: AlacPoint::default(),
            },
            cursor_char: ' ',
            cursor_blinking: false,
            terminal_bounds: TerminalBounds::default(),
            search_matches: Vec::new(),
            active_match: None,
            images: Vec::new(),
        }
    }
}

impl Content {
    /// copy visible grid from `term`, reusing the cell buffer so steady redraws don't allocate
    pub(super) fn refresh(
        &mut self,
        term: &Term<ZedListener>,
        search: Option<&mut SearchState>,
        graphics: &Graphics,
    ) {
        let content = term.renderable_content();
        self.cells.clear();
        self.cells
            .extend(content.display_iter.map(|indexed| IndexedCell {
                point: indexed.point,
                cell: indexed.cell.clone(),
            }));
        self.selection = content.selection;
        self.mode = content.mode;
        self.display_offset = content.display_offset;
        self.history_size = term.grid().history_size();
        self.cursor = content.cursor;
        self.cursor_char = term.grid()[content.cursor.point].c;
        self.cursor_blinking = term.cursor_style().blinking;
        (self.search_matches, self.active_match) = match search {
            Some(search) => (visible_matches(term, search), active_match(term, search)),
            None => (Vec::new(), None),
        };
        self.images.clear();
        if !graphics.is_empty() {
            self.find_images(term, graphics);
        }
    }

    // an image starting above the view still shows its lower part, so the lines it can reach
    // down from are looked at too
    fn find_images(&mut self, term: &Term<ZedListener>, graphics: &Graphics) {
        let top = -(term.grid().display_offset() as i32);
        let first = (top - graphics.tallest() as i32 + 1).max(term.topmost_line().0);
        let last = top + term.screen_lines() as i32 - 1;
        for line in first..=last {
            let row = &term.grid()[Line(line)];
            for column in 0..term.columns() {
                let Some(link) = row[Column(column)].hyperlink() else {
                    continue;
                };
                let Some(mark) = link
                    .id()
                    .strip_prefix(IMAGE_MARK)
                    .and_then(|mark| mark.parse().ok())
                else {
                    continue;
                };
                if let Some(image) = graphics.placement(mark) {
                    let point = AlacPoint::new(Line(line), Column(column));
                    // shown rows of an image above the view
                    if line + image.rows as i32 > top {
                        self.images.push((point, image));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;

    use crate::{
        settings::{Shell, TerminalSettings},
        terminal::tests::{profile, spawn_with, wait_for_text},
    };

    #[test]
    fn only_marks_of_placed_images_are_shown() {
        // 10 by 20 pixels is one cell of the test grid
        let data = base64::engine::general_purpose::STANDARD.encode(vec![200u8; 10 * 20 * 4]);
        let image = |id: u32| format!("\\033_Ga=T,q=2,f=32,s=10,v=20,i={id};{data}\\033\\\\");
        // a link of the program's own is a hyperlink too, but no image mark. image 9 loses its
        // placement, the mark it left in the grid then shows nothing
        let script = format!(
            "printf '{}a {}b \\033]8;;http://x\\033\\\\link\\033]8;;\\033\\\\ \\033_Ga=d,d=i,q=2,i=9\\033\\\\done\\n'; sleep 5",
            image(9),
            image(10),
        );
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "done");
        terminal.sync();
        let images = &terminal.last_content.images;
        assert_eq!(images.len(), 1);
        // the second image, after "a "
        let (point, _) = &images[0];
        assert_eq!((point.line.0, point.column.0), (0, 3));
    }
}
