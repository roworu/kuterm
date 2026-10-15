//! links in the grid: osc 8 hyperlinks and text matched by regex patterns

use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::Dimensions,
    index::{Boundary, Column, Direction, Line, Point as AlacPoint},
    term::{
        cell::Flags,
        search::{Match, RegexIter, RegexSearch},
    },
};
use gpui::{Pixels, Point};

use super::{Terminal, builder::ZedListener};
use crate::settings::{PatternAction, TextPattern};

/// hyperlink ids kuterm gives cells it marks for itself, they are never shown as links
pub(crate) const MARKER_PREFIX: &str = "kuterm-";

// rows a url cut by hard wraps is followed over
const MAX_JOINED_ROWS: usize = 8;
// sides of boxes programs draw around text, a url inside goes on after the next row's border
const BORDERS: &[char] = &['│', '┃', '║', '┆', '┊', '╎', '|'];

/// text a pattern or a hyperlink covers
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// cells of the link in grid points, where it starts
    pub range: Match,
    /// cells of the rows it goes on in after a hard wrap, top to bottom
    pub rest: Vec<Match>,
    pub target: String,
    pub action: PatternAction,
    /// name of the pattern that found it, none for hyperlinks
    pub name: Option<String>,
}

/// patterns from settings with their regexes built, rebuilt when settings change
#[derive(Default)]
pub struct TextPatterns {
    source: Vec<TextPattern>,
    compiled: Vec<(RegexSearch, TextPattern)>,
}

impl Link {
    /// cells of every part of the link, top to bottom
    pub fn ranges(&self) -> impl Iterator<Item = &Match> {
        std::iter::once(&self.range).chain(&self.rest)
    }

    fn contains(&self, point: AlacPoint) -> bool {
        self.ranges().any(|range| range.contains(&point))
    }

    fn overlaps(&self, other: &Link) -> bool {
        self.ranges().any(|a| {
            other
                .ranges()
                .any(|b| a.start() <= b.end() && b.start() <= a.end())
        })
    }
}

fn is_url_char(c: char) -> bool {
    !c.is_whitespace()
        && c != '\0'
        && !BORDERS.contains(&c)
        && !matches!(
            c,
            '<' | '>' | '"' | '\'' | '`' | '(' | ')' | '{' | '}' | '[' | ']'
        )
}

/// cells after `end` up to where its row stops: the last column, or a box border after at
/// most one space of padding. none when other text comes first
fn tail_to_edge<T: EventListener>(term: &Term<T>, end: AlacPoint) -> Option<String> {
    let row = &term.grid()[end.line];
    let mut tail = String::new();
    let mut column = end.column.0 + 1;
    while column < term.columns() && is_url_char(row[Column(column)].c) {
        tail.push(row[Column(column)].c);
        column += 1;
    }
    let padding = (column..term.columns())
        .take_while(|&c| row[Column(c)].c == ' ')
        .count();
    let after = column + padding;
    let at_edge = column == term.columns()
        || (padding <= 1 && after < term.columns() && BORDERS.contains(&row[Column(after)].c));
    at_edge.then_some(tail)
}

/// true when `line` ends in url chars at its edge or right before a box border, so a url
/// may go on in the row below
fn ends_in_url_at_edge<T: EventListener>(term: &Term<T>, line: Line) -> bool {
    let row = &term.grid()[line];
    let Some(mut last) = (0..term.columns()).rev().find(|&c| row[Column(c)].c != ' ') else {
        return false;
    };
    if BORDERS.contains(&row[Column(last)].c) {
        // one space of padding may sit before the border
        last = match last.checked_sub(1) {
            Some(before) if row[Column(before)].c == ' ' => match before.checked_sub(1) {
                Some(before) => before,
                None => return false,
            },
            Some(before) => before,
            None => return false,
        };
    } else if last != term.columns() - 1 {
        return false;
    }
    is_url_char(row[Column(last)].c)
}

