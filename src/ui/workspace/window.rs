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
    /// apply background settings to an open window, its frame is kept from startup
    pub(super) fn apply_window_settings(&self, window: &mut Window, cx: &App) {
        window.set_background_appearance(background_appearance(Settings::get(cx)));
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

    /// tabs of the top right pane, whose bars decide the row above the panes
    fn corner_tabs(&self) -> usize {
        self.layout
            .corner_pane_id()
            .and_then(|id| self.pane(id))
            .map_or(0, |pane| pane.tabs.len())
    }

    /// true when the window has a row above the panes
    pub(super) fn shows_top_row(&self, cx: &App) -> bool {
        shows_top_row(Settings::get(cx), self.corner_tabs())
    }

    /// rems taken at the top of the window by the row above the panes and the tab bar below it
    pub(super) fn top_bars_height(&self, cx: &App) -> f32 {
        let settings = Settings::get(cx);
        let tabs = self.corner_tabs();
        let top_row = if shows_top_row(settings, tabs) {
            MENU_BAR_HEIGHT
        } else {
            0.
        };
        let tab_bar = if shows_tab_bar(settings, tabs) {
            2.
        } else {
            0.
        };
        top_row + tab_bar
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
    use crate::ui::workspace::test_support::*;

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

    #[gpui::test]
    fn padding_moves_and_shrinks_the_grid(cx: &mut TestAppContext) {
        let settings = window_settings(
            r#"{"decorations": "server", "padding": {"x": 30, "y": 20}, "padding_balance": false}"#,
        );
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        cx.run_until_parked();
        let b = grid_bounds(&ws, cx);
        assert_eq!(b.bounds.origin, point(gpui::px(30.), gpui::px(20.)));
        assert_eq!(b.bounds.size, gpui::size(gpui::px(840.), gpui::px(560.)));
        assert_eq!(b.num_columns(), (840. / f32::from(b.cell_width)) as usize);
        assert_eq!(b.num_lines(), (560. / f32::from(b.line_height)) as usize);
    }

    #[gpui::test]
    fn balanced_padding_centers_the_grid(cx: &mut TestAppContext) {
        let settings = window_settings(
            r#"{"decorations": "server", "padding": {"x": 30, "y": 20}, "padding_balance": true}"#,
        );
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        cx.run_until_parked();
        let b = grid_bounds(&ws, cx);
        let spare_x = 840. % f32::from(b.cell_width);
        let spare_y = 560. % f32::from(b.line_height);
        // origins are snapped down to whole device pixels
        let expected_x = (30. + spare_x / 2.).floor();
        let expected_y = (20. + spare_y / 2.).floor();
        assert_eq!(f32::from(b.bounds.origin.x), expected_x);
        assert_eq!(f32::from(b.bounds.origin.y), expected_y);
        // the grid is never pushed past the padding on the far side
        let right = f32::from(b.bounds.origin.x + b.cell_width * b.num_columns() as f32);
        let bottom = f32::from(b.bounds.origin.y + b.line_height * b.num_lines() as f32);
        assert!(right <= 870., "{right}");
        assert!(bottom <= 580., "{bottom}");
    }

    #[gpui::test]
    fn startup_size_fits_columns_and_rows(cx: &mut TestAppContext) {
        let settings = window_settings(
            r#"{"decorations": "server", "padding": {"x": 7, "y": 3}, "columns": 83, "rows": 27,
                "startup": "windowed", "remember_size": false}"#,
        );
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        let bounds = cx.update(|_, cx| startup_bounds(cx));
        cx.simulate_resize(bounds.size);
        cx.run_until_parked();
        let b = grid_bounds(&ws, cx);
        assert_eq!((b.num_columns(), b.num_lines()), (83, 27));
        // the shell spawned before the window gets the same grid
        let early = cx.update(|_, cx| window::startup_grid(bounds.size, cx));
        assert_eq!((early.num_columns(), early.num_lines()), (83, 27));
    }

    #[gpui::test]
    fn startup_size_counts_the_tab_bar_without_frame(cx: &mut TestAppContext) {
        let settings = window_settings(
            r#"{"decorations": "none", "padding": {"x": 0, "y": 0}, "columns": 50, "rows": 10,
                "startup": "windowed", "remember_size": false}"#,
        );
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        let bounds = cx.update(|_, cx| startup_bounds(cx));
        cx.simulate_resize(bounds.size);
        cx.run_until_parked();
        assert!(cx.debug_bounds("drag-area").is_some(), "tab bar is hidden");
        let b = grid_bounds(&ws, cx);
        assert_eq!((b.num_columns(), b.num_lines()), (50, 10));
        let early = cx.update(|_, cx| window::startup_grid(bounds.size, cx));
        assert_eq!((early.num_columns(), early.num_lines()), (50, 10));
    }

    #[gpui::test]
    fn frameless_window_shows_tab_bar_with_drag_area(cx: &mut TestAppContext) {
        let settings = window_settings(r#"{"decorations": "server"}"#);
        let (_ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        assert!(cx.debug_bounds("drag-area").is_none());
        assert!(
            cx.debug_bounds("new-tab").is_none(),
            "one tab shows the bar"
        );

        let settings = window_settings(r#"{"decorations": "none"}"#);
        let (_ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        let area = cx.debug_bounds("drag-area").expect("no drag area");
        let tab = cx.debug_bounds("tab-title-0").expect("no tab bar");
        assert!(area.left() >= tab.right(), "drag area overlaps the tab");
        assert!(cx.debug_bounds("window-close").is_none());
    }

    #[gpui::test]
    fn expanded_tabs_leave_only_a_grip_to_drag_the_window_by(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"expand_tabs": true, "window": {"decorations": "none"},
                "terminal": {"scrollbar": {"enable": "off"}}}"#,
        )
        .unwrap();
        let (_ws, cx) = open_with_settings(cx, 2, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        let area = cx.debug_bounds("drag-area").expect("no drag area");
        let rem = cx.update(|window, _| window.rem_size());
        assert!(
            (area.size.width - rem * 3.).abs() < gpui::px(1.),
            "drag area is not a grip: {area:?}"
        );
        // the tabs take the rest of the row, the grip sits at its end
        assert!(area.left() > gpui::px(700.), "{area:?}");
        let tab = cx.debug_bounds("tab-title-1").expect("no tab bar");
        assert!(area.left() >= tab.right());
    }

    #[gpui::test]
    fn client_frame_draws_window_buttons_in_the_corner_pane(cx: &mut TestAppContext) {
        let settings = window_settings(r#"{"decorations": "client"}"#);
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        let ids = [
            "window-fullscreen",
            "window-minimize",
            "window-maximize",
            "window-close",
        ];
        for (id, next) in ids.iter().zip(ids.iter().skip(1)) {
            let button = cx.debug_bounds(id).unwrap_or_else(|| panic!("no {id}"));
            assert!(button.left() > gpui::px(450.), "{id} is not on the right");
            assert!(button.right() <= cx.debug_bounds(next).unwrap().left());
        }
        let fullscreen = cx.debug_bounds("window-fullscreen").unwrap().center();
        cx.simulate_click(fullscreen, Modifiers::none());
        assert!(cx.update(|window, _| window.is_fullscreen()));
        cx.simulate_click(fullscreen, Modifiers::none());
        assert!(!cx.update(|window, _| window.is_fullscreen()));
        run_actions(&ws, cx, r#""split_left""#);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        // the moved tab is on the left, the buttons stay at the right edge
        let close = cx.debug_bounds("window-close").expect("no close button");
        assert!(close.left() > gpui::px(450.));
        assert_eq!(ws.update(cx, |ws, _| ws.layout.panes().len()), 2);

        // the close button asks like any other quit
        start_program(&ws, cx);
        cx.simulate_click(close.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(dialog(&ws, cx).map(|d| d.0), Some(CloseTarget::Window));
    }

    #[cfg(not(target_os = "macos"))]
    #[gpui::test]
    fn one_tab_hides_the_tab_bar_under_a_top_row(cx: &mut TestAppContext) {
        let ids = |cx: &mut VisualTestContext| {
            ["tab-title-0", "menu-bar", "title-bar", "window-close"]
                .map(|id| cx.debug_bounds(id).is_some())
        };
        // the menu bar moves the window and holds the buttons
        let settings = window_settings(r#"{"decorations": "client", "menu_bar": "always"}"#);
        let (_, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        assert_eq!(ids(cx), [false, true, false, true]);
        let settings = window_settings(r#"{"decorations": "none", "menu_bar": "always"}"#);
        let (_, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        assert_eq!(ids(cx), [false, true, false, false]);
        // without menus a client frame gets an empty title bar
        let settings = window_settings(r#"{"decorations": "client", "menu_bar": "never"}"#);
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        assert_eq!(ids(cx), [false, false, true, true]);
        assert!(cx.debug_bounds("menu-drag-area").is_some());
        // a second tab brings the tab bar back with the buttons on it
        run_actions(&ws, cx, r#""new_tab""#);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        assert_eq!(ids(cx), [true, false, false, true]);
    }

    #[cfg(not(target_os = "macos"))]
    #[gpui::test]
    fn client_frame_puts_window_buttons_on_the_menu_bar(cx: &mut TestAppContext) {
        let settings = window_settings(r#"{"decorations": "client", "menu_bar": "always"}"#);
        let (_, cx) = open_with_settings(cx, 2, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        // the menu bar is the top row, the tab bar below it has no buttons
        let bar = cx.debug_bounds("menu-bar").expect("no menu bar");
        for id in [
            "window-fullscreen",
            "window-minimize",
            "window-maximize",
            "window-close",
        ] {
            let button = cx.debug_bounds(id).unwrap_or_else(|| panic!("no {id}"));
            assert!(
                button.bottom() <= bar.bottom(),
                "{id} is below the menu bar"
            );
            assert!(button.left() > gpui::px(450.), "{id} is not on the right");
        }
        // the free room between the menus and the buttons moves the window
        let area = cx.debug_bounds("menu-drag-area").expect("no drag area");
        assert!(area.top() >= bar.top() && area.bottom() <= bar.bottom());
        assert!(area.right() <= cx.debug_bounds("window-fullscreen").unwrap().left());
        // a framed window has neither
        let settings = window_settings(r#"{"decorations": "server", "menu_bar": "always"}"#);
        let (_, cx) = open_with_settings(cx, 2, "{}", settings);
        assert!(cx.debug_bounds("menu-bar").is_some());
        assert!(cx.debug_bounds("menu-drag-area").is_none());
        assert!(cx.debug_bounds("window-close").is_none());
    }

    #[test]
    fn corner_pane_is_top_right() {
        let leaf = |id| {
            Box::new(LayoutNode::Leaf(Pane {
                id,
                tabs: Vec::new(),
                active: 0,
                tab_scroll: ScrollHandle::new(),
            }))
        };
        assert_eq!(leaf(3).corner_pane_id(), Some(3));
        let side_by_side = LayoutNode::Split {
            axis: SplitAxis::Horizontal,
            ratio: 0.5,
            first: leaf(1),
            second: leaf(2),
        };
        assert_eq!(side_by_side.corner_pane_id(), Some(2));
        // right column split into top and bottom
        let nested = LayoutNode::Split {
            axis: SplitAxis::Horizontal,
            ratio: 0.5,
            first: leaf(1),
            second: Box::new(LayoutNode::Split {
                axis: SplitAxis::Vertical,
                ratio: 0.5,
                first: leaf(4),
                second: leaf(5),
            }),
        };
        assert_eq!(nested.corner_pane_id(), Some(4));
        assert_eq!(LayoutNode::Empty.corner_pane_id(), None);
    }

    #[test]
    fn opacity_and_blur_pick_background_appearance() {
        use gpui::WindowBackgroundAppearance as Appearance;
        for (json, expected) in [
            (r#"{"opacity": 1.0, "blur": true}"#, Appearance::Opaque),
            (
                r#"{"opacity": 0.8, "blur": false}"#,
                Appearance::Transparent,
            ),
            (r#"{"opacity": 0.8, "blur": true}"#, Appearance::Blurred),
        ] {
            assert_eq!(
                background_appearance(&window_settings(json)),
                expected,
                "{json}"
            );
        }
        for (mode, expected) in [
            ("server", gpui::WindowDecorations::Server),
            ("none", gpui::WindowDecorations::Client),
            ("client", gpui::WindowDecorations::Client),
        ] {
            let settings = window_settings(&format!(r#"{{"decorations": "{mode}"}}"#));
            assert_eq!(window_decorations(&settings), expected, "{mode}");
        }
    }
}
