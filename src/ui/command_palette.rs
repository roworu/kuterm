//! command palette, the about page it opens and the close tab and paste dialogs, all float over
//! the terminal

use std::ops::Range;

use gpui::{
    App, Bounds, Context, DismissEvent, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, Pixels, ScrollHandle, SharedString, Subscription, Window, canvas, div, fill,
    point, prelude::*, px, rems, size,
};

use crate::{
    settings::{Command, Pins},
    terminal::paste_check,
    theme::Theme,
    ui::{
        text_input::{Changed, TextInput},
        workspace::RunNamedCommand,
    },
};

// thin rounded thumb, styled like the terminal scrollbar
const SCROLLBAR_WIDTH: f32 = 6.;
const SCROLLBAR_MIN_THUMB: f32 = 24.;
// tallest the command list grows before it scrolls
const LIST_MAX_HEIGHT: f32 = 20.;
// nerd font pin, replaced by the crossed out one while it is hovered to unpin
const PIN_ICON: &str = "\u{f08d}";
const UNPIN_ICON: &str = "\u{f00d}";

/// true when every query letter shows up in `text` in order, ignoring case and spaces
fn fuzzy_match(text: &str, query: &str) -> bool {
    let mut text = text.chars().flat_map(char::to_lowercase);
    query
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .all(|q| text.any(|ch| ch == q))
}

/// thumb for a scrolled list, `viewport` is the visible track and `max_offset` how far it scrolls
fn scrollbar_thumb(
    viewport: Bounds<Pixels>,
    offset: f32,
    max_offset: f32,
) -> Option<Bounds<Pixels>> {
    let height = f32::from(viewport.size.height);
    if max_offset <= 0. || height <= 0. {
        return None;
    }
    let content = height + max_offset;
    let thumb = (height * height / content)
        .max(SCROLLBAR_MIN_THUMB)
        .min(height);
    let free = height - thumb;
    // gpui offsets grow negative as the list scrolls down
    let progress = (-offset / max_offset).clamp(0., 1.);
    let top = viewport.origin.y + px(free * progress);
    let x = viewport.origin.x + viewport.size.width - px(SCROLLBAR_WIDTH);
    Some(Bounds::new(
        point(x, top),
        size(px(SCROLLBAR_WIDTH), px(thumb)),
    ))
}

