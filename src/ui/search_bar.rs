//! search bar over the top right corner of a terminal, walks the matches in its history

use std::time::Duration;

use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    SharedString, Subscription, Task, Window, div, prelude::*, rems,
};

use crate::{
    settings::{SearchCase, Settings},
    terminal::{SearchQuery, SearchScope, SearchStatus, Terminal},
    theme::Theme,
    ui::text_input::{Changed, TextInput},
};

// a long history takes a while to scan, so typing searches once it pauses
pub(crate) const SEARCH_DELAY: Duration = Duration::from_millis(100);

pub struct SearchBar {
    terminal: Entity<Terminal>,
    input: Entity<TextInput>,
    case: SearchCase,
    regex: bool,
    whole_word: bool,
    scope: SearchScope,
    /// block "this block" scope searches, picked when the bar opened
    block: Option<u64>,
    /// matches of the last search, or why the query can't be searched
    status: Result<SearchStatus, String>,
    /// the query changed and was not searched yet
    search_due: bool,
    _search_task: Task<()>,
    _input_subscription: Subscription,
}

impl EventEmitter<DismissEvent> for SearchBar {}

impl SearchBar {
    /// empty bar searching `terminal`, options start from settings. `block` is the one the
    /// "this block" scope searches
    pub fn new(terminal: Entity<Terminal>, block: Option<u64>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("search", cx));
        let _input_subscription = cx.subscribe(&input, |this: &mut Self, _, _: &Changed, cx| {
            this.search_due = true;
            this._search_task = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SEARCH_DELAY).await;
                this.update(cx, |this, cx| this.search(cx)).ok();
            });
        });
        let settings = Settings::get(cx).search;
        Self {
            terminal,
            input,
            case: settings.case_sensitive,
            regex: settings.regex,
            whole_word: settings.whole_word,
            scope: SearchScope::All,
            block,
            status: Ok(SearchStatus::default()),
            search_due: false,
            _search_task: Task::ready(()),
            _input_subscription,
        }
    }

    /// what the bar looks for right now
    pub fn query(&self, cx: &App) -> SearchQuery {
        SearchQuery {
            text: self.input.read(cx).content().to_string(),
            regex: self.regex,
            whole_word: self.whole_word,
            case: self.case,
            scope: self.scope,
        }
    }

    /// matches of the last search, exposed for tests
    #[cfg(test)]
    pub(crate) fn status(&self) -> &Result<SearchStatus, String> {
        &self.status
    }

    // runs again whenever the query or an option changes
    fn search(&mut self, cx: &mut Context<Self>) {
        self.search_due = false;
        let query = self.query(cx);
        self.status = if query.text.is_empty() {
            self.terminal
                .update(cx, |terminal, _| terminal.end_search());
            Ok(SearchStatus::default())
        } else {
            self.terminal
                .update(cx, |terminal, _| terminal.search(&query))
        };
        // the terminal view around the bar redraws with it
        cx.notify();
    }

    /// go to the match above the current one, or below it
    pub fn step(&mut self, up: bool, cx: &mut Context<Self>) {
        // walking right after typing walks the matches of what was typed
        if self.search_due {
            self.search(cx);
        }
        if self.status.is_err() {
            return;
        }
        let status = self
            .terminal
            .update(cx, |terminal, _| terminal.search_next(up));
        self.status = Ok(status);
        cx.notify();
    }

    fn toggle_case(&mut self, cx: &mut Context<Self>) {
        // smart already ignores case for lower case queries, so it flips to always
        self.case = match self.case {
            SearchCase::Sensitive => SearchCase::Insensitive,
            SearchCase::Smart | SearchCase::Insensitive => SearchCase::Sensitive,
        };
        self.search(cx);
    }

    fn toggle_regex(&mut self, cx: &mut Context<Self>) {
        self.regex = !self.regex;
        self.search(cx);
    }

    fn toggle_whole_word(&mut self, cx: &mut Context<Self>) {
        self.whole_word = !self.whole_word;
        self.search(cx);
    }

    /// search everything, then the block picked when the bar opened, then failed commands
    pub fn cycle_scope(&mut self, cx: &mut Context<Self>) {
        self.scope = next_scope(self.scope, self.block);
        self.search(cx);
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;
        match keystroke.key.as_str() {
            "escape" => cx.emit(DismissEvent),
            "enter" => self.step(!modifiers.shift, cx),
            "up" => self.step(true, cx),
            "down" => self.step(false, cx),
            "c" if modifiers.alt => self.toggle_case(cx),
            "r" if modifiers.alt => self.toggle_regex(cx),
            "w" if modifiers.alt => self.toggle_whole_word(cx),
            "s" if modifiers.alt => self.cycle_scope(cx),
            // the terminal below never gets keys typed into the bar
            _ => return,
        }
        cx.stop_propagation();
    }

    fn status_text(&self) -> SharedString {
        match &self.status {
            Err(error) if self.regex => format!("bad regex: {error}").into(),
            Err(_) => SharedString::default(),
            Ok(status) if status.count == 0 => "no matches".into(),
            Ok(status) => {
                let count = if status.capped {
                    format!("{}+", status.count)
                } else {
                    status.count.to_string()
                };
                match status.active {
                    Some(active) => format!("{} of {count}", active + 1).into(),
                    None => count.into(),
                }
            }
        }
    }
}

