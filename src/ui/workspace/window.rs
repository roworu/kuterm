//! window frame, background and size, from the `window` settings

use std::time::Duration;

use gpui::{
    AnyElement, App, Bounds, BoxShadow, Context, CursorStyle, Div, HitboxBehavior, MouseButton,
    MouseDownEvent, Pixels, Point, ResizeEdge, Size, Tiling, Window, WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, black, canvas, div, point, prelude::*, px, rems, size,
};

use super::{Workspace, menu_bar::MENU_BAR_HEIGHT, tab_bar::STATUS_BAR_HEIGHT};
use crate::{
    settings::{Decorations, ScrollbarEnable, Settings, StartupMode, WindowSize},
    terminal::TerminalBounds,
    theme::Theme,
    ui::terminal_element::cell_size,
};

// a resize sends many events, only the size it stops at is written
const SIZE_SAVE_DELAY: Duration = Duration::from_millis(500);

// nerd font icon of the fullscreen button
const FULLSCREEN_ICON: &str = "\u{f0293}";

// room around a client drawn frame for its shadow, dragging it resizes the window
const FRAME_SHADOW: Pixels = px(10.);

/// a press on an empty bar moves the window, a double click maximizes it
fn move_window(event: &MouseDownEvent, window: &mut Window, _: &mut App) {
    if event.click_count == 2 {
        window.zoom_window();
    } else {
        window.start_window_move();
    }
}

/// true when a pane with `tabs` tabs shows its tab bar
pub(super) fn shows_tab_bar(settings: &Settings, tabs: usize) -> bool {
    let window = &settings.window;
    // without a frame or a menu bar the tab bar is the only place to move the window from
    !(settings.hide_bar_for_one_tab && tabs == 1)
        || (window.decorations == Decorations::None && !window.menu_bar.shown())
}

/// true when the window has a row above the panes: the menu bar, or for a client frame a title
/// bar with the window buttons while the tab bar of the corner pane with `corner_tabs` is hidden
pub(super) fn shows_top_row(settings: &Settings, corner_tabs: usize) -> bool {
    settings.window.menu_bar.shown()
        || (settings.window.decorations == Decorations::Client
            && !shows_tab_bar(settings, corner_tabs))
}

/// window background the platform should prepare for `opacity` and `blur`
pub fn background_appearance(settings: &Settings) -> WindowBackgroundAppearance {
    let window = &settings.window;
    if window.opacity >= 1. {
        WindowBackgroundAppearance::Opaque
    } else if window.blur {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Transparent
    }
}

/// who draws the window frame
pub fn window_decorations(settings: &Settings) -> WindowDecorations {
    match settings.window.decorations {
        Decorations::Server => WindowDecorations::Server,
        Decorations::None | Decorations::Client => WindowDecorations::Client,
    }
}