/// keys bound to the command of this name with a keymap "run" entry, as gpui shows them
fn bound_key(name: &str, cx: &App) -> Option<String> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let action = RunNamedCommand(name.to_string());
    let binding = keymap.bindings_for_action(&action).last()?;
    Some(
        binding
            .keystrokes()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// floating box near the top of the window, shared by the palette and the about page
fn panel(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .w(rems(34.))
        .max_w_full()
        .bg(theme.tab_bar_background)
        .border_1()
        .border_color(theme.border)
        .rounded_md()
        .shadow_lg()
        .text_color(theme.text)
        .overflow_hidden()
}

/// list of commands filtered by typed text, emits the picked command
pub struct CommandPalette {
    input: Entity<TextInput>,
    commands: Vec<Command>,
    /// indices into `commands` matching the query, in display order
    matches: Vec<usize>,
    /// index into `matches`
    selected: usize,
    scroll: ScrollHandle,
    /// labels of recently run commands, most recent first, empty when disabled
    recent: Vec<String>,
    /// tab picker keeps the given order and has no pins
    is_tab_picker: bool,
    /// the list comes from outside for each query, like matches in every tab
    finder: bool,
    /// chars of each command's name to highlight, by index into `commands`
    highlights: Vec<Option<Range<usize>>>,
    _input_subscription: Subscription,
}

/// query typed into a palette opened with `finder`
pub struct FinderQuery(pub String);

/// command the selection moved to, for pickers that show a preview
pub struct Highlighted(pub Command);

impl EventEmitter<DismissEvent> for CommandPalette {}
impl EventEmitter<Command> for CommandPalette {}
impl EventEmitter<FinderQuery> for CommandPalette {}
impl EventEmitter<Highlighted> for CommandPalette {}

impl CommandPalette {
    /// palette listing `commands` with an empty query
    pub fn new(commands: Vec<Command>, recent: Vec<String>, cx: &mut Context<Self>) -> Self {
        Self::build(commands, recent, "execute a command...", false, cx)
    }

    /// palette listing one command per tab, in tab order
    pub fn tab_picker(tabs: Vec<Command>, cx: &mut Context<Self>) -> Self {
        Self::picker(tabs, "go to tab...", cx)
    }

    /// palette listing `items` in the given order, with no pins
    pub fn picker(
        items: Vec<Command>,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(items, Vec::new(), placeholder, true, cx)
    }

    /// palette whose list is set with `set_results` for each typed query
    pub fn finder(placeholder: &'static str, cx: &mut Context<Self>) -> Self {
        let mut palette = Self::build(Vec::new(), Vec::new(), placeholder, true, cx);
        palette.finder = true;
        palette
    }

    /// show `rows` in place of the list, each with the chars of its name to highlight
    pub fn set_results(
        &mut self,
        rows: Vec<(Command, Option<Range<usize>>)>,
        cx: &mut Context<Self>,
    ) {
        (self.commands, self.highlights) = rows.into_iter().unzip();
        self.update_matches(cx);
        self.select(0);
        cx.notify();
    }

    fn build(
        commands: Vec<Command>,
        recent: Vec<String>,
        placeholder: impl Into<SharedString>,
        is_tab_picker: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| TextInput::new(placeholder, cx));
        // a new query means a new best match
        let _input_subscription =
            cx.subscribe(&input, |this: &mut Self, input, _: &Changed, cx| {
                if this.finder {
                    cx.emit(FinderQuery(input.read(cx).content().to_string()));
                    return;
                }
                this.update_matches(cx);
                this.select(0);
                this.emit_highlighted(cx);
                cx.notify();
            });
        let mut palette = Self {
            input,
            commands,
            matches: Vec::new(),
            selected: 0,
            scroll: ScrollHandle::new(),
            recent,
            is_tab_picker,
            finder: false,
            highlights: Vec::new(),
            _input_subscription,
        };
        palette.update_matches(cx);
        palette
    }

    /// commands matching the typed query, in display order, exposed for tests
    #[cfg(test)]
    pub fn matches(&self) -> impl Iterator<Item = &Command> {
        self.matches.iter().map(|&ix| &self.commands[ix])
    }

    /// refilter by the query, pinned first, then recency, category and name
    fn update_matches(&mut self, cx: &App) {
        let query = self.input.read(cx).content();
        let pins = Pins::get(cx);
        let mut matches: Vec<usize> = (0..self.commands.len())
            .filter(|&ix| self.finder || fuzzy_match(&self.commands[ix].label(), query))
            .collect();
        // cached so each label is built once instead of on every comparison
        if !self.is_tab_picker {
            matches.sort_by_cached_key(|&ix| {
                let command = &self.commands[ix];
                let label = command.label();
                let recent = self.recent.iter().position(|recent| *recent == label);
                (
                    !pins.is_pinned(command),
                    recent.unwrap_or(usize::MAX),
                    // categorized commands come first, each group in name order
                    command.category.is_none(),
                    &command.category,
                    &command.name,
                )
            });
        }
        self.matches = matches;
    }

    /// flip the pin of a listed command, save it and keep that command selected
    fn toggle_pin(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(&toggled) = self.matches.get(ix) else {
            return;
        };
        let command = &self.commands[toggled];
        cx.update_global(|pins: &mut Pins, _| {
            pins.set(command, !pins.is_pinned(command));
            pins.save();
        });
        self.update_matches(cx);
        if let Some(ix) = self.matches.iter().position(|&m| m == toggled) {
            self.select(ix);
        }
        cx.notify();
    }

    fn select(&mut self, ix: usize) {
        self.selected = ix;
        self.scroll.scroll_to_item(ix);
    }

    fn emit_highlighted(&self, cx: &mut Context<Self>) {
        if let Some(&ix) = self.matches.get(self.selected) {
            cx.emit(Highlighted(self.commands[ix].clone()));
        }
    }

    /// typed query, exposed for tests
    #[cfg(test)]
    pub(crate) fn query<'a>(&self, cx: &'a App) -> &'a str {
        self.input.read(cx).content()
    }

    /// thin scrollbar pinned to the right of the command list
    fn render_scrollbar(&self, theme: &Theme) -> impl IntoElement {
        let scroll = self.scroll.clone();
        let color = theme.scrollbar;
        canvas(
            move |bounds, _, _| {
                scrollbar_thumb(
                    bounds,
                    f32::from(scroll.offset().y),
                    f32::from(scroll.max_offset().y),
                )
            },
            move |_, thumb, window, _| {
                if let Some(thumb) = thumb {
                    window.paint_quad(fill(thumb, color).corner_radii(thumb.size.width / 2.));
                }
            },
        )
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .w(px(SCROLLBAR_WIDTH))
    }

    fn confirm(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(&ix) = self.matches.get(ix) {
            cx.emit(self.commands[ix].clone());
        }
    }

    /// keys the text field leaves alone, moving the command list instead
    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => cx.emit(DismissEvent),
            "enter" => self.confirm(self.selected, cx),
            // arrows wrap around between the first and the last command
            "up" => self.select(
                self.selected
                    .checked_sub(1)
                    .unwrap_or(self.matches.len().saturating_sub(1)),
            ),
            "down" => self.select((self.selected + 1) % self.matches.len().max(1)),
            _ => return,
        }
        self.emit_highlighted(cx);
        cx.stop_propagation();
        cx.notify();
    }
}

