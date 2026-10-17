//! grid of live previews of every tab, typing filters them by title, arrows or a click pick
//! one. previews are the text of each terminal's screen in a tiny font

use gpui::{
    App, Context, DismissEvent, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, Subscription, Window, div, prelude::*, px, rems,
};

use crate::{
    settings::Settings,
    terminal::{Event, Terminal},
    theme::Theme,
    ui::text_input::{Changed, Left, Right, TextInput},
};

// font size of the preview text, small enough for a whole screen to fit a cell
const PREVIEW_FONT_SIZE: f32 = 5.;

/// columns and rows of a grid holding `count` cells, as square as it gets with more columns
/// than rows on a wide window
pub fn grid(count: usize) -> (usize, usize) {
    if count == 0 {
        return (0, 0);
    }
    let columns = (count as f64).sqrt().ceil() as usize;
    (columns, count.div_ceil(columns))
}

/// one tab in the overview
pub struct OverviewTab {
    pub id: EntityId,
    pub title: String,
    pub terminal: Entity<Terminal>,
    pub active: bool,
}

/// emitted with the tab picked from the overview
pub struct OverviewPicked(pub EntityId);

pub struct TabOverview {
    input: Entity<TextInput>,
    tabs: Vec<OverviewTab>,
    /// indices into `tabs` whose title fits the query, in tab order
    shown: Vec<usize>,
    /// index into `shown`
    selected: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for TabOverview {}
impl EventEmitter<OverviewPicked> for TabOverview {}

impl TabOverview {
    /// overview of `tabs`, the active one selected
    pub fn new(tabs: Vec<OverviewTab>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("filter tabs...", cx));
        let mut subscriptions =
            vec![cx.subscribe(&input, |this: &mut Self, _, _: &Changed, cx| {
                this.filter(cx);
                cx.notify();
            })];
        // previews follow what the programs print
        subscriptions.extend(
            tabs.iter()
                .map(|tab| cx.subscribe(&tab.terminal, |_, _, _: &Event, cx| cx.notify())),
        );
        let selected = tabs.iter().position(|tab| tab.active).unwrap_or(0);
        Self {
            input,
            shown: (0..tabs.len()).collect(),
            tabs,
            selected,
            _subscriptions: subscriptions,
        }
    }

    fn filter(&mut self, cx: &App) {
        let query = self.input.read(cx).content().to_lowercase();
        self.shown = (0..self.tabs.len())
            .filter(|&ix| self.tabs[ix].title.to_lowercase().contains(&query))
            .collect();
        self.selected = 0;
    }

    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(&tab) = self.shown.get(ix) {
            cx.emit(OverviewPicked(self.tabs[tab].id));
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.shown.len();
        let (columns, _) = grid(count);
        let selected = self.selected;
        self.selected = match event.keystroke.key.as_str() {
            "escape" => {
                cx.emit(DismissEvent);
                return;
            }
            "enter" => {
                self.pick(selected, cx);
                return;
            }
            "up" => selected.checked_sub(columns).unwrap_or(selected),
            "down" => match selected + columns {
                below if below < count => below,
                _ => selected,
            },
            _ => return,
        };
        cx.stop_propagation();
        cx.notify();
    }

    /// screen lines of `terminal`, trailing blanks dropped
    fn screen_lines(terminal: &Terminal) -> Vec<String> {
        let content = &terminal.last_content;
        let mut lines: Vec<String> = Vec::new();
        let mut last = None;
        for cell in &content.cells {
            if last != Some(cell.point.line) {
                lines.push(String::new());
                last = Some(cell.point.line);
            }
            if let Some(line) = lines.last_mut() {
                line.push(cell.cell.c);
            }
        }
        for line in &mut lines {
            line.truncate(line.trim_end().len());
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }
}

impl Focusable for TabOverview {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for TabOverview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // a hidden tab has no view syncing it, a sleeping one stays asleep
        for &tab_ix in &self.shown {
            self.tabs[tab_ix].terminal.update(cx, |terminal, _| {
                if !terminal.asleep() {
                    terminal.sync();
                }
            });
        }
        let theme = Theme::get(cx);
        let font = Settings::get(cx).terminal.font.family.clone();
        let (columns, _) = grid(self.shown.len());
        let cells = self.shown.iter().enumerate().map(|(ix, &tab_ix)| {
            let tab = &self.tabs[tab_ix];
            let terminal = tab.terminal.read(cx);
            let colors = terminal.theme(cx);
            let lines = Self::screen_lines(terminal);
            let selected = ix == self.selected;
            div()
                .id(("overview-tab", ix))
                .debug_selector(move || format!("overview-tab-{ix}"))
                .flex()
                .flex_col()
                .gap_1()
                .p_1()
                .rounded_md()
                .border_2()
                .border_color(if selected { theme.cursor } else { theme.border })
                .bg(theme.tab_bar_background)
                .cursor_pointer()
                .child(
                    div()
                        .h(rems(9.))
                        .overflow_hidden()
                        .rounded_sm()
                        .p_1()
                        .bg(colors.terminal_background)
                        .text_color(colors.terminal_foreground)
                        .font_family(font.clone())
                        .text_size(px(PREVIEW_FONT_SIZE))
                        .line_height(px(PREVIEW_FONT_SIZE * 1.2))
                        .whitespace_nowrap()
                        .children(lines.into_iter().map(|line| div().child(line))),
                )
                .child(
                    div()
                        .px_1()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_color(if tab.active {
                            theme.text
                        } else {
                            theme.text_muted
                        })
                        .child(tab.title.clone()),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.pick(ix, cx)))
        });
        div()
            .id("tab-overview")
            .debug_selector(|| "tab-overview".into())
            .key_context("TabOverview")
            .on_key_down(cx.listener(Self::key_down))
            // the filter field binds left and right to its caret, the pick takes them first
            .capture_action(cx.listener(|this, _: &Left, _, cx| {
                this.selected = this.selected.saturating_sub(1);
                cx.stop_propagation();
                cx.notify();
            }))
            .capture_action(cx.listener(|this, _: &Right, _, cx| {
                this.selected = (this.selected + 1).min(this.shown.len().saturating_sub(1));
                cx.stop_propagation();
                cx.notify();
            }))
            .occlude()
            .absolute()
            .inset_0()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .bg(theme.tab_bar_background.opacity(0.95))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.tab_active_background)
                    .text_color(theme.text)
                    .child(self.input.clone()),
            )
            .child(
                div()
                    .id("overview-grid")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .grid()
                    .grid_cols(columns.max(1) as u16)
                    .gap_3()
                    .children(cells),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::grid;

    #[test]
    fn grids_are_about_square() {
        assert_eq!(grid(0), (0, 0));
        assert_eq!(grid(1), (1, 1));
        assert_eq!(grid(2), (2, 1));
        assert_eq!(grid(3), (2, 2));
        assert_eq!(grid(4), (2, 2));
        assert_eq!(grid(5), (3, 2));
        assert_eq!(grid(9), (3, 3));
        assert_eq!(grid(10), (4, 3));
    }
}