/// `link` followed into the rows below while it runs to the right edge or a box border and
/// the next row goes on with url chars, after its own border and padding in a box. a row
/// with more text after them is only joined when they look like a url, so a word that
/// happens to start the next row stays apart
fn join_cut_url<T: EventListener>(term: &Term<T>, link: &mut Link) {
    let start = *link.range.start();
    let first_row = &term.grid()[start.line];
    let border = (0..start.column.0)
        .rev()
        .find(|&c| BORDERS.contains(&first_row[Column(c)].c));
    let mut end = *link.range.end();
    for _ in 0..MAX_JOINED_ROWS {
        // soft wrapped rows are one line for the patterns already
        if end.line >= term.bottommost_line()
            || term.grid()[end.line][term.last_column()]
                .flags
                .contains(Flags::WRAPLINE)
        {
            break;
        }
        let Some(tail) = tail_to_edge(term, end) else {
            break;
        };
        let line = Line(end.line.0 + 1);
        let row = &term.grid()[line];
        let mut column = match border {
            Some(border) if !BORDERS.contains(&row[Column(border)].c) => break,
            Some(border) => {
                let mut column = border + 1;
                while column < term.columns() && row[Column(column)].c == ' ' {
                    column += 1;
                }
                column
            }
            None => 0,
        };
        let first = column;
        let mut run = String::new();
        while column < term.columns() && is_url_char(row[Column(column)].c) {
            run.push(row[Column(column)].c);
            column += 1;
        }
        let rest_blank = (column..term.columns()).all(|c| {
            let c = row[Column(c)].c;
            c == ' ' || BORDERS.contains(&c)
        });
        if run.is_empty() || !(rest_blank || run.contains(['/', '?', '&', '=', '#', '%'])) {
            break;
        }
        // punctuation at the end belongs to the sentence around the url
        let kept = run.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if kept.is_empty() {
            break;
        }
        // the url chars before the edge of the row above are part of it after all
        if !tail.is_empty() {
            let last = link.rest.last_mut().unwrap_or(&mut link.range);
            let tail_end = AlacPoint::new(end.line, end.column + tail.chars().count());
            *last = *last.start()..=tail_end;
            link.target.push_str(&tail);
        }
        link.target.push_str(kept);
        end = AlacPoint::new(line, Column(first + kept.chars().count() - 1));
        link.rest.push(AlacPoint::new(line, Column(first))..=end);
        if kept.len() < run.len() {
            break;
        }
    }
}

impl TextPatterns {
    /// rebuild the regexes when `patterns` differ from the ones built
    pub fn update(&mut self, patterns: &[TextPattern]) {
        if self.source == patterns {
            return;
        }
        self.source = patterns.to_vec();
        // settings already dropped invalid regexes
        self.compiled = patterns
            .iter()
            .filter_map(|pattern| Some((RegexSearch::new(&pattern.regex).ok()?, pattern.clone())))
            .collect();
    }
}

/// the osc 8 hyperlink at `point` with every cell around it that shares it
fn hyperlink_at(term: &Term<ZedListener>, point: AlacPoint) -> Option<Link> {
    let hyperlink = term.grid()[point].hyperlink()?;
    if hyperlink.id().starts_with(MARKER_PREFIX) {
        return None;
    }
    let same = |point: AlacPoint| term.grid()[point].hyperlink().as_ref() == Some(&hyperlink);
    let mut start = point;
    loop {
        let before = start.sub(term, Boundary::Grid, 1);
        if before == start || !same(before) {
            break;
        }
        start = before;
    }
    let mut end = point;
    loop {
        let after = end.add(term, Boundary::Grid, 1);
        if after == end || !same(after) {
            break;
        }
        end = after;
    }
    Some(Link {
        range: start..=end,
        rest: Vec::new(),
        target: hyperlink.uri().to_string(),
        action: PatternAction::Open,
        name: None,
    })
}

/// first pattern match covering `point`
fn pattern_at<T: EventListener>(
    term: &Term<T>,
    patterns: &mut TextPatterns,
    point: AlacPoint,
) -> Option<Link> {
    // a line wrapped over several rows is one line for the patterns
    let mut start = term.line_search_left(point);
    let end = term.line_search_right(point);
    // a url cut by hard wraps is found from the row it starts in
    for _ in 0..MAX_JOINED_ROWS {
        if start.line <= term.topmost_line() {
            break;
        }
        let above = start.line - 1;
        if !ends_in_url_at_edge(term, above) {
            break;
        }
        start = term.line_search_left(AlacPoint::new(above, Column(0)));
    }
    pattern_matches(term, patterns, start, end)
        .into_iter()
        .find(|link| link.contains(point))
}