impl Focusable for CommandPalette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let pins = Pins::get(cx);
        let items = self
            .matches
            .iter()
            .enumerate()
            .map(|(ix, &command_ix)| {
                let command = &self.commands[command_ix];
                let highlight = self.highlights.get(command_ix).cloned().flatten();
                let category = command.category.clone();
                let pinned = pins.is_pinned(command);
                div()
                    .id(("command", ix))
                    .debug_selector(move || format!("command-{ix}"))
                    .flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .group("command")
                    .when_some(category, |item, category| {
                        item.child(
                            div()
                                .text_color(theme.text_muted)
                                .child(format!("{category}:")),
                        )
                    })
                    .child(match highlight {
                        Some(range) => highlighted(&command.name, range, theme).into_any_element(),
                        None => command.name.clone().into_any_element(),
                    })
                    .when_some(bound_key(&command.name, cx), |item, key| {
                        item.child(div().text_color(theme.text_muted).child(key))
                    })
                    .when(!self.is_tab_picker, |item| {
                        item.child(
                            div()
                                .id(("pin", ix))
                                .debug_selector(move || format!("pin-{ix}"))
                                .ml_auto()
                                .group("pin")
                                .text_color(theme.text_muted)
                                // unpinned commands only show their pin while the row is hovered
                                .when(!pinned, |pin| {
                                    pin.invisible().group_hover("command", |pin| pin.visible())
                                })
                                .relative()
                                .child(
                                    div()
                                        .when(pinned, |icon| {
                                            icon.group_hover("pin", |icon| icon.invisible())
                                        })
                                        .child(PIN_ICON),
                                )
                                .child(
                                    // covers the pin, shown while hovered to hint the click unpins
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .invisible()
                                        .when(pinned, |icon| {
                                            icon.group_hover("pin", |icon| icon.visible())
                                        })
                                        .child(UNPIN_ICON),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_pin(ix, cx);
                                })),
                        )
                    })
                    .when(ix == self.selected, |item| {
                        item.bg(theme.tab_active_background)
                    })
                    .hover(|item| item.bg(theme.tab_active_background))
                    .on_click(cx.listener(move |this, _, _, cx| this.confirm(ix, cx)))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let children = if items.is_empty() {
            vec![
                div()
                    .px_3()
                    .py_2()
                    .text_color(theme.text_muted)
                    .child(if self.finder {
                        "nothing found"
                    } else {
                        "no matching commands"
                    })
                    .into_any_element(),
            ]
        } else {
            items
        };
        panel(theme)
            .relative()
            .id("command-palette")
            .debug_selector(|| "command-palette".into())
            .key_context("CommandPalette")
            .on_key_down(cx.listener(Self::key_down))
            // keeps clicks from reaching the terminal below
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().flex_1().min_w_0().child(self.input.clone())),
            )
            .child(
                // the row of commands is its own scroll container, so the items are its
                // direct children and `scroll_to_item` can bring the selected one into view
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .max_h(rems(LIST_MAX_HEIGHT))
                    .child(
                        div()
                            .id("command-list")
                            .debug_selector(|| "command-list".into())
                            .flex_1()
                            .min_h_0()
                            .p_1()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .children(children),
                    )
                    .child(self.render_scrollbar(theme)),
            )
    }
}

