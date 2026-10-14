//! single line text field built on gpui's text input protocol,
//! ported from gpui's own `examples/input.rs` and themed with kuterm colors

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextAlign, TextRun, UTF16Selection, Window, actions, div,
    fill, point, prelude::*, px, relative, size,
};

use crate::theme::Theme;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        WordLeft,
        WordRight,
        DeleteWordBack,
        Home,
        End,
        Paste,
        Cut,
        Copy,
    ]
);

/// key bindings for the field, scoped to the "TextInput" context
pub fn bindings() -> Vec<KeyBinding> {
    let ctx = Some("TextInput");
    vec![
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("ctrl-a", SelectAll, ctx),
        KeyBinding::new("cmd-a", SelectAll, ctx),
        KeyBinding::new("alt-left", WordLeft, ctx),
        KeyBinding::new("ctrl-left", WordLeft, ctx),
        KeyBinding::new("alt-right", WordRight, ctx),
        KeyBinding::new("ctrl-right", WordRight, ctx),
        KeyBinding::new("alt-backspace", DeleteWordBack, ctx),
        KeyBinding::new("ctrl-backspace", DeleteWordBack, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("ctrl-v", Paste, ctx),
        KeyBinding::new("cmd-v", Paste, ctx),
        KeyBinding::new("ctrl-c", Copy, ctx),
        KeyBinding::new("cmd-c", Copy, ctx),
        KeyBinding::new("ctrl-x", Cut, ctx),
        KeyBinding::new("cmd-x", Cut, ctx),
    ]
}

/// text changed, the palette uses it to refilter the command list
#[derive(Clone, Debug, PartialEq)]
pub struct Changed;

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    /// selected range in bytes, empty means just a caret
    selected_range: Range<usize>,
    selection_reversed: bool,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

impl EventEmitter<Changed> for TextInput {}

impl TextInput {
    /// empty field showing `placeholder`
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
        }
    }

    /// current text
    pub fn content(&self) -> &str {
        &self.content
    }

    /// replace the text, selected whole so typing replaces it
    pub fn set_content(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete_word_back(
        &mut self,
        _: &DeleteWordBack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_word_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content[..offset.min(self.content.len())]
            .char_indices()
            .next_back()
            .map_or(0, |(ix, _)| ix)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.content.len());
        self.content[offset..]
            .chars()
            .next()
            .map_or(self.content.len(), |ch| offset + ch.len_utf8())
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        let mut boundary = 0;
        let mut seen_word = false;
        for (ix, ch) in self.content[..offset.min(self.content.len())]
            .char_indices()
            .rev()
        {
            if ch.is_whitespace() {
                if seen_word {
                    break;
                }
            } else {
                seen_word = true;
                boundary = ix;
            }
        }
        boundary
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.content.len());
        let mut seen_word = false;
        for (ix, ch) in self.content[offset..].char_indices() {
            if ch.is_whitespace() {
                if seen_word {
                    return offset + ix;
                }
            } else {
                seen_word = true;
            }
        }
        self.content.len()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // the field is one line, so control characters like enter never land in it
        let new_text: String = new_text.chars().filter(|ch| !ch.is_control()).collect();
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        cx.emit(Changed);
        cx.notify();
    }

    // marked text is not tracked, so ime composition text is inserted like typed text
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(range_utf16, new_text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let theme = Theme::get(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), theme.text_muted)
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &[run], None);

        let cursor_pos = line.x_for_index(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    theme.cursor,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    theme.selection,
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        line.paint(
            bounds.origin,
            window.line_height(),
            TextAlign::Left,
            None,
            window,
            cx,
        )
        .unwrap();

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        div()
            .w_full()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .text_color(theme.text)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::delete_word_back))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Element, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext};

    use super::*;

    struct Host {
        input: Entity<TextInput>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.input.clone())
        }
    }

    fn open(cx: &mut TestAppContext) -> (Entity<TextInput>, &mut VisualTestContext) {
        cx.update(|cx| {
            cx.set_global(Theme::default());
            cx.bind_keys(bindings());
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            let input = cx.new(|cx| TextInput::new("hint", cx));
            Host { input }
        });
        cx.simulate_resize(size(px(400.), px(60.)));
        cx.run_until_parked();
        let input = host.read_with(cx, |host, _| host.input.clone());
        (input, cx)
    }

    fn set(
        input: &Entity<TextInput>,
        cx: &mut VisualTestContext,
        content: &str,
        selection: Range<usize>,
    ) {
        input.update(cx, |input, cx| {
            input.content = content.into();
            input.selected_range = selection;
            input.selection_reversed = false;
            cx.notify();
        });
        cx.run_until_parked();
    }

    fn text(input: &Entity<TextInput>, cx: &VisualTestContext) -> String {
        input.read_with(cx, |input, _| input.content.to_string())
    }

    fn cursor(input: &Entity<TextInput>, cx: &VisualTestContext) -> usize {
        input.read_with(cx, |input, _| input.cursor_offset())
    }

    fn selection(input: &Entity<TextInput>, cx: &VisualTestContext) -> (Range<usize>, bool) {
        input.read_with(cx, |input, _| {
            (input.selected_range.clone(), input.selection_reversed)
        })
    }

    fn act<R>(
        input: &Entity<TextInput>,
        cx: &mut VisualTestContext,
        f: impl FnOnce(&mut TextInput, &mut Window, &mut Context<TextInput>) -> R,
    ) -> R {
        input.update_in(cx, f)
    }

    fn down_event(position: Point<Pixels>, shift: bool) -> MouseDownEvent {
        MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers {
                shift,
                ..Modifiers::default()
            },
            click_count: 1,
            first_mouse: false,
        }
    }

    #[test]
    fn bindings_cover_the_editing_keys() {
        assert_eq!(bindings().len(), 22);
    }

    #[gpui::test]
    fn utf16_offsets_round_trip(cx: &mut TestAppContext) {
        let (input, cx) = open(cx);
        set(&input, cx, "a\u{e9}\u{65e5}\u{1f680}", 0..0);

        let (f0, f1, f2, f3, f4, f5) = input.read_with(cx, |input, _| {
            (
                input.offset_from_utf16(0),
                input.offset_from_utf16(1),
                input.offset_from_utf16(2),
                input.offset_from_utf16(3),
                input.offset_from_utf16(4),
                input.offset_from_utf16(5),
            )
        });
        assert_eq!((f0, f1, f2, f3, f4, f5), (0, 1, 3, 6, 10, 10));

        let (t0, t1, t3, t6, t9, t10) = input.read_with(cx, |input, _| {
            (
                input.offset_to_utf16(0),
                input.offset_to_utf16(1),
                input.offset_to_utf16(3),
                input.offset_to_utf16(6),
                input.offset_to_utf16(9),
                input.offset_to_utf16(10),
            )
        });
        assert_eq!((t0, t1, t3, t6, t9, t10), (0, 1, 2, 3, 5, 5));

        let (range_to, range_from) = input.read_with(cx, |input, _| {
            (
                input.range_to_utf16(&(1..6)),
                input.range_from_utf16(&(1..3)),
            )
        });
        assert_eq!(range_to, 1..3);
        assert_eq!(range_from, 1..6);
    }

    #[gpui::test]
    fn boundaries_stop_at_char_edges(cx: &mut TestAppContext) {
        let (input, cx) = open(cx);
        set(&input, cx, "a\u{e9}\u{65e5}\u{1f680}", 0..0);

        let values = input.read_with(cx, |input, _| {
            (
                input.previous_boundary(0),
                input.previous_boundary(1),
                input.previous_boundary(3),
                input.previous_boundary(10),
                input.next_boundary(0),
                input.next_boundary(1),
                input.next_boundary(3),
                input.next_boundary(6),
                input.next_boundary(10),
                input.next_boundary(99),
            )
        });
        assert_eq!(values, (0, 0, 1, 6, 1, 3, 6, 10, 10, 10));
    }

    #[gpui::test]
    fn word_boundaries_skip_whitespace(cx: &mut TestAppContext) {
        let (input, cx) = open(cx);
        set(&input, cx, "foo bar  baz", 0..0);

        let values = input.read_with(cx, |input, _| {
            (
                input.previous_word_boundary(12),
                input.previous_word_boundary(9),
                input.previous_word_boundary(4),
                input.previous_word_boundary(0),
                input.next_word_boundary(0),
                input.next_word_boundary(4),
                input.next_word_boundary(9),
            )
        });
        assert_eq!(values, (9, 4, 0, 0, 3, 7, 12));

        set(&input, cx, "  x", 0..0);
        let (previous, next) = input.read_with(cx, |input, _| {
            (input.previous_word_boundary(3), input.next_word_boundary(0))
        });
        assert_eq!((previous, next), (2, 3));
    }

    #[gpui::test]
    fn left_and_right_collapse_selection(cx: &mut TestAppContext) {
        let (input, cx) = open(cx);
        set(&input, cx, "abc", 2..2);
        act(&input, cx, |input, window, cx| {
            input.left(&Left, window, cx)
        });
        assert_eq!(cursor(&input, cx), 1);
        act(&input, cx, |input, window, cx| {
            input.right(&Right, window, cx)
        });
        assert_eq!(cursor(&input, cx), 2);

        set(&input, cx, "abc", 0..3);
        act(&input, cx, |input, window, cx| {
            input.left(&Left, window, cx)
        });
        assert_eq!(cursor(&input, cx), 0);
        set(&input, cx, "abc", 0..3);
        act(&input, cx, |input, window, cx| {
            input.right(&Right, window, cx)
        });
        assert_eq!(cursor(&input, cx), 3);
    }

    #[gpui::test]
    fn select_moves_the_selection(cx: &mut TestAppContext) {
        let (input, cx) = open(cx);
        set(&input, cx, "abc", 2..2);
        act(&input, cx, |input, window, cx| {
            input.select_left(&SelectLeft, window, cx)
        });
        assert_eq!(selection(&input, cx), (1..2, true));
        act(&input, cx, |input, window, cx| {
            input.select_left(&SelectLeft, window, cx)
        });
        assert_eq!(selection(&input, cx), (0..2, true));
        act(&input, cx, |input, window, cx| {
            input.select_right(&SelectRight, window, cx)
        });
        assert_eq!(selection(&input, cx), (1..2, true));

        act(&input, cx, |input, window, cx| {
            input.select_all(&SelectAll, window, cx)
        });
        assert_eq!(selection(&input, cx), (0..3, false));

        act(&input, cx, |input, window, cx| {
            input.home(&Home, window, cx)
        });
        assert_eq!(cursor(&input, cx), 0);
        act(&input, cx, |input, window, cx| input.end(&End, window, cx));
        assert_eq!(cursor(&input, cx), 3);
    }

    #[gpui::test]
    fn word_actions_move_by_words(cx: &mut TestAppContext) {
        let (input, cx) = open(cx);
        set(&input, cx, "foo bar", 7..7);
        act(&input, cx, |input, window, cx| {
            input.word_left(&WordLeft, window, cx)
        });
        assert_eq!(cursor(&input, cx), 4);
        set(&input, cx, "foo bar", 0..0);
        act(&input, cx, |input, window, cx| {
            input.word_right(&WordRight, window, cx)
        });
        assert_eq!(cursor(&input, cx), 3);
    }

    #[gpui::test]
    fn backspace_and_delete_remove_chars(cx: &mut TestAppContext) {
        let (input, cx) = open(cx);
        set(&input, cx, "abc", 3..3);
        act(&input, cx, |input, window, cx| {
            input.backspace(&Backspace, window, cx)
        });
        assert_eq!(text(&input, cx), "ab");
        assert_eq!(cursor(&input, cx), 2);

        // the selection is removed whole
        set(&input, cx, "abc", 0..2);
        act(&input, cx, |input, window, cx| {
            input.backspace(&Backspace, window, cx)
        });
        assert_eq!(text(&input, cx), "c");

        set(&input, cx, "abc", 0..0);
        act(&input, cx, |input, window, cx| {
            input.backspace(&Backspace, window, cx)
        });
        assert_eq!(text(&input, cx), "abc");

        set(&input, cx, "abc", 1..1);
        act(&input, cx, |input, window, cx| {
            input.delete(&Delete, window, cx)
        });
        assert_eq!(text(&input, cx), "ac");
        set(&input, cx, "abc", 0..2);
        act(&input, cx, |input, window, cx| {
            input.delete(&Delete, window, cx)
        });
        assert_eq!(text(&input, cx), "c");
        set(&input, cx, "abc", 3..3);
        act(&input, cx, |input, window, cx| {
            input.delete(&Delete, window, cx)
        });
        assert_eq!(text(&input, cx), "abc");
    }

    #[gpui::test]
    fn delete_word_back_removes_the_last_word(cx: &mut TestAppContext) {
        let (input, cx) = open(cx);
        set(&input, cx, "foo bar", 7..7);
        act(&input, cx, |input, window, cx| {
            input.delete_word_back(&DeleteWordBack, window, cx)
        });
        assert_eq!(text(&input, cx), "foo ");

        set(&input, cx, "foo bar", 0..4);
        act(&input, cx, |input, window, cx| {
            input.delete_word_back(&DeleteWordBack, window, cx)
        });
        assert_eq!(text(&input, cx), "bar");
    }

    #[gpui::test]
    fn replace_text_in_range_uses_utf16_and_selection(cx: &mut TestAppContext) {
        let (input, cx) = open(cx);

        set(&input, cx, "abc", 1..2);
        act(&input, cx, |input, window, cx| {
            input.replace_text_in_range(None, "X", window, cx)
        });
        assert_eq!(text(&input, cx), "aXc");
        assert_eq!(selection(&input, cx), (2..2, false));

        set(&input, cx, "abcdef", 0..0);
        act(&input, cx, |input, window, cx| {
            input.replace_text_in_range(Some(1..3), "Y", window, cx)
        });
        assert_eq!(text(&input, cx), "aYdef");

        // control characters never enter the single line field
        set(&input, cx, "", 0..0);
        act(&input, cx, |input, window, cx| {
            input.replace_text_in_range(None, "a\tb\nc", window, cx)
        });
        assert_eq!(text(&input, cx), "abc");
    }

    #[gpui::test]
    fn paste_copy_and_cut_use_the_clipboard(cx: &mut TestAppContext) {
        let (input, cx) = open(cx);

        cx.write_to_clipboard(ClipboardItem::new_string("HELLO".into()));
        set(&input, cx, "abc", 3..3);
        act(&input, cx, |input, window, cx| {
            input.paste(&Paste, window, cx)
        });
        assert_eq!(text(&input, cx), "abcHELLO");

        cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
        set(&input, cx, "abc", 0..0);
        act(&input, cx, |input, window, cx| {
            input.paste(&Paste, window, cx)
        });
        assert_eq!(text(&input, cx), "abc");

        set(&input, cx, "abc", 1..3);
        act(&input, cx, |input, window, cx| {
            input.copy(&Copy, window, cx)
        });
        assert_eq!(text(&input, cx), "abc");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("bc".into())
        );

        cx.write_to_clipboard(ClipboardItem::new_string("keep".into()));
        set(&input, cx, "abc", 2..2);
        act(&input, cx, |input, window, cx| {
            input.copy(&Copy, window, cx)
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("keep".into())
        );

        set(&input, cx, "abc", 1..3);
        act(&input, cx, |input, window, cx| input.cut(&Cut, window, cx));
        assert_eq!(text(&input, cx), "a");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("bc".into())
        );

        cx.write_to_clipboard(ClipboardItem::new_string("keep".into()));
        set(&input, cx, "abc", 1..1);
        act(&input, cx, |input, window, cx| input.cut(&Cut, window, cx));
        assert_eq!(text(&input, cx), "abc");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("keep".into())
        );
    }

    #[gpui::test]
    fn input_handler_reports_text_bounds_and_clears_marking(cx: &mut TestAppContext) {
        let (input, cx) = open(cx);
        set(&input, cx, "abc", 1..2);

        let mut actual = None;
        let first_two = act(&input, cx, |input, window, cx| {
            input.text_for_range(0..2, &mut actual, window, cx)
        });
        assert_eq!(first_two, Some("ab".into()));
        assert_eq!(actual, Some(0..2));

        let selected = act(&input, cx, |input, window, cx| {
            input.selected_text_range(false, window, cx)
        })
        .unwrap();
        assert_eq!(selected.range, 1..2);
        assert!(!selected.reversed);

        assert!(
            act(&input, cx, |input, window, cx| input
                .marked_text_range(window, cx))
            .is_none()
        );
        act(&input, cx, |input, window, cx| {
            input.unmark_text(window, cx)
        });

        act(&input, cx, |input, window, cx| {
            input.replace_and_mark_text_in_range(None, "xy", None, window, cx)
        });
        assert_eq!(text(&input, cx), "axyc");

        let (range_bounds, index) = input.update_in(cx, |input, window, cx| {
            let bounds = *input.last_bounds.as_ref().unwrap();
            let range_bounds = input.bounds_for_range(0..2, bounds, window, cx);
            let index = input.character_index_for_point(
                point(bounds.left() + px(1.), bounds.center().y),
                window,
                cx,
            );
            (range_bounds, index)
        });
        assert!(range_bounds.is_some());
        assert_eq!(index, Some(0));
    }

    #[gpui::test]
    fn mouse_click_moves_caret_and_drags_selection(cx: &mut TestAppContext) {
        let (input, cx) = open(cx);
        set(&input, cx, "hello world", 0..0);
        let bounds = input.read_with(cx, |input, _| *input.last_bounds.as_ref().unwrap());

        let click = down_event(point(bounds.left() + px(0.5), bounds.center().y), false);
        act(&input, cx, |input, window, cx| {
            input.on_mouse_down(&click, window, cx)
        });
        assert_eq!(cursor(&input, cx), 0);

        let drag = MouseMoveEvent {
            position: point(bounds.right() - px(1.), bounds.center().y),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        };
        act(&input, cx, |input, window, cx| {
            input.on_mouse_move(&drag, window, cx)
        });
        assert_eq!(selection(&input, cx), (0..11, false));
        assert_eq!(cursor(&input, cx), 11);

        let up = MouseUpEvent {
            button: MouseButton::Left,
            position: drag.position,
            modifiers: Modifiers::default(),
            click_count: 1,
        };
        act(&input, cx, |input, window, cx| {
            input.on_mouse_up(&up, window, cx)
        });

        // after the button is released a hover must not extend the selection
        let hover = MouseMoveEvent {
            position: point(bounds.left() + px(0.5), bounds.center().y),
            pressed_button: None,
            modifiers: Modifiers::default(),
        };
        act(&input, cx, |input, window, cx| {
            input.on_mouse_move(&hover, window, cx)
        });
        assert_eq!(cursor(&input, cx), 11);

        set(&input, cx, "hello world", 3..3);
        let shift_click = down_event(point(bounds.left() + px(0.5), bounds.center().y), true);
        act(&input, cx, |input, window, cx| {
            input.on_mouse_down(&shift_click, window, cx)
        });
        assert_eq!(selection(&input, cx), (0..3, true));

        let above = down_event(point(bounds.left() + px(5.), bounds.top() - px(10.)), false);
        act(&input, cx, |input, window, cx| {
            input.on_mouse_down(&above, window, cx)
        });
        assert_eq!(cursor(&input, cx), 0);
        let below = down_event(
            point(bounds.left() + px(5.), bounds.bottom() + px(10.)),
            false,
        );
        act(&input, cx, |input, window, cx| {
            input.on_mouse_down(&below, window, cx)
        });
        assert_eq!(cursor(&input, cx), 11);
    }

    #[gpui::test]
    fn mouse_position_defaults_to_start_without_layout(cx: &mut TestAppContext) {
        let input = cx.new(|cx| TextInput::new("hint", cx));
        let index = input.update(cx, |input, _| {
            input.index_for_mouse_position(point(px(5.), px(5.)))
        });
        assert_eq!(index, 0);
    }

    #[gpui::test]
    fn text_element_has_no_source_location(cx: &mut TestAppContext) {
        let (input, _cx) = open(cx);
        assert!(TextElement { input }.source_location().is_none());
    }
}
