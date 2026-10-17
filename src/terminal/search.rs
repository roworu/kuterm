//! regex search over scrollback and screen, matches are highlighted and walked one by one

use std::{
    ops::{Range, RangeInclusive},
    sync::{Arc, atomic::Ordering},
};

use alacritty_terminal::sync::FairMutex;

use alacritty_terminal::{
    Term,
    grid::Dimensions,
    index::{Column, Line, Point as AlacPoint},
    term::{
        TermMode,
        search::{Match, RegexIter, RegexSearch},
    },
};

use super::{Terminal, builder::ZedListener};
use crate::settings::SearchCase;

// a search over a huge history stops counting here, it would only cost time
const MAX_MATCHES: usize = 10_000;

/// what to look for and how
#[derive(Clone, Debug, PartialEq)]
pub struct SearchQuery {
    pub text: String,
    pub regex: bool,
    pub whole_word: bool,
    pub case: SearchCase,
    pub scope: SearchScope,
}

/// which lines a search looks at
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SearchScope {
    #[default]
    All,
    /// the output of one block
    Block(u64),
    /// the output of commands that exited nonzero
    Failed,
}

/// a match counted in lines from the top of the history, so it stays put while output
/// scrolls the screen
type AbsMatch = RangeInclusive<(usize, Column)>;

pub(super) struct SearchState {
    regex: RegexSearch,
    /// every match from the top of the history down, from the last scan
    matches: Vec<AbsMatch>,
    /// index into `matches`
    active: Option<usize>,
    scope: SearchScope,
}

/// match count and the place of the current one, for the search bar
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SearchStatus {
    /// 0 based index of the current match
    pub active: Option<usize>,
    pub count: usize,
    /// true when counting stopped at the limit
    pub capped: bool,
}