/// `text` with the chars in `range` on the search match color
fn highlighted(text: &str, range: Range<usize>, theme: &Theme) -> impl IntoElement {
    let part = |from: usize, to: usize| -> String {
        text.chars()
            .skip(from)
            .take(to.saturating_sub(from))
            .collect()
    };
    let length = text.chars().count();
    div()
        .flex()
        .child(part(0, range.start))
        .child(
            div()
                .bg(theme.search_match_active)
                .text_color(theme.terminal_foreground)
                .child(part(range.start, range.end)),
        )
        .child(part(range.end, length))
}

/// value typed for one input of a command
pub struct InputFilled(pub String);

/// asks for one value of a command, a text field with the input's hint
pub struct InputPrompt {
    /// input name, shown above the field
    name: String,
    hint: String,
    input: Entity<TextInput>,
}

impl EventEmitter<DismissEvent> for InputPrompt {}
impl EventEmitter<InputFilled> for InputPrompt {}

impl InputPrompt {
    /// prompt for the input `name`, showing `hint` while the field is empty
    pub fn new(name: &str, hint: &str, cx: &mut Context<Self>) -> Self {
        Self {
            name: name.to_string(),
            hint: hint.to_string(),
            input: cx.new(|cx| TextInput::new(hint, cx)),
        }
    }

    /// start with `text` in the field, like a value being edited
    pub fn set_text(&self, text: String, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_content(text, cx));
    }

    /// enter fills the input, escape cancels the whole command
    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => cx.emit(InputFilled(self.input.read(cx).content().to_string())),
            "escape" => cx.emit(DismissEvent),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Focusable for InputPrompt {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for InputPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        panel(theme)
            .id("command-input")
            .debug_selector(|| "command-input".into())
            .key_context("CommandInput")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .p_4()
            .gap_2()
            .child(div().text_color(theme.text_muted).child(self.name.clone()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(theme.border)
                    .child(div().flex_1().min_w_0().child(self.input.clone())),
            )
            .child(div().text_color(theme.text_muted).child(format!(
                "enter runs the command, escape cancels. {}",
                self.hint
            )))
    }
}

/// page with the version and links, closed by any key or a click outside
pub struct About {
    focus_handle: FocusHandle,
}

impl EventEmitter<DismissEvent> for About {}

impl About {
    /// about page ready to be focused
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Focusable for About {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for About {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let line = |label: &'static str, value: &'static str| {
            div()
                .flex()
                .gap_2()
                .child(div().w(rems(6.)).text_color(theme.text_muted).child(label))
                .child(value)
        };
        panel(theme)
            .id("about")
            .debug_selector(|| "about".into())
            .track_focus(&self.focus_handle)
            .key_context("About")
            .on_key_down(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
                cx.emit(DismissEvent);
            }))
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .p_4()
            .gap_1()
            .child(div().text_size(rems(1.5)).child("kuterm"))
            .child(
                div()
                    .pb_2()
                    .text_color(theme.text_muted)
                    .child(env!("CARGO_PKG_DESCRIPTION")),
            )
            .child(line("version", env!("CARGO_PKG_VERSION")))
            .child(line("source", env!("CARGO_PKG_REPOSITORY")))
            .child(line("license", env!("CARGO_PKG_LICENSE")))
            .child(
                div()
                    .pt_2()
                    .text_color(theme.text_muted)
                    .child("press any key to close"),
            )
    }
}

