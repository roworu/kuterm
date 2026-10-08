//! custom gpui element that paints terminal grid

mod cursor;
mod grid;
mod input_handler;
mod scrollbar;

use alacritty_terminal::{term::TermMode, vte::ansi::CursorShape};
use gpui::{
    App, Bounds, ContentMask, CursorStyle, DispatchPhase, Element, ElementId, Entity, FocusHandle,
    Font, FontFeatures, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Style, TextRun,
    Window, fill, point, px, relative, size,
};

use crate::{
    settings::{ScrollbarEnable, ScrollbarPlacement, Settings},
    terminal::{Terminal, TerminalBounds},
    ui::terminal_view::TerminalView,
};
use cursor::CursorLayout;
use grid::{BatchedTextRun, LayoutRect, layout_grid};
use input_handler::TerminalInputHandler;
use scrollbar::ScrollbarLayout;

/// everything computed in prepaint that paint needs
pub struct LayoutState {
    hitbox: Hitbox,
    rects: Vec<LayoutRect>,
    batched_text_runs: Vec<BatchedTextRun>,
    cursor: Option<CursorLayout>,
    scrollbar: Option<(ScrollbarLayout, Hitbox)>,
    dimensions: TerminalBounds,
    font_size: Pixels,
}

pub struct TerminalElement {
    terminal: Entity<Terminal>,
    terminal_view: Entity<TerminalView>,
    focus: FocusHandle,
    focused: bool,
    /// false while a blinking cursor is in its hidden phase
    cursor_on: bool,
    /// false once auto hide kicked in
    scrollbar_visible: bool,
}

impl TerminalElement {
    /// create element that paints given terminal
    pub fn new(
        terminal: Entity<Terminal>,
        terminal_view: Entity<TerminalView>,
        focus: FocusHandle,
        focused: bool,
        cursor_on: bool,
        scrollbar_visible: bool,
    ) -> Self {
        Self {
            terminal,
            terminal_view,
            focus,
            focused,
            cursor_on,
            scrollbar_visible,
        }
    }

