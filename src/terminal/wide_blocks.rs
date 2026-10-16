//! finished blocks keep the width they were printed at: narrowing a pane rewraps old
//! `kubectl get` and `docker ps` tables into a mess, so before alacritty reflows, the
//! unwrapped lines of blocks printed wider are copied aside and drawn in place of their
//! reflowed rows, cut at the pane edge and scrolled sideways with shift+wheel.
//!
//! an experiment behind `terminal.keep_block_width`. only the drawing changes: copy, search
//! and `output_text` read the grid, where soft wrapped rows join into the same lines the
//! side copy holds. selection and search highlights are still drawn where the grid has the
//! text, the reflowed rows, which is the part that would need render side line mapping to
//! go further

use std::collections::HashMap;

use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::Dimensions,
    index::{Column, Line, Point as AlacPoint},
    term::cell::{Cell, Flags},
};

use super::{IndexedCell, Terminal};

// drawn in the last column of a line cut at the pane edge
const MORE: char = '→';

/// a block drawn from its side copy
#[derive(Clone, Debug, PartialEq)]
pub struct WideBlock {
    /// columns it was printed at
    pub width: usize,
    /// its output lines with soft wraps joined, trailing empty cells cut
    pub lines: Vec<Vec<Cell>>,
    /// line of its first output row counted from the top of the history, after the reflow
    pub abs_start: usize,
    /// columns scrolled sideways
    pub offset: usize,
}

/// rows of `term` from `start` to `end` joined at soft wraps
pub(super) fn logical_lines<T: EventListener>(
    term: &Term<T>,
    start: Line,
    end: Line,
) -> Vec<Vec<Cell>> {
    let mut lines = Vec::new();
    let mut current: Vec<Cell> = Vec::new();
    let last = term.last_column();
    for line in start.0..=end.0 {
        let row = &term.grid()[Line(line)];
        current.extend((0..=last.0).map(|column| row[Column(column)].clone()));
        if !row[last].flags.contains(Flags::WRAPLINE) {
            lines.push(trimmed(std::mem::take(&mut current)));
        }
    }
    if !current.is_empty() {
        lines.push(trimmed(current));
    }
    lines
}

fn trimmed(mut cells: Vec<Cell>) -> Vec<Cell> {
    let empty = Cell::default();
    while cells.last().is_some_and(|cell| *cell == empty) {
        cells.pop();
    }
    for cell in &mut cells {
        cell.flags.remove(Flags::WRAPLINE);
    }
    cells
}

/// rows lines of these lengths take once wrapped at `columns`
pub fn rows_after_reflow(lengths: impl IntoIterator<Item = usize>, columns: usize) -> usize {
    lengths
        .into_iter()
        .map(|len| len.div_ceil(columns.max(1)).max(1))
        .sum()
}

/// cells drawn for row `row` of `block` at `columns` wide: its line scrolled by the block's
/// offset and cut at the edge with an arrow, none past its lines
pub fn shown_line(block: &WideBlock, row: usize, columns: usize) -> Option<Vec<Cell>> {
    let line = block.lines.get(row)?;
    let mut cells: Vec<Cell> = line
        .iter()
        .skip(block.offset)
        .take(columns)
        .cloned()
        .collect();
    if line.len() > block.offset + columns
        && let Some(last) = cells.last_mut()
    {
        *last = Cell {
            c: MORE,
            ..Cell::default()
        };
    }
    Some(cells)
}

impl WideBlock {
    /// rows its reflowed output takes at `columns`
    pub fn grid_rows(&self, columns: usize) -> usize {
        rows_after_reflow(self.lines.iter().map(Vec::len), columns)
    }

    /// longest line, how far it scrolls
    fn widest(&self) -> usize {
        self.lines.iter().map(Vec::len).max().unwrap_or(0)
    }
}

/// `cells` with the rows of `blocks` drawn from their side copies, the extra rows reflow
/// added left empty. `history` and `display_offset` place the blocks
pub(super) fn replace_rows(
    cells: &mut Vec<IndexedCell>,
    blocks: &HashMap<u64, WideBlock>,
    history: usize,
    columns: usize,
) {
    if blocks.is_empty() {
        return;
    }
    let mut ranges = Vec::new();
    let mut extra = Vec::new();
    let (first, last) = match (cells.first(), cells.last()) {
        (Some(first), Some(last)) => (first.point.line.0, last.point.line.0),
        _ => return,
    };
    for block in blocks.values() {
        let start = block.abs_start as i32 - history as i32;
        let end = start + block.grid_rows(columns) as i32 - 1;
        if end < first || start > last {
            continue;
        }
        ranges.push(start..=end);
        for line in start.max(first)..=end.min(last) {
            let row = (line - start) as usize;
            for (column, cell) in shown_line(block, row, columns)
                .into_iter()
                .flatten()
                .enumerate()
            {
                extra.push(IndexedCell {
                    point: AlacPoint::new(Line(line), Column(column)),
                    cell,
                });
            }
        }
    }
    cells.retain(|cell| {
        !ranges
            .iter()
            .any(|range| range.contains(&cell.point.line.0))
    });
    cells.extend(extra);
    cells.sort_by_key(|cell| (cell.point.line, cell.point.column));
}

