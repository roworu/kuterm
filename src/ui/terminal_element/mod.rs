//! custom gpui element that paints terminal grid

mod box_drawing;
mod cursor;
mod decorations;
mod grid;
mod input_handler;
mod scrollbar;

use alacritty_terminal::{
    index::Point as AlacPoint, term::TermMode, term::search::Match, vte::ansi::CursorShape,
};

use gpui::{
    App, Bounds, ContentMask, CursorStyle, DispatchPhase, Element, ElementId, Entity, FocusHandle,
    Font, FontFallbacks, FontFeatures, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, ShapedLine, Size, Style, TextRun, Window, fill, point, px,
    relative, size,
};

use crate::{
    settings::{
        ScrollbarEnable, ScrollbarPlacement, Settings, TerminalSettings, parse_font_feature,
    },
    terminal::{Terminal, TerminalBounds},
    ui::terminal_view::TerminalView,
};
use cursor::CursorLayout;
use grid::{
    BatchedTextRun, BoxGlyph, GridStyle, LayoutRect, SearchHighlights, TerminalFonts, layout_grid,
};
use input_handler::TerminalInputHandler;
use scrollbar::ScrollbarLayout;

/// concealed text (sgr 8) is drawn while this is on, so people can see what programs hid
#[derive(Clone, Copy, Default)]
pub struct ShowHidden(pub bool);

impl gpui::Global for ShowHidden {}

/// regular terminal font with the features and fallbacks from settings
fn terminal_font(settings: &TerminalSettings) -> Font {
    let features: Vec<_> = settings
        .font_features
        .iter()
        .filter_map(|feature| parse_font_feature(feature))
        .collect();
    Font {
        features: FontFeatures(features.into()),
        fallbacks: (!settings.font_fallbacks.is_empty())
            .then(|| FontFallbacks::from_fonts(settings.font_fallbacks.clone())),
        ..gpui::font(settings.font.family.clone())
    }
}

/// faces for each style, a style without own family uses the regular one
fn terminal_fonts(settings: &TerminalSettings) -> TerminalFonts {
    let regular = terminal_font(settings);
    let font = &settings.font;
    let face = |family: &Option<String>, weight: FontWeight, style: gpui::FontStyle| Font {
        family: family.clone().map_or(regular.family.clone(), Into::into),
        weight,
        style,
        ..regular.clone()
    };
    let bold = FontWeight(font.bold.weight);
    let italic = font.italic.style.into();
    TerminalFonts {
        bold: face(&font.bold.family, bold, gpui::FontStyle::Normal),
        italic: face(&font.italic.family, FontWeight::NORMAL, italic),
        bold_italic: face(&font.bold_italic.family, bold, italic),
        regular,
    }
}

/// width and height of one grid cell at `font_size`
pub fn cell_size(font_size: Pixels, cx: &App) -> Size<Pixels> {
    let settings = &Settings::get(cx).terminal;
    let text_system = cx.text_system();
    let font_id = text_system.resolve_font(&terminal_font(settings));
    let cell_width = text_system.advance(font_id, font_size, 'm').unwrap().width;
    size(
        (cell_width + px(settings.cell_width)).max(px(1.)),
        (font_size * settings.line_height.value()).round(),
    )
}

/// everything computed in prepaint that paint needs
pub struct LayoutState {
    hitbox: Hitbox,
    rects: Vec<LayoutRect>,
    batched_text_runs: Vec<BatchedTextRun>,
    box_glyphs: Vec<BoxGlyph>,
    /// moves text inside its cells, backgrounds and builtin glyphs stay
    glyph_offset: Point<Pixels>,
    cursor: Option<CursorLayout>,
    scrollbar: Option<(ScrollbarLayout, Hitbox)>,
    dimensions: TerminalBounds,
    font_size: Pixels,
    /// underline of the hovered link, relative to the grid origin
    link_underlines: Vec<Bounds<Pixels>>,
    link_color: Hsla,
    /// hint labels with their boxes, relative to the grid origin
    hint_labels: Vec<(Bounds<Pixels>, ShapedLine)>,
    hint_background: Hsla,
    /// hint line under the prompt, relative to the grid origin, never part of the grid
    hint_line: Option<(Point<Pixels>, ShapedLine)>,
    /// popovers, relative to the window
    decorations: decorations::DecorationsLayout,
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
    /// fading bell flash painted over the terminal
    bell_flash: Option<Hsla>,
    /// cells of every part of the link under the mouse, underlined
    link: Vec<Match>,
    /// hint labels and the cell each one starts at
    hints: Vec<(AlacPoint, String)>,
}

