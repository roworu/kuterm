//! block decorations: marks in a gutter left of the grid and popovers over cell ranges.
//! render only, so copy and search still see every line, and nothing here is asked during
//! prepaint but plain data

use std::{collections::HashMap, sync::atomic::Ordering};

use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line, Point as AlacPoint},
};
use gpui::Hsla;

use super::{Terminal, scanner::PROMPT_MARK};

/// icon in the gutter next to a block's command line
#[derive(Clone, Debug, PartialEq)]
pub struct GutterMark {
    pub icon: String,
    pub color: Hsla,
    /// shown while the mouse is over the icon
    pub tooltip: String,
}

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
    /// by block id
    pub gutter: HashMap<u64, GutterMark>,
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

    /// gutter marks on the visible rows, by screen row from the top, read from the prompt
    /// marks so they follow scrollback
    pub fn visible_gutter(&self) -> Vec<(usize, GutterMark)> {
        if self.decorations.gutter.is_empty() {
            return Vec::new();
        }
        let by_prompt: HashMap<usize, &GutterMark> = self
            .blocks
            .list
            .iter()
            .filter_map(|block| Some((block.prompt_mark?, self.decorations.gutter.get(&block.id)?)))
            .collect();
        let term = self.term.lock_unfair();
        let offset = term.grid().display_offset() as i32;
        let mut shown = Vec::new();
        // a prompt over several lines gets one mark, two prompts with equal marks get one each
        let mut last_prompt = None;
        for row in 0..term.screen_lines() {
            let line = &term.grid()[Line(row as i32 - offset)];
            // a prompt mark starts at the first cell of its line, or soon after
            let mark = (0..term.columns().min(8)).find_map(|column| {
                let link = line[Column(column)].hyperlink()?;
                link.id().strip_prefix(PROMPT_MARK)?.parse::<usize>().ok()
            });
            if let Some(prompt) = mark
                && last_prompt != Some(prompt)
                && let Some(mark) = by_prompt.get(&prompt)
            {
                last_prompt = Some(prompt);
                shown.push((row, (*mark).clone()));
            }
        }
        shown
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn gutter_marks_follow_their_prompt() {
        use crate::{
            settings::{Shell, TerminalSettings},
            terminal::tests::{profile, spawn_with, wait_for_text},
        };
        let script = concat!(
            "printf '\\033]133;A\\007$ \\033]133;B\\007\\033]133;E;one\\007\\n\\033]133;C\\007o\\n\\033]133;D;0\\007'; ",
            "printf '\\033]133;A\\007$ \\033]133;B\\007\\033]133;E;two\\007\\n\\033]133;C\\007t\\n\\033]133;D;0\\007'; ",
            "echo marks_ready; sleep 5"
        );
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "marks_ready");
        terminal.handle_scan_events();
        let ids: Vec<u64> = terminal.blocks().iter().map(|block| block.id).collect();
        assert!(terminal.visible_gutter().is_empty());
        let mark = super::GutterMark {
            icon: "R".into(),
            color: gpui::black(),
            tooltip: "remote".into(),
        };
        terminal.decorate(|decorations| decorations.gutter.insert(ids[1], mark.clone()));
        // the second prompt is the third screen row, after "$ " and "o"
        assert_eq!(terminal.visible_gutter(), [(2, mark.clone())]);
        // an equal mark on the block before still shows
        terminal.decorate(|decorations| decorations.gutter.insert(ids[0], mark.clone()));
        assert_eq!(terminal.visible_gutter(), [(0, mark.clone()), (2, mark)]);
    }
}
