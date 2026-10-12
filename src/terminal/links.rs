//! links in the grid: osc 8 hyperlinks and text matched by regex patterns

use alacritty_terminal::{
    Term,
    grid::Dimensions,
    index::{Boundary, Column, Direction, Line, Point as AlacPoint},
    term::search::{Match, RegexIter, RegexSearch},
};
use gpui::{Pixels, Point};

use super::{Terminal, builder::ZedListener};
use crate::settings::{PatternAction, TextPattern};

/// hyperlink ids kuterm gives cells it marks for itself, they are never shown as links
pub(crate) const MARKER_PREFIX: &str = "kuterm-";

/// text a pattern or a hyperlink covers
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// cells of the link in grid points
    pub range: Match,
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
        target: hyperlink.uri().to_string(),
        action: PatternAction::Open,
        name: None,
    })
}

/// matches of `patterns` between `start` and `end`, in pattern order, each pattern top to bottom
pub(super) fn pattern_matches(
    term: &Term<ZedListener>,
    patterns: &mut TextPatterns,
    start: AlacPoint,
    end: AlacPoint,
) -> Vec<Link> {
    let mut links = Vec::new();
    for (regex, pattern) in &mut patterns.compiled {
        for range in RegexIter::new(start, end, Direction::Right, term, regex) {
            links.push(Link {
                target: term.bounds_to_string(*range.start(), *range.end()),
                range,
                action: pattern.action.clone(),
                name: pattern.name.clone(),
            });
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
        // a line wrapped over several rows is one line for the patterns
        let start = term.line_search_left(point);
        let end = term.line_search_right(point);
        pattern_matches(&term, patterns, start, end)
            .into_iter()
            .find(|link| link.range.contains(&point))
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
            let overlaps = kept.iter().any(|other| {
                link.range.start() <= other.range.end() && other.range.start() <= link.range.end()
            });
            if !overlaps {
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