impl TerminalElement {
    /// create element that paints given terminal
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        terminal: Entity<Terminal>,
        terminal_view: Entity<TerminalView>,
        focus: FocusHandle,
        focused: bool,
        cursor_on: bool,
        scrollbar_visible: bool,
        bell_flash: Option<Hsla>,
        link: Vec<Match>,
        hints: Vec<(AlacPoint, String)>,
    ) -> Self {
        Self {
            terminal,
            terminal_view,
            focus,
            focused,
            cursor_on,
            scrollbar_visible,
            bell_flash,
            link,
            hints,
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
    fn register_mouse_listeners(&self, hitbox: Hitbox, window: &mut Window) {
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
            // moves without a button matter to programs tracking all motion and to links
            if phase == DispatchPhase::Bubble
                && !cx.has_active_drag()
                && (event.pressed_button.is_some() || hitbox.is_hovered(window))
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
        let window_settings = Settings::get(cx).window.clone();
        let settings = &Settings::get(cx).terminal;
        let fonts = terminal_fonts(settings);
        let glyph_offset = point(px(settings.glyph_offset.x), px(settings.glyph_offset.y));
        let cursor_settings = settings.cursor;
        let style = GridStyle {
            fonts: &fonts,
            bold_is_bright: settings.bold_is_bright,
            minimum_contrast: settings.minimum_contrast,
            dim_opacity: settings.dim_opacity,
            builtin_box_drawing: settings.builtin_box_drawing,
            show_hidden: cx.try_global::<ShowHidden>().is_some_and(|shown| shown.0),
        };
        let font_size = px(self.terminal_view.read(cx).font_size(cx));
        let cell = cell_size(font_size, cx);
        let (cell_width, line_height) = (cell.width, cell.height);

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

        let padding = window_settings.padding;
        let (pad_x, pad_y) = (px(padding.x), px(padding.y));
        let mut origin = point(bounds.origin.x + pad_x, bounds.origin.y + pad_y);
        if bar_settings.placement == ScrollbarPlacement::Left {
            origin.x += bar_width;
        }
        let mut grid_size = bounds.size;
        grid_size.width = (grid_size.width - pad_x * 2. - bar_width).max(cell_width * 2.);
        // alacritty panics on a grid without rows
        grid_size.height = (grid_size.height - pad_y * 2.).max(line_height);
        if window_settings.padding_balance {
            // only whole cells are drawn, so the rest of the grid size is spare room
            let spare = |length: Pixels, cell: Pixels| px(f32::from(length) % f32::from(cell));
            origin.x += spare(grid_size.width, cell_width) / 2.;
            origin.y += spare(grid_size.height, line_height) / 2.;
        }

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
        let (rects, batched_text_runs, box_glyphs) = layout_grid(
            &content.cells,
            content.display_offset,
            (content.cursor.shape != CursorShape::Hidden).then_some(content.cursor.point),
            content.selection,
            SearchHighlights {
                matches: &content.search_matches,
                active: content.active_match.as_ref(),
            },
            &style,
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
                    font: fonts.regular.clone(),
                    color: cursor_settings
                        .text_color
                        .unwrap_or(theme.terminal_background),
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
                color: cursor_settings.color.unwrap_or(theme.cursor),
                focused: self.focused,
                unfocused: cursor_settings.unfocused,
                thickness: px(cursor_settings.thickness),
                text,
                text_offset: glyph_offset,
            }
        });

        // only while the cursor is on screen, it belongs to the line being typed
        let hint_line = terminal
            .hint_line()
            .filter(|hint| {
                cursor.is_some()
                    && !hint.text.is_empty()
                    && (cursor_line as usize) + 1 < dimensions.num_lines()
            })
            .map(|hint| {
                let text = window.text_system().shape_line(
                    hint.text.clone().into(),
                    font_size,
                    &[TextRun {
                        len: hint.text.len(),
                        font: fonts.italic.clone(),
                        color: hint.color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    Some(cell_width),
                );
                (point(px(0.), line_height * (cursor_line as f32 + 1.)), text)
            });

        let decorations = decorations::layout_decorations(
            terminal,
            &dimensions,
            bounds,
            &fonts.regular,
            font_size,
            theme,
            window,
        );

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

        let link_underlines = self
            .link
            .iter()
            .flat_map(|link| link_underlines(link, content.display_offset, &dimensions))
            .collect();
        let link_color = theme.terminal_foreground;
        let hint_labels = self
            .hints
            .iter()
            .filter_map(|(start, label)| {
                let row = start.line.0 + content.display_offset as i32;
                if row < 0 || row as usize >= dimensions.num_lines() || label.is_empty() {
                    return None;
                }
                let text = window.text_system().shape_line(
                    label.clone().into(),
                    font_size,
                    &[TextRun {
                        len: label.len(),
                        font: fonts.bold.clone(),
                        color: theme.terminal_background,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    Some(cell_width),
                );
                let bounds = Bounds::new(
                    point(cell_width * start.column.0 as f32, line_height * row as f32),
                    size(cell_width * label.chars().count() as f32, line_height),
                );
                Some((bounds, text))
            })
            .collect();
        let hint_background = theme.cursor;

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
            box_glyphs,
            glyph_offset,
            cursor,
            scrollbar,
            dimensions,
            font_size,
            link_underlines,
            link_color,
            hint_labels,
            hint_background,
            hint_line,
            decorations,
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
        let cursor = if layout.link_underlines.is_empty() {
            CursorStyle::IBeam
        } else {
            CursorStyle::PointingHand
        };
        window.set_cursor_style(cursor, &layout.hitbox);
        self.register_mouse_listeners(layout.hitbox.clone(), window);
        if let Some((scrollbar, hitbox)) = &layout.scrollbar {
            window.set_cursor_style(CursorStyle::Arrow, hitbox);
            self.register_scrollbar_listeners(*scrollbar, hitbox.clone(), window);
        }

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // the workspace paints the terminal background, with the window opacity. painting
            // it again here compounds the alpha, so a window at 0.85 turns almost opaque
            let origin = layout.dimensions.bounds.origin;
            window.handle_input(
                &self.focus,
                TerminalInputHandler {
                    terminal_view: self.terminal_view.clone(),
                    cursor_bounds: layout.cursor.as_ref().map(|c| c.bounds + origin),
                },
                cx,
            );

            // only full screen programs own the edges, a shell prompt keeps its padding
            let alt_screen = self
                .terminal
                .read(cx)
                .last_content
                .mode
                .contains(TermMode::ALT_SCREEN);
            let edges = alt_screen.then_some(bounds);
            for rect in &layout.rects {
                rect.paint(origin, &layout.dimensions, edges, window);
            }
            for run in &mut layout.batched_text_runs {
                run.paint(
                    origin + layout.glyph_offset,
                    &layout.dimensions,
                    layout.font_size,
                    window,
                    cx,
                );
            }
            for glyph in &layout.box_glyphs {
                glyph.paint(origin, &layout.dimensions, window);
            }
            for underline in &layout.link_underlines {
                window.paint_quad(fill(*underline + origin, layout.link_color));
            }
            for (bounds, text) in &layout.hint_labels {
                let bounds = *bounds + origin;
                window.paint_quad(fill(bounds, layout.hint_background));
                text.paint(
                    bounds.origin,
                    layout.dimensions.line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            }
            if let Some((at, text)) = &layout.hint_line {
                let at = *at + origin + layout.glyph_offset;
                let background = self.terminal.read(cx).theme(cx).terminal_background;
                // covers what the grid has there, the hint reads as a line of its own
                window.paint_quad(fill(
                    Bounds::new(at, size(text.width, layout.dimensions.line_height)),
                    background,
                ));
                text.paint(
                    at,
                    layout.dimensions.line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            }
            // layout is kept while hidden, the input handler still needs its bounds for ime
            if let Some(cursor) = &layout.cursor
                && self.cursor_on
            {
                cursor.paint(origin, window, cx);
            }
            let line_height = layout.dimensions.line_height;
            if let Some((scrollbar, _)) = &layout.scrollbar {
                scrollbar.paint(window);
            }
            for text_box in &layout.decorations.boxes {
                text_box.paint(line_height, window, cx);
            }
            if let Some(flash) = self.bell_flash {
                window.paint_quad(fill(bounds, flash));
            }
        });
    }
}

/// one underline per visible row of `link`, at the bottom of its cells
fn link_underlines(
    link: &Match,
    display_offset: usize,
    dims: &TerminalBounds,
) -> Vec<Bounds<Pixels>> {
    let (start, end) = (*link.start(), *link.end());
    let last_column = dims.num_columns().saturating_sub(1);
    (start.line.0..=end.line.0)
        .filter_map(|line| {
            let row = line + display_offset as i32;
            if row < 0 || row as usize >= dims.num_lines() {
                return None;
            }
            let first = if line == start.line.0 {
                start.column.0
            } else {
                0
            };
            let last = if line == end.line.0 {
                end.column.0
            } else {
                last_column
            };
            Some(Bounds::new(
                point(
                    dims.cell_width * first as f32,
                    dims.line_height * (row + 1) as f32 - px(2.),
                ),
                size(dims.cell_width * (last + 1 - first) as f32, px(1.)),
            ))
        })
        .collect()
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::index::{Column, Line};

    use super::*;

    /// 10 columns by 3 lines of 5x10 cells
    fn dims() -> TerminalBounds {
        TerminalBounds::new(
            px(10.),
            px(5.),
            Bounds::new(point(px(0.), px(0.)), size(px(50.), px(30.))),
        )
    }

    #[test]
    fn wrapped_link_is_underlined_on_each_visible_row() {
        let link: Match = AlacPoint::new(Line(-1), Column(7))..=AlacPoint::new(Line(1), Column(2));
        let underline = |x: f32, row: f32, cells: f32| {
            Bounds::new(
                point(px(5. * x), px(10. * (row + 1.) - 2.)),
                size(px(5. * cells), px(1.)),
            )
        };
        // its first row is in the history, out of view
        assert_eq!(
            link_underlines(&link, 0, &dims()),
            [underline(0., 0., 10.), underline(0., 1., 3.)]
        );
        // scrolled up a line: from its start column, the whole middle row, up to its end
        assert_eq!(
            link_underlines(&link, 1, &dims()),
            [
                underline(7., 0., 3.),
                underline(0., 1., 10.),
                underline(0., 2., 3.)
            ]
        );
        // scrolled up so far it is below the view
        assert!(link_underlines(&link, 5, &dims()).is_empty());
    }
}