impl Focusable for SearchBar {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for SearchBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let empty = self.input.read(cx).content().is_empty();
        let toggle = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px_1()
                .rounded_sm()
                .border_1()
                .border_color(if on { theme.text } else { theme.border })
                .text_color(if on { theme.text } else { theme.text_muted })
                .hover(|button| button.bg(theme.tab_active_background))
                .child(label)
        };
        let failed = self.status.is_err() && !empty;
        div()
            .id("search-bar")
            .debug_selector(|| "search-bar".into())
            .key_context("Search")
            .on_key_down(cx.listener(Self::key_down))
            // keeps clicks from reaching the terminal below
            .occlude()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .bg(theme.tab_bar_background)
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .shadow_lg()
            .text_color(theme.text)
            .child(div().w(rems(14.)).child(self.input.clone()))
            .child(
                div()
                    .debug_selector(|| "search-status".into())
                    .min_w(rems(5.))
                    .whitespace_nowrap()
                    .text_color(if failed {
                        theme.danger_button
                    } else {
                        theme.text_muted
                    })
                    .when(!empty, |status| status.child(self.status_text())),
            )
            .child(
                toggle("search-prev", "↑", false)
                    .on_click(cx.listener(|this, _, _, cx| this.step(true, cx))),
            )
            .child(
                toggle("search-next", "↓", false)
                    .on_click(cx.listener(|this, _, _, cx| this.step(false, cx))),
            )
            .child(
                toggle("search-case", "Aa", self.case == SearchCase::Sensitive)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_case(cx))),
            )
            .child(
                toggle("search-word", "ab", self.whole_word)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_whole_word(cx))),
            )
            .child(
                toggle("search-regex", ".*", self.regex)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_regex(cx))),
            )
            .child(
                toggle(
                    "search-scope",
                    scope_label(self.scope),
                    self.scope != SearchScope::All,
                )
                .on_click(cx.listener(|this, _, _, cx| this.cycle_scope(cx))),
            )
            .child(
                div()
                    .id("search-close")
                    .debug_selector(|| "search-close".into())
                    .px_1()
                    .rounded_sm()
                    .text_color(theme.text_muted)
                    .hover(|button| button.bg(theme.tab_active_background))
                    .child("×")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
            )
    }
}

/// scope after `scope`, skipping "this block" when there is none
fn next_scope(scope: SearchScope, block: Option<u64>) -> SearchScope {
    match (scope, block) {
        (SearchScope::All, Some(block)) => SearchScope::Block(block),
        (SearchScope::All, None) | (SearchScope::Block(_), _) => SearchScope::Failed,
        (SearchScope::Failed, _) => SearchScope::All,
    }
}