impl Terminal {
    /// before a resize to `columns`: copy aside finished blocks printed wider, and drop the
    /// copies a pane at least as wide needs no more
    pub(super) fn keep_wide_blocks(&mut self, columns: usize) {
        self.wide_blocks.retain(|_, block| block.width > columns);
        if !self.keep_block_width {
            self.wide_blocks.clear();
            return;
        }
        let now = self.term.lock_unfair().columns();
        if columns >= now {
            return;
        }
        let wide: Vec<(u64, usize)> = self
            .blocks
            .list
            .iter()
            .filter(|block| block.ended.is_some() && !self.wide_blocks.contains_key(&block.id))
            .filter_map(|block| Some((block.id, block.columns?)))
            .filter(|(_, width)| *width > columns)
            .collect();
        for (id, width) in wide {
            let Some((start, end)) = self.output_range(id) else {
                continue;
            };
            let term = self.term.lock_unfair();
            let lines = logical_lines(&term, start.line, end.line);
            drop(term);
            self.wide_blocks.insert(
                id,
                WideBlock {
                    width,
                    lines,
                    abs_start: 0,
                    offset: 0,
                },
            );
        }
    }

    /// after a resize: where the copied blocks start in the reflowed grid
    pub(super) fn place_wide_blocks(&mut self) {
        let ids: Vec<u64> = self.wide_blocks.keys().copied().collect();
        for id in ids {
            let placed = self.output_range(id).map(|(start, _)| {
                let history = self.term.lock_unfair().grid().history_size();
                (start.line.0 + history as i32).max(0) as usize
            });
            match placed {
                Some(abs) => {
                    if let Some(block) = self.wide_blocks.get_mut(&id) {
                        block.abs_start = abs;
                    }
                }
                None => {
                    self.wide_blocks.remove(&id);
                }
            }
        }
    }

    /// scroll the wide block under grid line `line` sideways by `columns`, false when no
    /// wide block is there
    pub fn scroll_wide_block(&mut self, line: Line, columns: i32) -> bool {
        let history = self.term.lock_unfair().grid().history_size() as i32;
        let width = self.last_content.terminal_bounds.num_columns();
        let Some(block) = self.wide_blocks.values_mut().find(|block| {
            let start = block.abs_start as i32 - history;
            (start..start + block.grid_rows(width) as i32).contains(&line.0)
        }) else {
            return false;
        };
        let most = block.widest().saturating_sub(width);
        block.offset = (block.offset as i64 + i64::from(columns)).clamp(0, most as i64) as usize;
        self.dirty.store(true, std::sync::atomic::Ordering::Release);
        true
    }

