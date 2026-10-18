//! popovers over cell ranges. render only, so copy and search still see every line, and
//! nothing here is asked during prepaint but plain data

use std::sync::atomic::Ordering;

use alacritty_terminal::index::Point as AlacPoint;

use super::Terminal;

/// box of text shown over the grid next to a cell range while the mouse is over it
#[derive(Clone, Debug, PartialEq)]
pub struct Popover {
    pub range: (AlacPoint, AlacPoint),
    pub content: Vec<PopoverLine>,
}

/// one line of a popover
#[derive(Clone, Debug, PartialEq)]
pub struct PopoverLine {
    pub text: String,
    /// drawn brighter, like the line a `file:line` names
    pub highlight: bool,
}

/// decorations of one terminal
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decorations {
    pub popover: Option<Popover>,
}

impl Terminal {
    /// decorations drawn with the grid
    pub fn decorations(&self) -> &Decorations {
        &self.decorations
    }

    /// change the decorations and redraw
    pub fn decorate<R>(&mut self, change: impl FnOnce(&mut Decorations) -> R) -> R {
        let result = change(&mut self.decorations);
        self.dirty.store(true, Ordering::Release);
        result
    }
}
