//! window frame, background and size, from the `window` settings

use std::time::Duration;

use gpui::{
    App, Bounds, Context, MouseButton, Pixels, Size, Window, WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, div, prelude::*, px, rems, size,
};

use super::Workspace;
use crate::{
    settings::{Decorations, ScrollbarEnable, Settings, StartupMode, WindowSize},
    theme::Theme,
    ui::terminal_element::cell_size,
};

// a resize sends many events, only the size it stops at is written
const SIZE_SAVE_DELAY: Duration = Duration::from_millis(500);

/// true when a pane with `tabs` tabs shows its tab bar
pub(super) fn shows_tab_bar(settings: &Settings, tabs: usize) -> bool {
    // without a frame the tab bar is the only place to move the window from
    settings.window.decorations != Decorations::Full
        || !(settings.hide_bar_for_one_tab && tabs == 1)
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
        Decorations::Full => WindowDecorations::Server,
        Decorations::None | Decorations::Client => WindowDecorations::Client,
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

/// window size that fits `window.columns` by `window.rows` cells of one tab
fn grid_window_size(cx: &App) -> Size<Pixels> {
    let settings = Settings::get(cx);
    let window = &settings.window;
    let cell = cell_size(px(settings.terminal.font.size), cx);
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
    size(
        (cell.width * window.columns as f32 + px(2. * window.padding.x + scrollbar_width)).ceil(),
        (cell.height * window.rows as f32 + px(2. * window.padding.y + tab_bar)).ceil(),
    )
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
        let WindowBounds::Windowed(bounds) = window.window_bounds() else {
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
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
            })
    }

    /// minimize, maximize and close buttons for client drawn frames
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
            .child(button("window-minimize", "−").on_click(|_, window, _| window.minimize_window()))
            .child(button("window-maximize", "□").on_click(|_, window, _| window.zoom_window()))
            .child(
                button("window-close", "×")
                    .on_click(cx.listener(|this, _, window, cx| this.request_quit(window, cx))),
            )
    }
}