/// regex for a query, the error tells why it can't be searched
pub fn search_pattern(query: &SearchQuery) -> Result<String, String> {
    if query.text.is_empty() {
        return Err("empty query".into());
    }
    let mut pattern = if query.regex {
        query.text.clone()
    } else {
        escape(&query.text)
    };
    if query.whole_word {
        // the lazy dfa only knows ascii word boundaries
        pattern = format!(r"(?-u:\b)(?:{pattern})(?-u:\b)");
    }
    // alacritty already ignores case unless the query has an upper case letter
    Ok(match query.case {
        SearchCase::Smart => pattern,
        SearchCase::Sensitive => format!("(?-i){pattern}"),
        SearchCase::Insensitive => format!("(?i){pattern}"),
    })
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if r"\.+*?()|[]{}^$#&-~".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn to_abs(point: AlacPoint, history: usize) -> (usize, Column) {
    (
        (point.line.0 + history as i32).max(0) as usize,
        point.column,
    )
}

fn to_grid((line, column): (usize, Column), history: usize) -> AlacPoint {
    AlacPoint::new(Line(line as i32 - history as i32), column)
}

/// true when grid line `line` is in one of `ranges`, every line counts without ranges
fn in_ranges(line: Line, ranges: Option<&[RangeInclusive<Line>]>) -> bool {
    ranges.is_none_or(|ranges| ranges.iter().any(|range| range.contains(&line)))
}

/// every match in `term` starting in `ranges`, top to bottom, cut at the limit
fn scan(
    term: &Term<ZedListener>,
    regex: &mut RegexSearch,
    ranges: Option<&[RangeInclusive<Line>]>,
) -> Vec<AbsMatch> {
    let history = term.grid().history_size();
    let start = AlacPoint::new(term.topmost_line(), Column(0));
    let end = AlacPoint::new(term.bottommost_line(), term.last_column());
    // nothing to look at, and an empty range list would otherwise scan the whole history
    if ranges.is_some_and(|ranges| ranges.is_empty()) {
        return Vec::new();
    }
    RegexIter::new(
        start,
        end,
        alacritty_terminal::index::Direction::Right,
        term,
        regex,
    )
    .filter(|m| in_ranges(m.start().line, ranges))
    .take(MAX_MATCHES)
    .map(|m| to_abs(*m.start(), history)..=to_abs(*m.end(), history))
    .collect()
}

/// one match of a search over every tab, its lines counted from the top of the history
#[derive(Clone, Debug, PartialEq)]
pub struct FoundLine {
    pub start: (usize, Column),
    pub end: (usize, Column),
    /// the line the match starts on
    pub text: String,
    /// chars of `text` the match covers
    pub highlight: Range<usize>,
}

/// up to `cap` matches in `term` starting in `ranges`, the newest lines first
fn find_lines<T: alacritty_terminal::event::EventListener>(
    term: &Term<T>,
    regex: &mut RegexSearch,
    cap: usize,
    ranges: Option<&[RangeInclusive<Line>]>,
) -> Vec<FoundLine> {
    let history = term.grid().history_size();
    let top = AlacPoint::new(term.topmost_line(), Column(0));
    let bottom = AlacPoint::new(term.bottommost_line(), term.last_column());
    let direction = alacritty_terminal::index::Direction::Left;
    if ranges.is_some_and(|ranges| ranges.is_empty()) {
        return Vec::new();
    }
    RegexIter::new(bottom, top, direction, term, regex)
        .filter(|m| in_ranges(m.start().line, ranges))
        .take(cap)
        .map(|m| {
            let (start, end) = (*m.start(), *m.end());
            let line_end = AlacPoint::new(start.line, term.last_column());
            let before = match start.column.0 {
                0 => String::new(),
                column => term.bounds_to_string(
                    AlacPoint::new(start.line, Column(0)),
                    AlacPoint::new(start.line, Column(column - 1)),
                ),
            };
            let matched = term.bounds_to_string(start, end.min(line_end));
            let text = term.bounds_to_string(AlacPoint::new(start.line, Column(0)), line_end);
            let first = before.chars().count();
            FoundLine {
                start: to_abs(start, history),
                end: to_abs(end, history),
                text: text.trim_end().to_string(),
                highlight: first..first + matched.trim_end_matches('\n').chars().count(),
            }
        })
        .collect()
}

/// matches in the visible lines, for highlighting
pub(super) fn visible_matches(term: &Term<ZedListener>, search: &mut SearchState) -> Vec<Match> {
    let offset = term.grid().display_offset() as i32;
    let start = AlacPoint::new(Line(-offset), Column(0));
    let end = AlacPoint::new(
        Line(-offset + term.screen_lines() as i32 - 1),
        term.last_column(),
    );
    let direction = alacritty_terminal::index::Direction::Right;
    let found = RegexIter::new(start, end, direction, term, &mut search.regex).take(MAX_MATCHES);
    if search.scope == SearchScope::All {
        return found.collect();
    }
    // a narrower scope lights only what its scan kept, not every match on screen
    let history = term.grid().history_size();
    let kept: std::collections::HashSet<(usize, usize)> = search
        .matches
        .iter()
        .map(|m| (m.start().0, m.start().1.0))
        .collect();
    found
        .filter(|m| {
            let (line, column) = to_abs(*m.start(), history);
            kept.contains(&(line, column.0))
        })
        .collect()
}

/// the current match in grid points, none when there is none
pub(super) fn active_match(term: &Term<ZedListener>, search: &SearchState) -> Option<Match> {
    let history = term.grid().history_size();
    let active = search.matches.get(search.active?)?;
    Some(to_grid(*active.start(), history)..=to_grid(*active.end(), history))
}

impl Terminal {
    /// output lines a search in `scope` looks at, none for every line
    fn scope_lines(&mut self, scope: SearchScope) -> Option<Vec<RangeInclusive<Line>>> {
        let ids: Vec<u64> = match scope {
            SearchScope::All => return None,
            SearchScope::Block(id) => vec![id],
            SearchScope::Failed => self
                .blocks()
                .iter()
                .filter(|block| block.exit.is_some_and(|exit| exit != 0))
                .map(|block| block.id)
                .collect(),
        };
        Some(
            ids.into_iter()
                .filter_map(|id| self.output_range(id))
                .map(|(start, end)| start.line..=end.line)
                .collect(),
        )
    }

    /// block the "this block" scope searches: the one under `mouse`, else the first one
    /// shown from the top of the view, else the newest
    pub fn block_for_search(&mut self, mouse: Option<gpui::Point<gpui::Pixels>>) -> Option<u64> {
        self.blocks();
        if let Some(block) = mouse.and_then(|mouse| self.block_at(self.mouse_point(mouse).0.line)) {
            return Some(block);
        }
        let top = -(self.last_content.display_offset as i32);
        let lines = self.last_content.terminal_bounds.num_lines() as i32;
        (top..top + lines)
            .find_map(|line| self.block_at(Line(line)))
            .or_else(|| self.blocks.list.back().map(|block| block.id))
    }

    /// a search of this terminal's history for `pattern` to run on another thread, it holds
    /// the terminal's lock only while it scans. none when the pattern is broken
    pub fn find_task(
        &mut self,
        pattern: &str,
        cap: usize,
        scope: SearchScope,
    ) -> Option<impl FnOnce() -> Vec<FoundLine> + Send + use<>> {
        let mut regex = RegexSearch::new(pattern).ok()?;
        // the history of a sleeping tab is searched too
        self.wake();
        let ranges = self.scope_lines(scope);
        let term: Arc<FairMutex<Term<ZedListener>>> = self.term.clone();
        // lines move as output scrolls, so the ranges are only good while nothing printed
        Some(move || find_lines(&term.lock_unfair(), &mut regex, cap, ranges.as_deref()))
    }

    /// select a match found by `find_task` and scroll to it, false when it left the history
    pub fn select_found(&mut self, found: &FoundLine) -> bool {
        let mut term = self.term.lock();
        let history = term.grid().history_size();
        let (start, end) = (to_grid(found.start, history), to_grid(found.end, history));
        if start.line < term.topmost_line() || end.line > term.bottommost_line() {
            return false;
        }
        let mut selection = alacritty_terminal::selection::Selection::new(
            alacritty_terminal::selection::SelectionType::Simple,
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

    /// look for `query`, picking the match closest above the bottom of the view.
    /// the error tells why the query can't be searched, like a broken regex
    pub fn search(&mut self, query: &SearchQuery) -> Result<SearchStatus, String> {
        let regex = search_pattern(query)
            .and_then(|pattern| RegexSearch::new(&pattern).map_err(|error| error.to_string()));
        // stale highlights of the last good query would look like matches of this one
        let mut regex = regex.inspect_err(|_| self.end_search())?;
        let ranges = self.scope_lines(query.scope);
        let term = self.term.lock_unfair();
        let matches = scan(&term, &mut regex, ranges.as_deref());
        let history = term.grid().history_size();
        let bottom = to_abs(
            AlacPoint::new(
                Line(term.screen_lines() as i32 - 1 - term.grid().display_offset() as i32),
                term.last_column(),
            ),
            history,
        );
        drop(term);
        // the last match that starts at or above the bottom of the view, else the first one
        let active = matches
            .iter()
            .rposition(|m| *m.start() <= bottom)
            .or((!matches.is_empty()).then_some(0));
        self.search = Some(SearchState {
            regex,
            matches,
            active,
            scope: query.scope,
        });
        self.reveal_active_match();
        self.dirty.store(true, Ordering::Release);
        Ok(self.search_status())
    }

    /// move to the match above the current one, or below it, wrapping around at the ends
    pub fn search_next(&mut self, up: bool) -> SearchStatus {
        let Some(scope) = self.search.as_ref().map(|search| search.scope) else {
            return SearchStatus::default();
        };
        let ranges = self.scope_lines(scope);
        let Some(search) = self.search.as_mut() else {
            return SearchStatus::default();
        };
        let term = self.term.lock_unfair();
        let history = term.grid().history_size();
        let current = search
            .active
            .and_then(|ix| search.matches.get(ix))
            .map(|m| to_grid(*m.start(), history));
        // output may have added or scrolled away matches since the last scan
        search.matches = scan(&term, &mut search.regex, ranges.as_deref());
        drop(term);
        let count = search.matches.len();
        search.active = match current {
            _ if count == 0 => None,
            None => Some(if up { count - 1 } else { 0 }),
            Some(current) => {
                let current = to_abs(current, history);
                Some(if up {
                    search
                        .matches
                        .iter()
                        .rposition(|m| *m.start() < current)
                        .unwrap_or(count - 1)
                } else {
                    search
                        .matches
                        .iter()
                        .position(|m| *m.start() > current)
                        .unwrap_or(0)
                })
            }
        };
        self.reveal_active_match();
        self.dirty.store(true, Ordering::Release);
        self.search_status()
    }

    /// stop searching and drop the highlights
    pub fn end_search(&mut self) {
        if self.search.take().is_some() {
            self.dirty.store(true, Ordering::Release);
        }
    }

    /// match count and the current match of the running search
    pub fn search_status(&self) -> SearchStatus {
        let Some(search) = &self.search else {
            return SearchStatus::default();
        };
        SearchStatus {
            active: search.active,
            count: search.matches.len(),
            capped: search.matches.len() >= MAX_MATCHES,
        }
    }

    /// true while a search is running
    #[cfg(test)]
    pub(crate) fn searching(&self) -> bool {
        self.search.is_some()
    }

    // scroll so the current match is on screen, centered when it was off screen
    fn reveal_active_match(&mut self) {
        let Some(search) = &self.search else {
            return;
        };
        let Some(active) = search.active.and_then(|ix| search.matches.get(ix)) else {
            return;
        };
        let mut term = self.term.lock_unfair();
        let history = term.grid().history_size();
        let lines = term.screen_lines();
        let offset = term.grid().display_offset();
        // vi mode carries on from the match, like `/` in vim
        if term.mode().contains(TermMode::VI) {
            let point = to_grid(*active.start(), history);
            term.vi_mode_cursor.point = point;
        }
        drop(term);
        // lines from the bottom of the history the match starts at
        let from_bottom = (history + lines).saturating_sub(active.start().0 + 1);
        let top_shown = offset + lines - 1;
        if from_bottom < offset || from_bottom > top_shown {
            let target = from_bottom.saturating_sub(lines / 2).min(history);
            self.scroll_to(target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SearchQuery, search_pattern};

    #[test]
    fn find_lines_lists_newest_matches_first_up_to_the_cap() {
        use alacritty_terminal::{
            Term,
            event::VoidListener,
            index::{Column, Line},
            term::{Config, search::RegexSearch, test::TermSize},
            vte::ansi::Processor,
        };
        let mut term = Term::new(Config::default(), &TermSize::new(40, 4), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(
            &mut term,
            b"alpha one\r\nbeta\r\nsay alpha two\r\nalpha three",
        );
        let mut regex = RegexSearch::new("alpha").unwrap();
        let found = super::find_lines(&term, &mut regex, 2, None);
        let rows: Vec<(&str, std::ops::Range<usize>)> = found
            .iter()
            .map(|found| (found.text.as_str(), found.highlight.clone()))
            .collect();
        assert_eq!(rows, [("alpha three", 0..5), ("say alpha two", 4..9)]);
        assert_eq!(found[1].start, (2, Column(4)));
        assert_eq!(found[1].end, (2, Column(8)));
        assert_eq!(super::find_lines(&term, &mut regex, 10, None).len(), 3);
        // only matches starting in the given lines count
        let ranges = [Line(0)..=Line(0), Line(3)..=Line(3)];
        let found = super::find_lines(&term, &mut regex, 10, Some(&ranges));
        let texts: Vec<&str> = found.iter().map(|found| found.text.as_str()).collect();
        assert_eq!(texts, ["alpha three", "alpha one"]);
        assert!(super::find_lines(&term, &mut regex, 10, Some(&[])).is_empty());
    }

    #[test]
    fn ranges_filter_lines() {
        use alacritty_terminal::index::Line;
        let ranges = [Line(-5)..=Line(-3), Line(2)..=Line(2)];
        assert!(super::in_ranges(Line(7), None));
        assert!(super::in_ranges(Line(-4), Some(&ranges)));
        assert!(super::in_ranges(Line(2), Some(&ranges)));
        assert!(!super::in_ranges(Line(-2), Some(&ranges)));
        assert!(!super::in_ranges(Line(0), Some(&[])));
    }
    use crate::settings::SearchCase;

    fn query(text: &str, regex: bool, whole_word: bool, case: SearchCase) -> SearchQuery {
        SearchQuery {
            text: text.into(),
            regex,
            whole_word,
            case,
            scope: Default::default(),
        }
    }

    #[test]
    fn plain_queries_are_escaped_and_options_wrap_the_pattern() {
        assert_eq!(
            search_pattern(&query("a.b(c)", false, false, SearchCase::Smart)).unwrap(),
            r"a\.b\(c\)"
        );
        assert_eq!(
            search_pattern(&query("a.b", true, false, SearchCase::Smart)).unwrap(),
            "a.b"
        );
        assert_eq!(
            search_pattern(&query("word", false, true, SearchCase::Sensitive)).unwrap(),
            r"(?-i)(?-u:\b)(?:word)(?-u:\b)"
        );
        assert_eq!(
            search_pattern(&query("Word", false, false, SearchCase::Insensitive)).unwrap(),
            "(?i)Word"
        );
        assert!(search_pattern(&query("", false, false, SearchCase::Smart)).is_err());
    }

    #[test]
    fn scopes_limit_search_to_block_output() {
        use crate::{
            settings::{Shell, TerminalSettings},
            terminal::tests::{profile, spawn_with, wait_for_text},
        };
        let script = concat!(
            "printf '\\033]133;A\\007$ \\033]133;B\\007\\033]133;E;ok\\007\\n\\033]133;C\\007'; ",
            "printf 'foo\\n\\033]133;D;0\\007'; ",
            "printf '\\033]133;A\\007$ \\033]133;B\\007\\033]133;E;bad\\007\\n\\033]133;C\\007'; ",
            "printf 'foo\\nfoo\\n\\033]133;D;1\\007'; ",
            "echo scope_ready; sleep 5"
        );
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "scope_ready");
        terminal.handle_scan_events();
        let ids: Vec<u64> = terminal.blocks().iter().map(|block| block.id).collect();
        assert_eq!(ids.len(), 2);
        let mut search = |scope| {
            let query = SearchQuery {
                scope,
                ..query("foo", false, false, SearchCase::Smart)
            };
            terminal.search(&query).unwrap().count
        };
        assert_eq!(search(super::SearchScope::All), 3);
        assert_eq!(search(super::SearchScope::Failed), 2);

        assert_eq!(search(super::SearchScope::Block(ids[0])), 1);
        assert_eq!(search(super::SearchScope::Block(u64::MAX)), 0);
        // the scope holds while walking matches
        assert_eq!(terminal.search_next(true).count, 0);
        // only the scope's matches light up on screen
        let failed = SearchQuery {
            scope: super::SearchScope::Failed,
            ..query("foo", false, false, SearchCase::Smart)
        };
        terminal.search(&failed).unwrap();
        terminal.sync();
        assert_eq!(terminal.last_content.search_matches.len(), 2);
        let pattern = search_pattern(&query("foo", false, false, SearchCase::Smart)).unwrap();
        let task = terminal
            .find_task(&pattern, 10, super::SearchScope::Failed)
            .unwrap();
        assert_eq!(task().len(), 2);
    }
}
