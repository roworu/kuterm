//! blocks: each command the shell ran, with its command line, folder, times, exit code and the
//! marks its output is found by. lines are looked up through the marks when asked, since they
//! move as history grows and is cut

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime},
};

use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::Dimensions,
    index::{Column, Line, Point as AlacPoint},
    term::cell::Flags,
};

use super::{
    Terminal,
    integration::{command_after_prompt, mark_cells},
    scanner::{OUTPUT_MARK, PROMPT_MARK},
};

// blocks each terminal remembers, older ones are dropped
const MAX_BLOCKS: usize = 1000;

// block ids are unique in the whole app, so a reference names one block of one pane
static NEXT_BLOCK: AtomicU64 = AtomicU64::new(1);

/// a command the shell ran
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub id: u64,
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub started: Instant,
    pub started_at: SystemTime,
    /// none while it runs
    pub ended: Option<Instant>,
    pub exit: Option<i32>,
    /// number of the mark of its output, `kuterm-output-<mark>`
    pub mark: usize,
    /// number of the mark of its prompt, none when the prompt was not marked
    pub prompt_mark: Option<usize>,
}

/// text people see in cells from `start` to `end`: concealed text and text in its background
/// color are left out, so is zero-width text, while overwritten text never is in the grid.
/// true when something was left out
pub(crate) fn visible_text<T: EventListener>(
    term: &Term<T>,
    start: AlacPoint,
    end: AlacPoint,
) -> (String, bool) {
    let mut text = String::new();
    let mut hidden = false;
    for line in start.line.0..=end.line.0 {
        let row = &term.grid()[Line(line)];
        let first = if line == start.line.0 {
            start.column.0
        } else {
            0
        };
        let last = if line == end.line.0 {
            end.column.0
        } else {
            term.last_column().0
        };
        let mut row_text = String::new();
        for column in first..=last.min(term.last_column().0) {
            let cell = &row[Column(column)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let (fg, bg) = if cell.flags.contains(Flags::INVERSE) {
                (cell.bg, cell.fg)
            } else {
                (cell.fg, cell.bg)
            };
            let unseen = cell.flags.contains(Flags::HIDDEN) || fg == bg;
            if unseen && cell.c != ' ' {
                hidden = true;
                row_text.push(' ');
                continue;
            }
            hidden |= cell.zerowidth().is_some_and(|chars| !chars.is_empty());
            row_text.push(cell.c);
        }
        text.push_str(row_text.trim_end());
        let wraps = row[term.last_column()].flags.contains(Flags::WRAPLINE);
        if line != end.line.0 && !wraps {
            text.push('\n');
        }
    }
    (text, hidden)
}

/// the smallest mark number still in `term`, none when no marked cell is left
fn oldest_mark<T: EventListener>(term: &Term<T>) -> Option<usize> {
    for line in term.topmost_line().0..=term.bottommost_line().0 {
        let row = &term.grid()[Line(line)];
        for column in 0..term.columns() {
            let Some(link) = row[Column(column)].hyperlink() else {
                continue;
            };
            let id = link.id();
            let number = id
                .strip_prefix(PROMPT_MARK)
                .or_else(|| id.strip_prefix(OUTPUT_MARK))
                .and_then(|number| number.parse().ok());
            if number.is_some() {
                return number;
            }
        }
    }
    None
}

/// blocks of one terminal, oldest first
#[derive(Default)]
pub(crate) struct Blocks {
    pub(super) list: VecDeque<Block>,
    /// line the shell sent with osc 133;E for the command about to start
    pending_line: Option<String>,
}

impl Blocks {
    pub(super) fn command_line(&mut self, line: String) {
        self.pending_line = Some(line);
    }

    /// a command started, its line from the shell or else read after its prompt
    pub(super) fn start<T: EventListener>(
        &mut self,
        term: &Term<T>,
        prompt_mark: Option<usize>,
        mark: usize,
        cwd: Option<PathBuf>,
        started: Instant,
    ) -> u64 {
        let command = self
            .pending_line
            .take()
            .or_else(|| prompt_mark.and_then(|prompt| command_after_prompt(term, prompt)))
            .unwrap_or_default();
        let id = NEXT_BLOCK.fetch_add(1, Ordering::Relaxed);
        self.list.push_back(Block {
            id,
            command,
            cwd,
            started,
            started_at: SystemTime::now(),
            ended: None,
            exit: None,
            mark,
            prompt_mark,
        });
        if self.list.len() > MAX_BLOCKS {
            self.list.pop_front();
        }
        id
    }

    /// the running command ended
    pub(super) fn end(&mut self, exit: Option<i32>, ended: Instant) -> Option<&Block> {
        let block = self.list.back_mut().filter(|block| block.ended.is_none())?;
        block.ended = Some(ended);
        block.exit = exit;
        Some(block)
    }
}

impl Terminal {
    /// commands run in this terminal whose lines are still in its history, oldest first
    pub fn blocks(&mut self) -> &VecDeque<Block> {
        let term = self.term.lock_unfair();
        let oldest = oldest_mark(&term);
        drop(term);
        // marks are numbered in order, so blocks before the oldest one left scrolled away
        let list = &mut self.blocks.list;
        match oldest {
            Some(oldest) => list.retain(|block| block.mark >= oldest),
            None => list.clear(),
        }
        &self.blocks.list
    }

    /// true when this terminal ran block `id`
    pub fn has_block(&self, id: u64) -> bool {
        self.block(id).is_some()
    }

    fn block(&self, id: u64) -> Option<&Block> {
        self.blocks.list.iter().find(|block| block.id == id)
    }

    /// first and last cell of the output of block `id`, whole lines
    pub fn output_range(&self, id: u64) -> Option<(AlacPoint, AlacPoint)> {
        let mark = self.block(id)?.mark;
        let term = self.term.lock_unfair();
        let (start, end) = mark_cells(&term, &format!("{OUTPUT_MARK}{mark}"))?;
        Some((
            AlacPoint::new(start.line, Column(0)),
            AlacPoint::new(end.line, term.last_column()),
        ))
    }

    /// what people see of the output of block `id`, and whether hidden text was left out
    pub fn output_text(&self, id: u64) -> Option<(String, bool)> {
        let (start, end) = self.output_range(id)?;
        let term = self.term.lock_unfair();
        let (text, hidden) = visible_text(&term, start, end);
        Some((text.trim_end().to_string(), hidden))
    }

    /// first and last cell of the prompt and command line of block `id`
    #[allow(dead_code)]
    pub fn command_range(&self, id: u64) -> Option<(AlacPoint, AlacPoint)> {
        let prompt = self.block(id)?.prompt_mark?;
        let term = self.term.lock_unfair();
        let (start, end) = mark_cells(&term, &format!("{PROMPT_MARK}{prompt}"))?;
        let mut last = end.line;
        while last < term.bottommost_line()
            && term.grid()[last][term.last_column()]
                .flags
                .contains(Flags::WRAPLINE)
        {
            last += 1;
        }
        Some((start, AlacPoint::new(last, term.last_column())))
    }

    /// block whose output covers grid line `line`
    #[allow(dead_code)]
    pub fn block_at(&self, line: Line) -> Option<u64> {
        self.blocks.list.iter().rev().find_map(|block| {
            let (start, end) = self.output_range(block.id)?;
            (start.line <= line && line <= end.line).then_some(block.id)
        })
    }

    /// what people see on screen, or in the whole history, and whether hidden text was left out
    pub fn visible_screen_text(&self, history: bool) -> (String, bool) {
        let term = self.term.lock_unfair();
        let top = if history {
            term.topmost_line()
        } else {
            Line(0)
        };
        let start = AlacPoint::new(top, Column(0));
        let end = AlacPoint::new(term.bottommost_line(), term.last_column());
        let (text, hidden) = visible_text(&term, start, end);
        (text.trim_end().to_string(), hidden)
    }

    /// select lines `first` to `last` (1 based) of the output of block `id` and scroll to them,
    /// false when the block left the history
    pub fn select_block_lines(&mut self, id: u64, lines: Option<(usize, usize)>) -> bool {
        let Some((start, end)) = self.output_range(id) else {
            return false;
        };
        let (start, end) = match lines {
            Some((first, last)) => {
                let line = |n: usize| Line((start.line.0 + n.max(1) as i32 - 1).min(end.line.0));
                (
                    AlacPoint::new(line(first), Column(0)),
                    AlacPoint::new(line(last.max(first)), end.column),
                )
            }
            None => (start, end),
        };
        let mut term = self.term.lock();
        let mut selection = alacritty_terminal::selection::Selection::new(
            alacritty_terminal::selection::SelectionType::Lines,
            start,
            alacritty_terminal::index::Side::Left,
        );
        selection.update(end, alacritty_terminal::index::Side::Right);
        term.selection = Some(selection);
        term.scroll_to_point(start);
        drop(term);
        self.dirty.store(true, Ordering::Release);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use alacritty_terminal::{
        Term,
        event::VoidListener,
        grid::Dimensions,
        index::{Column, Line, Point as AlacPoint},
        term::{Config, test::TermSize},
        vte::ansi::Processor,
    };

    use super::visible_text;
    use crate::{
        settings::{Shell, TerminalSettings},
        terminal::tests::{profile, spawn_with, wait_for_text},
    };

    fn term_with(bytes: &str) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &TermSize::new(30, 4), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, bytes.as_bytes());
        term
    }

    fn all(term: &Term<VoidListener>) -> (String, bool) {
        visible_text(
            term,
            AlacPoint::new(Line(0), Column(0)),
            AlacPoint::new(term.bottommost_line(), term.last_column()),
        )
    }

    #[test]
    fn concealed_and_invisible_text_is_left_out() {
        let (text, hidden) = all(&term_with("shown \x1b[8msecret\x1b[0m end"));
        assert_eq!(text.trim_end(), "shown        end");
        assert!(hidden);
        // the same color as the background
        let (text, hidden) = all(&term_with("a \x1b[31;41mred\x1b[0m b"));
        assert_eq!(text.trim_end(), "a     b");
        assert!(hidden);
        // zero-width chars ride on a cell and never show
        let (text, hidden) = all(&term_with("ls\u{200b}x"));
        assert_eq!(text.trim_end(), "lsx");
        assert!(hidden);
        // overwritten text is gone from the grid already
        let (text, hidden) = all(&term_with("rm -rf /\r\x1b[Kls"));
        assert_eq!(text.trim_end(), "ls");
        assert!(!hidden);
    }

    #[test]
    fn blocks_keep_command_exit_cwd_and_output_until_they_scroll_away() {
        let script = concat!(
            "printf '\\033]133;A\\007$ \\033]133;B\\007\\033]133;E;first\\\\x3b cmd\\007\\n\\033]133;C\\007'; ",
            "printf 'one\\n\\033]133;D;0\\007\\033]7;file://h/tmp\\007'; ",
            "printf '\\033]133;A\\007$ \\033]133;B\\007second\\n\\033]133;C\\007two\\nlines\\n\\033]133;D;3\\007'; ",
            "echo blocks_ready; read x; seq 1 200; echo all_gone; sleep 5"
        );
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        });
        let settings = TerminalSettings {
            max_history_length: 50,
            ..TerminalSettings::default()
        };
        let mut builder = spawn_with(&settings, &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "blocks_ready");
        terminal.handle_scan_events();
        let blocks: Vec<_> = terminal.blocks().iter().cloned().collect();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].command, "first; cmd");
        assert_eq!(blocks[0].exit, Some(0));
        assert_eq!(blocks[0].cwd, None);
        // without osc 133;E the line is read after the prompt
        assert_eq!(blocks[1].command, "second");
        assert_eq!(blocks[1].exit, Some(3));
        assert_eq!(blocks[1].cwd.as_deref(), Some(std::path::Path::new("/tmp")));
        assert!(blocks[1].ended.unwrap() >= blocks[1].started);
        assert_eq!(
            terminal.output_text(blocks[0].id),
            Some(("one".into(), false))
        );
        assert_eq!(
            terminal.output_text(blocks[1].id),
            Some(("two\nlines".into(), false))
        );
        let (start, _) = terminal.output_range(blocks[1].id).unwrap();
        assert_eq!(terminal.block_at(start.line), Some(blocks[1].id));

        // their lines leave the short history, the blocks go with them
        terminal.input(b"\r".to_vec());
        wait_for_text(terminal, "all_gone");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !terminal.blocks().is_empty() {
            assert!(Instant::now() < deadline, "blocks stayed");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
