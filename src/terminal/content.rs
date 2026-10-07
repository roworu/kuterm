//! snapshot of visible grid, read by painter

use alacritty_terminal::{
    Term,
    grid::Dimensions,
    index::Point as AlacPoint,
    selection::SelectionRange,
    term::{RenderableCursor, TermMode, cell::Cell},
};

use super::{TerminalBounds, builder::ZedListener};

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
        }
    }
}

impl Content {
    /// copy visible grid from `term`, reusing the cell buffer so steady redraws don't allocate
    pub(super) fn refresh(&mut self, term: &Term<ZedListener>) {
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
    }
}