/// matches of `patterns` between `start` and `end`, in pattern order, each pattern top to bottom
pub(super) fn pattern_matches<T: EventListener>(
    term: &Term<T>,
    patterns: &mut TextPatterns,
    start: AlacPoint,
    end: AlacPoint,
) -> Vec<Link> {
    let mut links = Vec::new();
    for (regex, pattern) in &mut patterns.compiled {
        for range in RegexIter::new(start, end, Direction::Right, term, regex) {
            let mut link = Link {
                target: term.bounds_to_string(*range.start(), *range.end()),
                range,
                rest: Vec::new(),
                action: pattern.action.clone(),
                name: pattern.name.clone(),
            };
            if link.target.contains("://") {
                join_cut_url(term, &mut link);
            }
            links.push(link);
        }
    }
    links
}

impl Terminal {
    /// link under a window position, an osc 8 hyperlink first, then the first pattern that
    /// matches there
    pub fn link_at(
        &mut self,
        position: Point<Pixels>,
        patterns: &mut TextPatterns,
    ) -> Option<Link> {
        let (point, _) = self.mouse_point(position);
        let term = self.term.lock_unfair();
        if let Some(link) = hyperlink_at(&term, point) {
            return Some(link);
        }
        pattern_at(&term, patterns, point)
    }

    /// matches of `patterns` on screen, top to bottom, overlapping ones dropped
    pub fn visible_matches(&mut self, patterns: &mut TextPatterns) -> Vec<Link> {
        let term = self.term.lock_unfair();
        let offset = term.grid().display_offset() as i32;
        let start = AlacPoint::new(Line(-offset), Column(0));
        let end = AlacPoint::new(
            Line(-offset + term.screen_lines() as i32 - 1),
            term.last_column(),
        );
        let mut links = pattern_matches(&term, patterns, start, end);
        drop(term);
        // earlier patterns win where matches overlap
        let mut kept: Vec<Link> = Vec::new();
        for link in links.drain(..) {
            if !kept.iter().any(|other| link.overlaps(other)) {
                kept.push(link);
            }
        }
        kept.sort_by_key(|link| *link.range.start());
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::TextPatterns;
    use crate::settings::{PatternAction, TextPattern};

    #[test]
    fn patterns_rebuild_only_when_settings_change() {
        let mut patterns = TextPatterns::default();
        let source = vec![TextPattern {
            name: None,
            regex: "a+".into(),
            action: PatternAction::Copy,
        }];
        patterns.update(&source);
        assert_eq!(patterns.compiled.len(), 1);
        patterns.compiled.clear();
        patterns.update(&source);
        assert!(
            patterns.compiled.is_empty(),
            "rebuilt for the same patterns"
        );
        patterns.update(&[]);
        assert!(patterns.source.is_empty());
    }

    /// a real 80 column alacritty term after printing `rows` joined by hard line breaks
    fn term_with(
        rows: &[&str],
    ) -> alacritty_terminal::Term<alacritty_terminal::event::VoidListener> {
        use alacritty_terminal::{
            Term,
            event::VoidListener,
            term::{Config, test::TermSize},
            vte::ansi::Processor,
        };
        let mut term = Term::new(Config::default(), &TermSize::new(80, 6), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, rows.join("\r\n").as_bytes());
        term
    }

    fn urls() -> TextPatterns {
        let mut patterns = TextPatterns::default();
        patterns.update(&[TextPattern {
            name: None,
            regex: r#"(https?|ftp|file)://[^\s<>"'`(){}\[\]]*[^\s<>"'`(){}\[\].,;:!?]"#.into(),
            action: PatternAction::Open,
        }]);
        patterns
    }

    fn at(line: i32, column: usize) -> alacritty_terminal::index::Point {
        use alacritty_terminal::index::{Column, Line, Point};
        Point::new(Line(line), Column(column))
    }

    #[test]
    fn urls_hard_wrapped_at_the_edge_are_joined() {
        // the url fills the row up to column 79 and goes on in the next one
        let first = format!("{}https://example.com/aaaaaaaa/b", "x ".repeat(25));
        assert_eq!(first.len(), 80);
        let term = term_with(&[&first, "bbbb/cc?d=1.", "next"]);
        let mut patterns = urls();
        let want = "https://example.com/aaaaaaaa/bbbbb/cc?d=1";
        for clicked in [at(0, 60), at(1, 3)] {
            let link = super::pattern_at(&term, &mut patterns, clicked).unwrap();
            assert_eq!(link.target, want);
            assert_eq!(link.range, at(0, 50)..=at(0, 79));
            // the full stop ends the sentence, not the url
            assert_eq!(link.rest, [at(1, 0)..=at(1, 10)]);
        }
        assert_eq!(super::pattern_at(&term, &mut patterns, at(1, 11)), None);
        assert_eq!(super::pattern_at(&term, &mut patterns, at(2, 1)), None);
    }

    #[test]
    fn urls_inside_a_box_go_on_after_the_border() {
        let term = term_with(&[
            "┌──────────────────────────────┐",
            "│ see https://example.com/long │",
            "│ /path/to/file.html           │",
            "└──────────────────────────────┘",
        ]);
        let mut patterns = urls();
        let link = super::pattern_at(&term, &mut patterns, at(2, 4)).unwrap();
        assert_eq!(link.target, "https://example.com/long/path/to/file.html");
        assert_eq!(link.rest, [at(2, 2)..=at(2, 19)]);
    }

    #[test]
    fn a_word_starting_the_next_row_is_no_part_of_the_url() {
        let first = format!("{}https://example.com/end", "x".repeat(57));
        assert_eq!(first.len(), 80);
        let term = term_with(&[&first, "unrelated words follow"]);
        let link = super::pattern_at(&term, &mut urls(), at(0, 70)).unwrap();
        assert_eq!(link.target, "https://example.com/end");
        assert!(link.rest.is_empty());
        assert_eq!(super::pattern_at(&term, &mut urls(), at(1, 2)), None);
    }

    use gpui::{point, px};

    use crate::{
        settings::{Settings, Shell, TerminalSettings},
        terminal::tests::{profile, screen_text, spawn_with, wait_for_text},
    };

    /// window position of a cell of the 80x24 test grid, 10 by 20 pixels each
    fn cell(column: usize, line: usize) -> gpui::Point<gpui::Pixels> {
        point(px(column as f32 * 10. + 5.), px(line as f32 * 20. + 10.))
    }

    fn row_of(text: &str, needle: &str) -> usize {
        text.lines()
            .position(|line| line.starts_with(needle))
            .unwrap_or_else(|| panic!("no line starts with {needle:?}:\n{text}"))
    }

    #[test]
    fn link_at_finds_urls_and_hyperlinks_but_not_markers() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                concat!(
                    "printf 'see https://example.com/a_b. done\\n';",
                    "printf '\\033]8;;http://osc.example/x\\033\\\\linked\\033]8;;\\033\\\\ text\\n';",
                    "printf '\\033]8;id=kuterm-mark;kuterm:x\\033\\\\marked\\033]8;;\\033\\\\\\n';",
                    "echo links_ready; sleep 5"
                )
                .into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "links_ready");
        let text = screen_text(terminal);
        let mut patterns = TextPatterns::default();
        patterns.update(&Settings::default().links.patterns);

