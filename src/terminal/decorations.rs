//! block decorations: marks in a gutter left of the grid, chips at the end of a block's
//! output, folds and popovers over cell ranges. render only, so copy and search still see
//! every line, and nothing here is asked during prepaint but plain data

use std::{collections::HashMap, ops::RangeInclusive, sync::atomic::Ordering};

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

/// small label at the end of a block's output, like "3 errors"
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub text: String,
    /// name of the action its click runs, like "copy_last_output"
    pub key: Option<String>,
}

/// lines drawn as one line with a label, counted from the top of the history
#[derive(Clone, Debug, PartialEq)]
pub struct Fold {
    pub range: RangeInclusive<usize>,
    pub label: String,
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

/// what is drawn for one screen row when folds hide lines
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShownRow {
    /// the history line of this number
    Line(usize),
    /// the label of fold `ix` in place of its lines
    Fold(usize),
}

/// decorations of one terminal
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decorations {
    /// by block id
    pub gutter: HashMap<u64, GutterMark>,
    /// by block id
    pub chips: HashMap<u64, Vec<Chip>>,
    pub folds: Vec<Fold>,
    pub popover: Option<Popover>,
}

// folds are kept and mapped, the element does not draw them yet
#[allow(dead_code)]
/// rows shown for history lines `lines` with `folds` collapsed, each fold one row in place of
/// its first line. folds are taken in order and must not overlap
pub fn shown_rows(lines: RangeInclusive<usize>, folds: &[Fold]) -> Vec<ShownRow> {
    let mut rows = Vec::new();
    let mut line = *lines.start();
    while line <= *lines.end() {
        match folds.iter().position(|fold| fold.range.contains(&line)) {
            Some(ix) => {
                // only its first line stands for it, a fold cut at the top still shows once
                if rows.last() != Some(&ShownRow::Fold(ix)) {
                    rows.push(ShownRow::Fold(ix));
                }
                line = *folds[ix].range.end() + 1;
            }
            None => {
                rows.push(ShownRow::Line(line));
                line += 1;
            }
        }
    }
    rows
}

#[allow(dead_code)]
/// history line drawn on screen row `row`, none past the end or on a fold label
pub fn line_at_row(lines: RangeInclusive<usize>, folds: &[Fold], row: usize) -> Option<usize> {
    match shown_rows(lines, folds).get(row)? {
        ShownRow::Line(line) => Some(*line),
        ShownRow::Fold(_) => None,
    }
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
        for row in 0..term.screen_lines() {
            let line = &term.grid()[Line(row as i32 - offset)];
            // a prompt mark starts at the first cell of its line, or soon after
            let mark = (0..term.columns().min(8)).find_map(|column| {
                let link = line[Column(column)].hyperlink()?;
                link.id().strip_prefix(PROMPT_MARK)?.parse::<usize>().ok()
            });
            if let Some(mark) = mark.and_then(|mark| by_prompt.get(&mark))
                && shown
                    .last()
                    .is_none_or(|(_, last): &(usize, GutterMark)| last != *mark)
            {
                shown.push((row, (*mark).clone()));
            }
        }
        shown
    }

    /// chips with the screen row of the last output line of their block, only visible ones
    pub fn visible_chips(&self) -> Vec<(usize, Vec<Chip>)> {
        let offset = self.last_content.display_offset as i32;
        let lines = self.last_content.terminal_bounds.num_lines() as i32;
        self.decorations
            .chips
            .iter()
            .filter_map(|(id, chips)| {
                let (_, end) = self.output_range(*id)?;
                let row = end.line.0 + offset;
                (0..lines)
                    .contains(&row)
                    .then(|| (row as usize, chips.clone()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Fold, ShownRow, line_at_row, shown_rows};

    fn fold(range: std::ops::RangeInclusive<usize>) -> Fold {
        Fold {
            range,
            label: "folded".into(),
        }
    }

    #[test]
    fn folds_take_one_row_and_lines_after_move_up() {
        let folds = [fold(2..=4), fold(7..=7)];
        assert_eq!(
            shown_rows(0..=9, &folds),
            [
                ShownRow::Line(0),
                ShownRow::Line(1),
                ShownRow::Fold(0),
                ShownRow::Line(5),
                ShownRow::Line(6),
                ShownRow::Fold(1),
                ShownRow::Line(8),
                ShownRow::Line(9),
            ]
        );
        assert_eq!(line_at_row(0..=9, &folds, 3), Some(5));
        assert_eq!(line_at_row(0..=9, &folds, 2), None);
        assert_eq!(line_at_row(0..=9, &folds, 99), None);
        // a fold starting above the view still shows once
        assert_eq!(
            shown_rows(3..=6, &folds),
            [ShownRow::Fold(0), ShownRow::Line(5), ShownRow::Line(6)]
        );
        assert_eq!(
            shown_rows(0..=2, &[]),
            [ShownRow::Line(0), ShownRow::Line(1), ShownRow::Line(2)]
        );
    }

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
        assert_eq!(terminal.visible_gutter(), [(2, mark)]);
        terminal.decorate(|decorations| {
            decorations.chips.insert(
                ids[0],
                vec![super::Chip {
                    text: "chip".into(),
                    key: None,
                }],
            )
        });
        terminal.sync();
        let chips = terminal.visible_chips();
        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].0, 1);
    }
}
