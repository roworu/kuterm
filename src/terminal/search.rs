//! regex search over scrollback and screen, matches are highlighted and walked one by one

use std::{ops::RangeInclusive, sync::atomic::Ordering};

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

/// every match in `term`, top to bottom, cut at the limit
fn scan(term: &Term<ZedListener>, regex: &mut RegexSearch) -> Vec<AbsMatch> {
    let history = term.grid().history_size();
    let start = AlacPoint::new(term.topmost_line(), Column(0));
    let end = AlacPoint::new(term.bottommost_line(), term.last_column());
    RegexIter::new(
        start,
        end,
        alacritty_terminal::index::Direction::Right,
        term,
        regex,
    )
    .take(MAX_MATCHES)
    .map(|m| to_abs(*m.start(), history)..=to_abs(*m.end(), history))
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
    RegexIter::new(start, end, direction, term, &mut search.regex)
        .take(MAX_MATCHES)
        .collect()
}

/// the current match in grid points, none when there is none
pub(super) fn active_match(term: &Term<ZedListener>, search: &SearchState) -> Option<Match> {
    let history = term.grid().history_size();
    let active = search.matches.get(search.active?)?;
    Some(to_grid(*active.start(), history)..=to_grid(*active.end(), history))
}

impl Terminal {
    /// look for `query`, picking the match closest above the bottom of the view.
    /// the error tells why the query can't be searched, like a broken regex
    pub fn search(&mut self, query: &SearchQuery) -> Result<SearchStatus, String> {
        let regex = search_pattern(query)
            .and_then(|pattern| RegexSearch::new(&pattern).map_err(|error| error.to_string()));
        // stale highlights of the last good query would look like matches of this one
        let mut regex = regex.inspect_err(|_| self.end_search())?;
        let term = self.term.lock_unfair();
        let matches = scan(&term, &mut regex);
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
        });
        self.reveal_active_match();
        self.dirty.store(true, Ordering::Release);
        Ok(self.search_status())
    }

    /// move to the match above the current one, or below it, wrapping around at the ends
    pub fn search_next(&mut self, up: bool) -> SearchStatus {
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
        search.matches = scan(&term, &mut search.regex);
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
    use crate::settings::SearchCase;

    fn query(text: &str, regex: bool, whole_word: bool, case: SearchCase) -> SearchQuery {
        SearchQuery {
            text: text.into(),
            regex,
            whole_word,
            case,
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
}