/// emitted when closing the tab or quitting is confirmed
pub struct CloseConfirmed;

/// what the close dialog asks about
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CloseTarget {
    /// these tabs, closed together, like the tabs on one side of the active one
    Tabs(Vec<EntityId>),
    /// the whole window, quitting the app
    Window,
}

impl CloseTarget {
    /// true when closing the tab with this view is part of what is asked
    pub fn contains(&self, id: EntityId) -> bool {
        match self {
            Self::Tabs(tabs) => tabs.contains(&id),
            Self::Window => false,
        }
    }
}

/// asks before closing a tab or quitting while programs still run
pub struct ConfirmClose {
    focus_handle: FocusHandle,
    message: String,
    pub target: CloseTarget,
    /// true while "close" is the button enter presses, false for "cancel"
    close_selected: bool,
}

impl EventEmitter<DismissEvent> for ConfirmClose {}
impl EventEmitter<CloseConfirmed> for ConfirmClose {}

impl ConfirmClose {
    /// dialog showing `message` about `target`, ready to be focused
    pub fn new(message: String, target: CloseTarget, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            message,
            target,
            close_selected: true,
        }
    }

    /// shown question, exposed for tests
    #[cfg(test)]
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match dialog_answer(event, &mut self.close_selected) {
            Some(true) => cx.emit(CloseConfirmed),
            Some(false) => cx.emit(DismissEvent),
            None => {}
        }
        cx.notify();
        // other keys must not reach the terminal behind the dialog
        cx.stop_propagation();
    }
}

/// answer a key gives to a yes or no dialog, true for yes. arrows move `yes_selected`, the
/// button enter presses
fn dialog_answer(event: &KeyDownEvent, yes_selected: &mut bool) -> Option<bool> {
    let modifiers = &event.keystroke.modifiers;
    // answers need a bare key, so a shell shortcut like ctrl+y typed by habit answers nothing.
    // shift is still allowed on the letters
    let plain = !modifiers.modified();
    let letter = !(modifiers.control || modifiers.alt || modifiers.platform || modifiers.function);
    match event.keystroke.key.as_str() {
        "enter" if plain => Some(*yes_selected),
        "y" if letter => Some(true),
        "n" if letter => Some(false),
        "escape" if plain => Some(false),
        // "cancel" sits left of the yes button
        "left" | "up" => {
            *yes_selected = false;
            None
        }
        "right" | "down" => {
            *yes_selected = true;
            None
        }
        _ => None,
    }
}

impl Focusable for ConfirmClose {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConfirmClose {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        // the selected button is outlined in the text color, it is the one enter presses
        let button = |id: &'static str, label: &'static str, selected: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px_3()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(if selected { theme.text } else { theme.border })
                .child(label)
        };
        panel(theme)
            .id("confirm-close")
            .debug_selector(|| "confirm-close".into())
            .track_focus(&self.focus_handle)
            .key_context("ConfirmClose")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .p_4()
            .gap_2()
            .child(self.message.clone())
            .child(
                div()
                    .text_color(theme.text_muted)
                    .child(match &self.target {
                        CloseTarget::Tabs(_) => "close anyway?",
                        CloseTarget::Window => "quit anyway?",
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("confirm-close-cancel", "cancel", !self.close_selected)
                            .when(!self.close_selected, |button| {
                                button.bg(theme.tab_active_background)
                            })
                            .hover(|button| button.bg(theme.tab_active_background))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    )
                    .child(
                        button(
                            "confirm-close-ok",
                            match &self.target {
                                CloseTarget::Tabs(_) => "close",
                                CloseTarget::Window => "quit",
                            },
                            self.close_selected,
                        )
                        .bg(theme.danger_button)
                        .text_color(theme.danger_button_text)
                        .hover(|button| button.opacity(0.85))
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(CloseConfirmed))),
                    ),
            )
    }
}