    /// the wide block under a window position, for shift+wheel
    pub fn wide_block_line(&self, position: gpui::Point<gpui::Pixels>) -> Option<Line> {
        (!self.wide_blocks.is_empty()).then(|| self.mouse_point(position).0.line)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use alacritty_terminal::{
        Term,
        event::VoidListener,
        grid::Dimensions,
        index::{Column, Line, Point as AlacPoint},
        term::{Config, cell::Cell, test::TermSize},
        vte::ansi::Processor,
    };

    use super::{WideBlock, logical_lines, replace_rows, rows_after_reflow, shown_line};
    use crate::terminal::IndexedCell;

    fn text(cells: &[Cell]) -> String {
        cells.iter().map(|cell| cell.c).collect()
    }

    fn block(lines: &[&str], abs_start: usize, offset: usize) -> WideBlock {
        WideBlock {
            width: 20,
            lines: lines
                .iter()
                .map(|line| {
                    line.chars()
                        .map(|c| Cell {
                            c,
                            ..Cell::default()
                        })
                        .collect()
                })
                .collect(),
            abs_start,
            offset,
        }
    }

    #[test]
    fn wrapped_rows_join_back_into_lines() {
        let mut term = Term::new(Config::default(), &TermSize::new(10, 5), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"0123456789abcdef\r\nshort\r\n\r\nend");
        let lines = logical_lines(&term, Line(0), Line(term.screen_lines() as i32 - 1));
        let texts: Vec<String> = lines.iter().map(|line| text(line)).collect();
        assert_eq!(texts[..4], ["0123456789abcdef", "short", "", "end"]);
        assert_eq!(rows_after_reflow([16, 5, 0, 3], 10), 5);
        assert_eq!(rows_after_reflow([16, 5, 0, 3], 4), 4 + 2 + 1 + 1);
    }

    #[test]
    fn lines_are_cut_at_the_edge_and_scroll_sideways() {
        let wide = block(&["NAME   STATUS   AGE", "web    Running  3d"], 0, 0);
        assert_eq!(text(&shown_line(&wide, 0, 10).unwrap()), "NAME   ST→");
        assert_eq!(
            text(&shown_line(&wide, 1, 20).unwrap()),
            "web    Running  3d"
        );
        let scrolled = WideBlock { offset: 7, ..wide };
        assert_eq!(text(&shown_line(&scrolled, 0, 10).unwrap()), "STATUS   →");
        assert_eq!(shown_line(&scrolled, 2, 10), None);
    }

    #[test]
    fn reflowed_rows_are_drawn_from_the_copy_and_extra_rows_are_empty() {
        // two 19 wide lines take 4 rows at 10 columns, history of 100 lines above
        let wide = block(&["NAME   STATUS   AGE", "web    Running  3d"], 103, 0);
        let mut cells: Vec<IndexedCell> = (0..6)
            .flat_map(|line| {
                (0..10).map(move |column| IndexedCell {
                    point: AlacPoint::new(Line(line), Column(column)),
                    cell: Cell {
                        c: 'x',
                        ..Cell::default()
                    },
                })
            })
            .collect();
        replace_rows(&mut cells, &HashMap::from([(1, wide)]), 100, 10);
        let row = |line: i32| -> String {
            cells
                .iter()
                .filter(|cell| cell.point.line == Line(line))
                .map(|cell| cell.cell.c)
                .collect()
        };
        assert_eq!(row(2), "xxxxxxxxxx");
        assert_eq!(row(3), "NAME   ST→");
        assert_eq!(row(4), "web    Ru→");
        assert_eq!(row(5), "");
        // nothing changes without wide blocks
        let before = cells.len();
        replace_rows(&mut cells, &HashMap::new(), 100, 10);
        assert_eq!(cells.len(), before);
    }

    #[test]
    fn narrowing_keeps_a_finished_table_unwrapped() {
        use gpui::{Bounds, point, px, size};

        use crate::{
            settings::{Shell, TerminalSettings},
            terminal::{
                TerminalBounds,
                tests::{profile, spawn_with, wait_for_text},
            },
        };
        let row = "a".repeat(70);
        let script = format!(
            "printf '\\033]133;A\\007$ \\033]133;B\\007\\033]133;E;t\\007\\n\\033]133;C\\007{row}b\\n\\033]133;D;0\\007'; echo table_done; sleep 5"
        );
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script],
        });
        let settings = TerminalSettings {
            keep_block_width: true,
            ..TerminalSettings::default()
        };
        let mut builder = spawn_with(&settings, &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "table_done");
        terminal.handle_scan_events();
        assert_eq!(terminal.blocks().back().unwrap().columns, Some(80));
        // 80 columns down to 40
        terminal.set_size(TerminalBounds::new(
            px(20.),
            px(10.),
            Bounds::new(point(px(0.), px(0.)), size(px(400.), px(480.))),
        ));
        terminal.sync();
        assert_eq!(terminal.wide_blocks.len(), 1);
        // narrowing may push lines into history, the block says where it went
        let block = terminal.wide_blocks.values().next().unwrap();
        let line = Line(block.abs_start as i32 - terminal.history_size() as i32);
        let row_text = |terminal: &crate::terminal::Terminal| -> String {
            terminal
                .last_content
                .cells
                .iter()
                .filter(|cell| cell.point.line == line)
                .map(|cell| cell.cell.c)
                .collect()
        };
        assert_eq!(row_text(terminal), format!("{}→", "a".repeat(39)));
        // the second row reflow made is left empty
        assert!(
            !terminal
                .last_content
                .cells
                .iter()
                .any(|cell| cell.point.line == line + 1)
        );
        // the text read back is the whole line, as printed
        let id = terminal.blocks().back().unwrap().id;
        assert_eq!(terminal.output_text(id).unwrap().0, format!("{row}b"));
        assert!(terminal.scroll_wide_block(line, 100));
        assert!(!terminal.scroll_wide_block(line + 5, 1));
        terminal.sync();
        let shown = row_text(terminal);
        assert!(shown.ends_with("ab"), "{shown:?}");
        // wide enough again, the copy goes
        terminal.set_size(TerminalBounds::new(
            px(20.),
            px(10.),
            Bounds::new(point(px(0.), px(0.)), size(px(800.), px(480.))),
        ));
        terminal.sync();
        assert!(terminal.wide_blocks.is_empty());
    }
}