    // window level too, so dragging the thumb keeps scrolling after leaving the track
    fn register_scrollbar_listeners(
        &self,
        scrollbar: ScrollbarLayout,
        hitbox: Hitbox,
        window: &mut Window,
    ) {
        let view = self.terminal_view.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble
                && !cx.has_active_drag()
                && event.button == MouseButton::Left
                && hitbox.is_hovered(window)
            {
                let offset = scrollbar.offset_at(event.position.y);
                view.update(cx, |view, cx| view.scrollbar_down(offset, cx));
            }
        });
        let view = self.terminal_view.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble
                && !cx.has_active_drag()
                && event.pressed_button == Some(MouseButton::Left)
            {
                let offset = scrollbar.offset_at(event.position.y);
                view.update(cx, |view, cx| view.scrollbar_drag(offset, cx));
            }
        });
    }

    // window level listeners, so a drag keeps selecting after leaving the terminal area
    fn register_mouse_listeners(&self, hitbox: Hitbox, report_motion: bool, window: &mut Window) {
        let view = self.terminal_view.clone();
        let down_hitbox = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble
                && !cx.has_active_drag()
                && down_hitbox.is_hovered(window)
            {
                view.update(cx, |view, cx| view.mouse_down(event, cx));
            }
        });
        let view = self.terminal_view.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            // moves without a button only matter to programs tracking all motion
            if phase == DispatchPhase::Bubble
                && !cx.has_active_drag()
                && (event.pressed_button.is_some() || (report_motion && hitbox.is_hovered(window)))
            {
                view.update(cx, |view, cx| view.mouse_move(event, cx));
            }
        });
        let view = self.terminal_view.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && !cx.has_active_drag() {
                view.update(cx, |view, cx| view.mouse_up(event, cx));
            }
        });
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = LayoutState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let settings = &Settings::get(cx).terminal;
        let font = Font {
            features: FontFeatures::disable_ligatures(),
            ..gpui::font(settings.font_family.clone())
        };
        let font_size = px(self.terminal_view.read(cx).font_size(cx));
        let line_height = (font_size * settings.line_height.value()).round();
        let text_system = cx.text_system();
        let font_id = text_system.resolve_font(&font);
        let cell_width = text_system.advance(font_id, font_size, 'm').unwrap().width;

        // the bar keeps its space while hidden, so showing it never reflows the grid
        let bar_settings = settings.scrollbar.clone();
        let bar_width = match bar_settings.enable {
            ScrollbarEnable::Off => px(0.),
            _ => px(bar_settings.width),
        };
        let track_x = match bar_settings.placement {
            ScrollbarPlacement::Left => bounds.origin.x,
            ScrollbarPlacement::Right => bounds.origin.x + bounds.size.width - bar_width,
        };
        let track = Bounds::new(
            point(track_x, bounds.origin.y),
            size(bar_width, bounds.size.height),
        );

        let mut origin = bounds.origin;
        origin.x += cell_width;
        if bar_settings.placement == ScrollbarPlacement::Left {
            origin.x += bar_width;
        }
        let mut grid_size = bounds.size;
        grid_size.width = (grid_size.width - cell_width - bar_width).max(cell_width * 2.);
        // alacritty panics on a grid without rows
        grid_size.height = grid_size.height.max(line_height);

        // snap to device pixels so glyphs do not jitter while resizing
        let scale_factor = window.scale_factor();
        let snap = |v: Pixels| px((f32::from(v) * scale_factor).floor() / scale_factor);
        origin = point(snap(origin.x), snap(origin.y));

        let dimensions =
            TerminalBounds::new(line_height, cell_width, Bounds::new(origin, grid_size));

        self.terminal.update(cx, |terminal, _| {
            terminal.set_size(dimensions);
            terminal.sync();
        });

        let terminal = self.terminal.read(cx);
        let theme = terminal.theme(cx);
        let content = &terminal.last_content;
        let (rects, batched_text_runs) = layout_grid(
            &content.cells,
            content.display_offset,
            content.selection,
            &font,
            theme,
        );

        let cursor_line = content.cursor.point.line.0 + content.display_offset as i32;
        let cursor = (content.cursor.shape != CursorShape::Hidden
            && cursor_line >= 0
            && (cursor_line as usize) < dimensions.num_lines())
        .then(|| {
            let text = window.text_system().shape_line(
                content.cursor_char.to_string().into(),
                font_size,
                &[TextRun {
                    len: content.cursor_char.len_utf8(),
                    font: font.clone(),
                    color: theme.terminal_background,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            // wide glyphs like emoji need a wider block
            let width = if content.cursor_char.is_whitespace() {
                cell_width
            } else {
                text.width.max(cell_width)
            };
            CursorLayout {
                bounds: Bounds::new(
                    point(
                        (content.cursor.point.column.0 as f32 * cell_width).floor(),
                        (cursor_line as f32 * line_height).floor(),
                    ),
                    size(width.ceil(), line_height),
                ),
                shape: content.cursor.shape,
                color: theme.cursor,
                focused: self.focused,
                text,
            }
        });

        let show_scrollbar = self.scrollbar_visible
            && match bar_settings.enable {
                ScrollbarEnable::On => true,
                ScrollbarEnable::Off => false,
                ScrollbarEnable::Dynamic => content.history_size > 0,
            };
        let scrollbar = show_scrollbar.then(|| {
            ScrollbarLayout::new(
                track,
                content.history_size,
                dimensions.num_lines(),
                content.display_offset,
                theme.scrollbar,
            )
        });

        // inserted after the terminal hitbox so it sits on top and clicks on it skip selection
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let scrollbar = scrollbar.map(|scrollbar| {
            let hitbox = window.insert_hitbox(track, HitboxBehavior::BlockMouseExceptScroll);
            (scrollbar, hitbox)
        });

        LayoutState {
            hitbox,
            rects,
            batched_text_runs,
            cursor,
            scrollbar,
            dimensions,
            font_size,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.set_cursor_style(CursorStyle::IBeam, &layout.hitbox);
        let report_motion = self
            .terminal
            .read(cx)
            .last_content
            .mode
            .contains(TermMode::MOUSE_MOTION);
        self.register_mouse_listeners(layout.hitbox.clone(), report_motion, window);
        if let Some((scrollbar, hitbox)) = &layout.scrollbar {
            window.set_cursor_style(CursorStyle::Arrow, hitbox);
            self.register_scrollbar_listeners(*scrollbar, hitbox.clone(), window);
        }

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let background = self.terminal.read(cx).theme(cx).terminal_background;
            window.paint_quad(fill(bounds, background));

            let origin = layout.dimensions.bounds.origin;
            window.handle_input(
                &self.focus,
                TerminalInputHandler {
                    terminal_view: self.terminal_view.clone(),
                    cursor_bounds: layout.cursor.as_ref().map(|c| c.bounds + origin),
                },
                cx,
            );

            for rect in &layout.rects {
                rect.paint(origin, &layout.dimensions, window);
            }
            for run in &mut layout.batched_text_runs {
                run.paint(origin, &layout.dimensions, layout.font_size, window, cx);
            }
            // layout is kept while hidden, the input handler still needs its bounds for ime
            if let Some(cursor) = &layout.cursor
                && self.cursor_on
            {
                cursor.paint(origin, window, cx);
            }
            if let Some((scrollbar, _)) = &layout.scrollbar {
                scrollbar.paint(window);
            }
        });
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