// lines and chars per line the paste dialog shows of the text
const PASTE_PREVIEW_LINES: usize = 6;
const PASTE_PREVIEW_CHARS: usize = 80;

/// emitted when pasting is confirmed
pub struct PasteConfirmed;

/// asks before pasting text that would run right away or has control characters
pub struct ConfirmPaste {
    focus_handle: FocusHandle,
    /// first lines of the text, control characters made visible, hidden and lookalike
    /// characters as marked pieces
    preview: Vec<Vec<PreviewPiece>>,
    /// the text has hidden or lookalike characters
    flagged: bool,
    /// lines not in the preview
    more_lines: usize,
    /// true while "paste" is the button enter presses, false for "cancel"
    paste_selected: bool,
}

impl EventEmitter<DismissEvent> for ConfirmPaste {}
impl EventEmitter<PasteConfirmed> for ConfirmPaste {}

impl ConfirmPaste {
    /// dialog previewing `text`, ready to be focused
    pub fn new(text: &str, cx: &mut Context<Self>) -> Self {
        let (preview, more_lines) = paste_preview(text);
        Self {
            focus_handle: cx.focus_handle(),
            flagged: preview.iter().flatten().any(|piece| piece.marked),
            preview,
            more_lines,
            // enter right after the paste key must not paste by habit
            paste_selected: false,
        }
    }

    /// shown lines, exposed for tests
    #[cfg(test)]
    pub(crate) fn preview(&self) -> Vec<String> {
        self.preview.iter().map(|line| preview_text(line)).collect()
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match dialog_answer(event, &mut self.paste_selected) {
            Some(true) => cx.emit(PasteConfirmed),
            Some(false) => cx.emit(DismissEvent),
            None => {}
        }
        cx.notify();
        cx.stop_propagation();
    }
}

/// part of a previewed line, marked ones are hidden or lookalike characters
#[derive(Clone, Debug, PartialEq)]
struct PreviewPiece {
    text: String,
    marked: bool,
}

#[cfg(test)]
fn preview_text(line: &[PreviewPiece]) -> String {
    line.iter().map(|piece| piece.text.as_str()).collect()
}

/// first lines of a paste with control characters in caret notation, like ^[ for escape,
/// hidden and lookalike characters as marked <U+200B>, and how many lines are left out
fn paste_preview(text: &str) -> (Vec<Vec<PreviewPiece>>, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let preview = lines
        .iter()
        .take(PASTE_PREVIEW_LINES)
        .map(|line| {
            let flagged = paste_check::flagged(line);
            let mut pieces: Vec<PreviewPiece> = Vec::new();
            let mut length = 0;
            for (at, c) in line.char_indices() {
                let marked = flagged.contains(&at);
                let shown = match c {
                    c if marked => format!("<U+{:04X}>", c as u32),
                    '\t' => "    ".into(),
                    c if (c as u32) < 0x20 => format!("^{}", char::from(c as u8 + b'@')),
                    '\x7f' => "^?".into(),
                    c if c.is_control() => format!("<{:02x}>", c as u32),
                    c => c.to_string(),
                };
                let count = shown.chars().count();
                if length + count > PASTE_PREVIEW_CHARS - 1 {
                    pieces.push(PreviewPiece {
                        text: "…".into(),
                        marked: false,
                    });
                    break;
                }
                length += count;
                match pieces.last_mut() {
                    Some(last) if !marked && !last.marked => last.text.push_str(&shown),
                    _ => pieces.push(PreviewPiece {
                        text: shown,
                        marked,
                    }),
                }
            }
            pieces
        })
        .collect();
    (preview, lines.len().saturating_sub(PASTE_PREVIEW_LINES))
}