/// shadow and resize border around a client drawn frame, desktops draw neither for it
pub(super) fn client_frame(content: Div, window: &mut Window, cx: &App) -> AnyElement {
    let settings = &Settings::get(cx).window;
    let frame = match window.window_decorations() {
        gpui::Decorations::Client { tiling } => Some(tiling),
        gpui::Decorations::Server => None,
    };
    match (settings.decorations, frame) {
        (Decorations::Client, Some(_)) => window.set_client_inset(FRAME_SHADOW),
        // compositors flip the mode at startup and gpui ignores the inset meanwhile. dropping it
        // would leave the window geometry stale and the size drifting once the frame is back
        (Decorations::Client, None) => {}
        _ => window.set_client_inset(px(0.)),
    }
    let Some(tiling) = frame.filter(|_| settings.decorations == Decorations::Client) else {
        return content.into_any_element();
    };
    // the shadow is drawn under the whole frame and would darken a see through background
    let shadow = !tiling.is_tiled() && settings.opacity >= 1.;
    div()
        .id("client-frame")
        .size_full()
        .when(!tiling.top, |frame| frame.pt(FRAME_SHADOW))
        .when(!tiling.bottom, |frame| frame.pb(FRAME_SHADOW))
        .when(!tiling.left, |frame| frame.pl(FRAME_SHADOW))
        .when(!tiling.right, |frame| frame.pr(FRAME_SHADOW))
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            let areas = resize_areas(window.viewport_size(), tiling);
            // corners come last and win over the edges they overlap
            if let Some((edge, _)) = areas
                .iter()
                .rev()
                .find(|(_, b)| b.contains(&event.position))
            {
                window.start_window_resize(*edge);
            }
        })
        .child(content.when(shadow, |content| {
            content.shadow(vec![
                BoxShadow::new(px(0.), px(0.), black().opacity(0.4)).blur_radius(FRAME_SHADOW / 2.),
            ])
        }))
        .child(
            canvas(
                move |bounds, window, _| {
                    resize_areas(bounds.size, tiling)
                        .into_iter()
                        .map(|(edge, area)| {
                            (edge, window.insert_hitbox(area, HitboxBehavior::Normal))
                        })
                        .collect::<Vec<_>>()
                },
                |_, hitboxes, window, _| {
                    for (edge, hitbox) in hitboxes {
                        window.set_cursor_style(resize_cursor(edge), &hitbox);
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .into_any_element()
}

/// parts of the shadow border of a `window` sized window that resize it, edges then corners
fn resize_areas(window: Size<Pixels>, tiling: Tiling) -> Vec<(ResizeEdge, Bounds<Pixels>)> {
    let (w, h, e) = (window.width, window.height, FRAME_SHADOW);
    let area = |x, y, w, h| Bounds::new(point(x, y), size(w, h));
    let zero = px(0.);
    [
        (!tiling.top, ResizeEdge::Top, area(zero, zero, w, e)),
        (!tiling.bottom, ResizeEdge::Bottom, area(zero, h - e, w, e)),
        (!tiling.left, ResizeEdge::Left, area(zero, zero, e, h)),
        (!tiling.right, ResizeEdge::Right, area(w - e, zero, e, h)),
        (
            !tiling.top && !tiling.left,
            ResizeEdge::TopLeft,
            area(zero, zero, e, e),
        ),
        (
            !tiling.top && !tiling.right,
            ResizeEdge::TopRight,
            area(w - e, zero, e, e),
        ),
        (
            !tiling.bottom && !tiling.left,
            ResizeEdge::BottomLeft,
            area(zero, h - e, e, e),
        ),
        (
            !tiling.bottom && !tiling.right,
            ResizeEdge::BottomRight,
            area(w - e, h - e, e, e),
        ),
    ]
    .into_iter()
    .filter_map(|(free, edge, area)| free.then_some((edge, area)))
    .collect()
}

/// pointer shown over a resize area
fn resize_cursor(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

/// bounds the window opens with, `window.startup` is applied once it is shown
pub fn startup_bounds(cx: &App) -> Bounds<Pixels> {
    let window = &Settings::get(cx).window;
    let size = window
        .remember_size
        .then(WindowSize::load)
        .flatten()
        .map(|saved| size(px(saved.width), px(saved.height)))
        .unwrap_or_else(|| grid_window_size(cx));
    Bounds::centered(None, size, cx)
}

/// maximize or fullscreen a window that just opened, as `window.startup` says
pub(super) fn apply_startup_mode(window: &mut Window, cx: &App) {
    let startup = Settings::get(cx).window.startup;
    // gpui asks for these before x11 maps the window, which window managers ignore,
    // so they wait for the first frame
    window.on_next_frame(move |window, _| match startup {
        StartupMode::Windowed => {}
        StartupMode::Maximized => window.zoom_window(),
        StartupMode::Fullscreen => window.toggle_fullscreen(),
    });
}

/// space around the grid of a window with one tab: padding, scrollbar, tab bar, menu bar and
/// status bar
fn grid_margin(settings: &Settings) -> Size<Pixels> {
    let window = &settings.window;
    let scrollbar = &settings.terminal.scrollbar;
    let scrollbar_width = match scrollbar.enable {
        ScrollbarEnable::Off => 0.,
        _ => scrollbar.width,
    };
    // the tab bar is 2 rems high and rems are the ui font size
    let tab_bar = if shows_tab_bar(settings, 1) {
        2. * settings.ui_font_size
    } else {
        0.
    };
    let menu_bar = if shows_top_row(settings, 1) {
        MENU_BAR_HEIGHT * settings.ui_font_size
    } else {
        0.
    };
    let status_bar = if settings.status_bar.enable {
        STATUS_BAR_HEIGHT * settings.ui_font_size
    } else {
        0.
    };
    size(
        px(2. * window.padding.x + scrollbar_width),
        px(2. * window.padding.y + tab_bar + menu_bar + status_bar),
    )
}

/// window size that fits `window.columns` by `window.rows` cells of one tab
fn grid_window_size(cx: &App) -> Size<Pixels> {
    let settings = Settings::get(cx);
    let window = &settings.window;
    let cell = cell_size(px(settings.terminal.font.size), cx);
    let margin = grid_margin(settings);
    size(
        (cell.width * window.columns as f32 + margin.width).ceil(),
        (cell.height * window.rows as f32 + margin.height).ceil(),
    )
}

/// grid of the only tab in a window of `window_size`, before the window lays it out
pub(super) fn startup_grid(window_size: Size<Pixels>, cx: &App) -> TerminalBounds {
    let settings = Settings::get(cx);
    let cell = cell_size(px(settings.terminal.font.size), cx);
    let margin = grid_margin(settings);
    // same floors as prepaint, alacritty panics on a grid without rows
    let grid = size(
        (window_size.width - margin.width).max(cell.width * 2.),
        (window_size.height - margin.height).max(cell.height),
    );
    TerminalBounds::new(cell.height, cell.width, Bounds::new(Point::default(), grid))
}

impl Workspace {
    /// apply background and frame settings to an open window
    pub(super) fn apply_window_settings(&self, window: &mut Window, cx: &App) {
        let settings = Settings::get(cx);
        window.set_background_appearance(background_appearance(settings));
        window.request_decorations(window_decorations(settings));
    }

    /// remember the windowed size once resizing stops
    pub(super) fn window_resized(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // without the client frame's shadow, the size the window opens with
        let WindowBounds::Windowed(bounds) = window.inner_window_bounds() else {
            return;
        };
        // x11 still reports fullscreen and maximized windows as windowed
        if !Settings::get(cx).window.remember_size
            || window.is_fullscreen()
            || window.is_maximized()
        {
            return;
        }
        let saved = WindowSize {
            width: bounds.size.width.into(),
            height: bounds.size.height.into(),
        };
        let executor = cx.background_executor().clone();
        // replacing the task drops the save still waiting for the last event
        self._size_save = cx.background_executor().spawn(async move {
            executor.timer(SIZE_SAVE_DELAY).await;
            saved.save();
        });
    }

    /// true when the window has a row above the panes
    pub(super) fn shows_top_row(&self, cx: &App) -> bool {
        let corner_tabs = self
            .layout
            .corner_pane_id()
            .and_then(|id| self.pane(id))
            .map_or(0, |pane| pane.tabs.len());
        shows_top_row(Settings::get(cx), corner_tabs)
    }

    /// top row with the menus, and without a frame room to move the window and its buttons
    pub(super) fn render_menu_row(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let window = &Settings::get(cx).window;
        let (decorations, menus) = (window.decorations, window.menu_bar.shown());
        div()
            .id("menu-bar")
            .debug_selector(move || if menus { "menu-bar" } else { "title-bar" }.into())
            .flex()
            .flex_none()
            .w_full()
            .h(rems(MENU_BAR_HEIGHT))
            .bg(theme.tab_bar_background)
            .border_b_1()
            .border_color(theme.border)
            .when(menus, |row| row.child(self.menu_bar.clone()))
            .when(decorations != Decorations::Server, |row| {
                row.child(
                    div()
                        .id("menu-drag-area")
                        .debug_selector(|| "menu-drag-area".into())
                        .flex_1()
                        .h_full()
                        .on_mouse_down(MouseButton::Left, move_window),
                )
            })
            .when(decorations == Decorations::Client, |row| {
                row.child(self.render_window_buttons(cx))
            })
    }

    /// empty part of a tab bar, it moves the window when there is no frame
    pub(super) fn render_drag_area(&self, pane: usize, cx: &Context<Self>) -> impl IntoElement {
        div()
            .id(("drag-area", pane))
            .debug_selector(|| "drag-area".into())
            .h_full()
            .min_w(rems(3.))
            // expanded tabs fill the row, so only a grip is left next to them
            .map(|area| {
                if Settings::get(cx).expand_tabs {
                    area.flex_none().w(rems(3.))
                } else {
                    area.flex_1()
                }
            })
            .on_mouse_down(MouseButton::Left, move_window)
    }

    /// fullscreen, minimize, maximize and close buttons for client drawn frames
    pub(super) fn render_window_buttons(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex()
                .items_center()
                .px_3()
                .text_color(theme.text_muted)
                .hover(|button| button.text_color(theme.text))
                .child(label)
        };
        div()
            .flex()
            .flex_none()
            .child(
                button("window-fullscreen", FULLSCREEN_ICON)
                    .on_click(|_, window, _| window.toggle_fullscreen()),
            )
            .child(button("window-minimize", "−").on_click(|_, window, _| window.minimize_window()))
            .child(button("window-maximize", "□").on_click(|_, window, _| window.zoom_window()))
            .child(
                button("window-close", "×")
                    .on_click(cx.listener(|this, _, window, cx| this.request_quit(window, cx))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_areas_cover_free_edges_of_the_shadow() {
        let window = size(px(300.), px(200.));
        let at = |tiling, x: f32, y: f32| {
            resize_areas(window, tiling)
                .into_iter()
                .rev()
                .find(|(_, area)| area.contains(&point(px(x), px(y))))
                .map(|(edge, _)| edge)
        };
        let free = Tiling::default();
        assert_eq!(at(free, 150., 5.), Some(ResizeEdge::Top));
        assert_eq!(at(free, 150., 195.), Some(ResizeEdge::Bottom));
        assert_eq!(at(free, 5., 100.), Some(ResizeEdge::Left));
        assert_eq!(at(free, 295., 100.), Some(ResizeEdge::Right));
        assert_eq!(at(free, 5., 5.), Some(ResizeEdge::TopLeft));
        assert_eq!(at(free, 295., 5.), Some(ResizeEdge::TopRight));
        assert_eq!(at(free, 5., 195.), Some(ResizeEdge::BottomLeft));
        assert_eq!(at(free, 295., 195.), Some(ResizeEdge::BottomRight));
        // the window itself starts past the shadow
        assert_eq!(at(free, 15., 15.), None);
        // a tiled edge has no shadow to grab
        let left = Tiling {
            left: true,
            ..Tiling::default()
        };
        assert_eq!(at(left, 5., 100.), None);
        assert_eq!(at(left, 5., 5.), Some(ResizeEdge::Top));
        assert!(resize_areas(window, Tiling::tiled()).is_empty());
    }
}
