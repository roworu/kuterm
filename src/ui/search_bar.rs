//! search bar over the top right corner of a terminal, walks the matches in its history

use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    SharedString, Subscription, Window, div, prelude::*, rems,
};

use crate::{
    settings::{SearchCase, Settings},
    terminal::{SearchQuery, SearchStatus, Terminal},
    theme::Theme,
    ui::text_input::{Changed, TextInput},
};

pub struct SearchBar {
    terminal: Entity<Terminal>,
    input: Entity<TextInput>,
    case: SearchCase,
    regex: bool,
    whole_word: bool,
    /// matches of the last search, or why the query can't be searched
    status: Result<SearchStatus, String>,
    _input_subscription: Subscription,
}

impl EventEmitter<DismissEvent> for SearchBar {}

impl SearchBar {
    /// empty bar searching `terminal`, options start from settings
    pub fn new(terminal: Entity<Terminal>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("search", cx));
        let _input_subscription = cx.subscribe(&input, |this: &mut Self, _, _: &Changed, cx| {
            this.search(cx)
        });
        let settings = Settings::get(cx).search;
        Self {
            terminal,
            input,
            case: settings.case_sensitive,
            regex: settings.regex,
            whole_word: settings.whole_word,
            status: Ok(SearchStatus::default()),
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
        }
    }

    /// matches of the last search, exposed for tests
    #[cfg(test)]
    pub(crate) fn status(&self) -> &Result<SearchStatus, String> {
        &self.status
    }

    // runs again whenever the query or an option changes
    fn search(&mut self, cx: &mut Context<Self>) {
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

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;
        match keystroke.key.as_str() {
            "escape" => cx.emit(DismissEvent),
            "enter" => self.step(!modifiers.shift, cx),
            "c" if modifiers.alt => self.toggle_case(cx),
            "r" if modifiers.alt => self.toggle_regex(cx),
            "w" if modifiers.alt => self.toggle_whole_word(cx),
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