impl Focusable for ConfirmPaste {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConfirmPaste {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let button = |id: &'static str, label: &'static str, selected: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px_3()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(if selected { theme.text } else { theme.border })
                .child(label)
        };
        panel(theme)
            .id("confirm-paste")
            .debug_selector(|| "confirm-paste".into())
            .track_focus(&self.focus_handle)
            .key_context("ConfirmPaste")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .p_4()
            .gap_2()
            .child("paste this text?")
            .child(div().text_color(theme.text_muted).child(if self.flagged {
                "it has characters that don't show or pass for others, marked below. the text \
                 may run something else than it seems to"
            } else {
                "it has new lines or control characters, the program gets them as typed keys"
            }))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(theme.terminal_background)
                    .text_color(theme.terminal_foreground)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .children(self.preview.iter().map(|line| {
                        div().flex().children(line.iter().map(|piece| {
                            div()
                                .when(piece.marked, |badge| {
                                    badge
                                        .debug_selector(|| "paste-badge".into())
                                        .px_0p5()
                                        .rounded_sm()
                                        .bg(theme.danger_button)
                                        .text_color(theme.danger_button_text)
                                })
                                .child(piece.text.clone())
                        }))
                    }))
                    .when(self.more_lines > 0, |preview| {
                        preview.child(
                            div()
                                .text_color(theme.text_muted)
                                .child(format!("… {} more lines", self.more_lines)),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("confirm-paste-cancel", "cancel", !self.paste_selected)
                            .when(!self.paste_selected, |button| {
                                button.bg(theme.tab_active_background)
                            })
                            .hover(|button| button.bg(theme.tab_active_background))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    )
                    .child(
                        button("confirm-paste-ok", "paste", self.paste_selected)
                            .bg(theme.danger_button)
                            .text_color(theme.danger_button_text)
                            .hover(|button| button.opacity(0.85))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PasteConfirmed))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_preview_marks_hidden_characters() {
        let (preview, _) = paste_preview("ls\u{202e}txt.exe");
        let marked: Vec<&str> = preview[0]
            .iter()
            .filter(|piece| piece.marked)
            .map(|piece| piece.text.as_str())
            .collect();
        assert_eq!(marked, ["<U+202E>"]);
        assert_eq!(preview_text(&preview[0]), "ls<U+202E>txt.exe");
    }

    #[test]
    fn fuzzy_match_skips_letters_case_and_spaces() {
        assert!(fuzzy_match("reload settings", ""));
        assert!(fuzzy_match("reload settings", "rel set"));
        assert!(fuzzy_match("reload settings", "RLDSTG"));
        assert!(!fuzzy_match("reload settings", "settings reload"));
        assert!(!fuzzy_match("about", "abx"));
    }

    #[test]
    fn scrollbar_thumb_follows_offset() {
        let track = Bounds::new(point(px(10.), px(20.)), size(px(6.), px(100.)));
        // nothing to scroll
        assert_eq!(scrollbar_thumb(track, 0., 0.), None);
        // 100px viewport over 100px of hidden content, thumb is half the track at the top
        let top = scrollbar_thumb(track, 0., 100.).unwrap();
        assert_eq!(top.origin, point(px(10.), px(20.)));
        assert_eq!(top.size, size(px(6.), px(50.)));
        // scrolled to the end the thumb sits at the bottom
        let bottom = scrollbar_thumb(track, -100., 100.).unwrap();
        assert_eq!(bottom.origin.y, px(70.));
        // a very long list keeps the minimum thumb height
        let tall = scrollbar_thumb(track, 0., 100_000.).unwrap();
        assert_eq!(tall.size.height, px(SCROLLBAR_MIN_THUMB));
    }

    #[test]
    fn paste_preview_shows_control_characters_and_cuts_long_text() {
        let text = "echo \x1b[31mred\t!\n".to_string() + &"x".repeat(200) + "\n3\n4\n5\n6\n7\n8";
        let (preview, more) = super::paste_preview(&text);
        let preview: Vec<String> = preview.iter().map(|line| preview_text(line)).collect();
        assert_eq!(preview[0], "echo ^[[31mred    !");
        assert_eq!(preview[1].chars().count(), super::PASTE_PREVIEW_CHARS);
        assert!(preview[1].ends_with('…'));
        assert_eq!(preview.len(), super::PASTE_PREVIEW_LINES);
        assert_eq!(more, 2);
    }
}
