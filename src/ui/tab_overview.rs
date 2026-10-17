//! grid of live previews of every tab, typing filters them by title, arrows or a click pick
//! one. previews are the text of each terminal's screen, scaled down to fit a cell of the grid

use gpui::{
    App, Context, DismissEvent, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, Pixels, Size, Subscription, Window, div, prelude::*, px,
};

use crate::{
    settings::Settings,
    terminal::{Event, Terminal},
    theme::Theme,
    ui::text_input::{Changed, Left, Right, TextInput},
};

// smallest a preview is scaled to, below it the text is only texture
const MIN_SCALE: f32 = 0.15;
// largest, so a few tabs still look like an overview and not like the tabs themselves
const MAX_SCALE: f32 = 0.6;

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

/// how much a terminal of `size` is scaled down to fit `room`, the space a grid cell leaves
/// for its preview, keeping its shape
pub fn preview_scale(size: Size<Pixels>, room: Size<Pixels>) -> f32 {
    if size.width <= px(0.) || size.height <= px(0.) {
        return MIN_SCALE;
    }
    let scale = (room.width / size.width).min(room.height / size.height);
    scale.clamp(MIN_SCALE, MAX_SCALE)
}

impl Focusable for TabOverview {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for TabOverview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // a hidden tab has no view syncing it, a sleeping one stays asleep
        for &tab_ix in &self.shown {
            self.tabs[tab_ix].terminal.update(cx, |terminal, _| {
                if !terminal.asleep() {
                    terminal.sync();
                }
            });
        }
        let theme = Theme::get(cx);
        let settings = Settings::get(cx);
        let font = settings.terminal.font.family.clone();
        let font_size = settings.terminal.font.size;
        let (columns, rows) = grid(self.shown.len());
        // the room a preview gets: the window less the padding, the filter field, the gaps and
        // per cell its border, padding and title, as laid out below
        let rem = window.rem_size();
        let viewport = window.viewport_size();
        let (columns_f, rows_f) = (columns.max(1) as f32, rows.max(1) as f32);
        let room = Size {
            width: (viewport.width - rem * (2. + 0.75 * (columns_f - 1.))) / columns_f
                - rem * 0.5
                - px(4.),
            height: (viewport.height - rem * (2. + 3. + 0.75 * rows_f)) / rows_f
                - rem * (0.75 + 1.5)
                - px(4.),
        };
        let cells = self.shown.iter().enumerate().map(|(ix, &tab_ix)| {
            let tab = &self.tabs[tab_ix];
            let terminal = tab.terminal.read(cx);
            let colors = terminal.theme(cx);
            let lines = Self::screen_lines(terminal);
            let bounds = terminal.last_content.terminal_bounds;
            let scale = preview_scale(bounds.bounds.size, room);
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
                    // the terminal's screen shrunk as a whole, so it keeps its shape
                    div()
                        .flex_none()
                        .w(bounds.bounds.size.width * scale)
                        .h(bounds.bounds.size.height * scale)
                        .overflow_hidden()
                        .rounded_sm()
                        .bg(colors.terminal_background)
                        .text_color(colors.terminal_foreground)
                        .font_family(font.clone())
                        .text_size(px(font_size * scale))
                        .line_height(bounds.line_height * scale)
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
                    // rows as tall as their cells, not stretched over the free room
                    .content_start()
                    .items_start()
                    .gap_3()
                    .children(cells),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Size, px};

    use super::{grid, preview_scale};

    #[test]
    fn previews_shrink_to_the_room_keeping_their_shape() {
        let terminal = Size {
            width: px(1000.),
            height: px(500.),
        };
        // the narrow side of the room decides
        let wide = Size {
            width: px(400.),
            height: px(400.),
        };
        assert_eq!(preview_scale(terminal, wide), 0.4);
        let low = Size {
            width: px(1000.),
            height: px(100.),
        };
        assert_eq!(preview_scale(terminal, low), 0.2);
        // never bigger than the cap, never smaller than the floor
        assert_eq!(
            preview_scale(
                terminal,
                Size {
                    width: px(5000.),
                    height: px(5000.)
                }
            ),
            0.6
        );
        assert_eq!(
            preview_scale(
                terminal,
                Size {
                    width: px(10.),
                    height: px(10.)
                }
            ),
            0.15
        );
        assert_eq!(preview_scale(Size::default(), wide), 0.15);
    }

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