fn scope_label(scope: SearchScope) -> &'static str {
    match scope {
        SearchScope::All => "all",
        SearchScope::Block(_) => "block",
        SearchScope::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use gpui::{Entity, TestAppContext};

    use super::*;
    use crate::terminal::{TerminalBounds, TerminalBuilder};

    /// a bar over a real shell that printed "word words sword", with an empty query
    fn bar_over_words(cx: &mut TestAppContext) -> (Entity<SearchBar>, Entity<Terminal>) {
        let settings = Settings::default();
        cx.update(|cx| cx.set_global(settings.clone()));
        let builder = TerminalBuilder::new(
            &settings.terminal,
            settings.default_profile(),
            TerminalBounds::default(),
            0,
        )
        .expect("failed to spawn shell");
        let terminal = cx.new(|_| builder.into_terminal());
        // split by quotes, so the typed command never matches, only the printed line
        terminal.update(cx, |terminal, _| {
            terminal.type_text(b"printf '%s\\n' 'wo''rd wo''rds swo''rd'\r")
        });
        let bar = cx.new(|cx| SearchBar::new(terminal.clone(), None, cx));
        let deadline = Instant::now() + Duration::from_secs(15);
        while count_of(&bar, "word", cx) != Ok(3) {
            assert!(Instant::now() < deadline, "shell never printed");
            std::thread::sleep(Duration::from_millis(50));
        }
        (bar, terminal)
    }

    /// matches of `text` with the bar's current options
    fn count_of(
        bar: &Entity<SearchBar>,
        text: &str,
        cx: &mut TestAppContext,
    ) -> Result<usize, String> {
        bar.update(cx, |bar, cx| {
            bar.input
                .update(cx, |input, cx| input.set_content(text.to_string(), cx));
            bar.search(cx);
            bar.status.clone().map(|status| status.count)
        })
    }

    #[gpui::test]
    fn whole_word_skips_matches_inside_words(cx: &mut TestAppContext) {
        let (bar, _) = bar_over_words(cx);
        bar.update(cx, |bar, cx| bar.toggle_whole_word(cx));
        assert_eq!(count_of(&bar, "word", cx), Ok(1));
        bar.update(cx, |bar, cx| bar.toggle_whole_word(cx));
        assert_eq!(count_of(&bar, "word", cx), Ok(3));
    }

    #[gpui::test]
    fn case_flips_between_sensitive_and_insensitive(cx: &mut TestAppContext) {
        let (bar, _) = bar_over_words(cx);
        bar.update(cx, |bar, _| bar.case = SearchCase::Sensitive);
        assert_eq!(count_of(&bar, "word", cx), Ok(3));
        assert_eq!(count_of(&bar, "WORD", cx), Ok(0));
        bar.update(cx, |bar, cx| bar.toggle_case(cx));
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.case),
            SearchCase::Insensitive
        );
        assert_eq!(count_of(&bar, "WORD", cx), Ok(3));
    }

    #[gpui::test]
    fn emptied_query_ends_the_search(cx: &mut TestAppContext) {
        let (bar, terminal) = bar_over_words(cx);
        assert!(terminal.read_with(cx, |terminal, _| terminal.searching()));
        assert_eq!(count_of(&bar, "", cx), Ok(0));
        assert!(!terminal.read_with(cx, |terminal, _| terminal.searching()));
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status.clone()),
            Ok(SearchStatus::default())
        );
    }

    #[gpui::test]
    fn arrows_do_nothing_on_a_broken_regex(cx: &mut TestAppContext) {
        let (bar, _) = bar_over_words(cx);
        bar.update(cx, |bar, cx| bar.toggle_regex(cx));
        assert!(count_of(&bar, "wo(", cx).is_err());
        bar.update(cx, |bar, cx| bar.step(true, cx));
        let status = bar.read_with(cx, |bar, _| bar.status.clone());
        assert!(status.is_err(), "the error went away: {status:?}");
    }

    #[gpui::test]
    fn status_text_tells_count_place_and_cap(cx: &mut TestAppContext) {
        let (bar, _) = bar_over_words(cx);
        let text = |status: Result<SearchStatus, String>, regex: bool, cx: &mut TestAppContext| {
            bar.update(cx, |bar, _| {
                bar.status = status;
                bar.regex = regex;
                bar.status_text().to_string()
            })
        };
        let found = |active, count, capped| {
            Ok(SearchStatus {
                active,
                count,
                capped,
            })
        };
        assert_eq!(text(found(Some(1), 3, false), false, cx), "2 of 3");
        assert_eq!(text(found(None, 3, false), false, cx), "3");
        assert_eq!(text(found(None, 10_000, true), false, cx), "10000+");
        assert_eq!(text(found(Some(0), 10_000, true), false, cx), "1 of 10000+");
        assert_eq!(text(found(None, 0, false), false, cx), "no matches");
        assert_eq!(text(Err("oops".into()), true, cx), "bad regex: oops");
        // only a regex has an error worth showing
        assert_eq!(text(Err("oops".into()), false, cx), "");
    }

    #[test]
    fn scope_cycles_and_skips_missing_block() {
        assert_eq!(next_scope(SearchScope::All, Some(4)), SearchScope::Block(4));
        assert_eq!(
            next_scope(SearchScope::Block(4), Some(4)),
            SearchScope::Failed
        );
        assert_eq!(next_scope(SearchScope::Failed, Some(4)), SearchScope::All);
        assert_eq!(next_scope(SearchScope::All, None), SearchScope::Failed);
    }
}