        let url = row_of(&text, "see ");
        let link = terminal.link_at(cell(10, url), &mut patterns).unwrap();
        // the full stop after the url is not part of it
        assert_eq!(link.target, "https://example.com/a_b");
        assert_eq!(
            (link.range.start().column.0, link.range.end().column.0),
            (4, 26)
        );
        assert_eq!(link.action, PatternAction::Open);
        assert_eq!(terminal.link_at(cell(1, url), &mut patterns), None);

        let osc = row_of(&text, "linked");
        let link = terminal.link_at(cell(2, osc), &mut patterns).unwrap();
        assert_eq!(link.target, "http://osc.example/x");
        assert_eq!(
            (link.range.start().column.0, link.range.end().column.0),
            (0, 5)
        );
        assert_eq!(terminal.link_at(cell(8, osc), &mut patterns), None);

        let marked = row_of(&text, "marked");
        assert_eq!(terminal.link_at(cell(1, marked), &mut patterns), None);
    }

    #[test]
    fn visible_matches_keep_the_first_pattern_where_they_overlap() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "echo 'go https://x.io/abc then /tmp/file at 1a2b3c4d'; echo hints_ready; sleep 5"
                    .into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "hints_ready");
        let mut patterns = TextPatterns::default();
        patterns.update(&Settings::default().hints.patterns);
        let found: Vec<(Option<String>, String)> = terminal
            .visible_matches(&mut patterns)
            .into_iter()
            .map(|link| (link.name, link.target))
            .collect();
        assert_eq!(
            found,
            [
                (Some("url".into()), "https://x.io/abc".into()),
                (Some("path".into()), "/tmp/file".into()),
                (Some("hash".into()), "1a2b3c4d".into()),
            ]
        );
    }
}
