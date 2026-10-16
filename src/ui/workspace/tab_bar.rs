//! drawing the workspace: tab bar on top, active terminal below
// TODO: need a setting on where to put tabs (top, bottom, left?, right?)

use std::{cell::RefCell, collections::HashMap, ops::Range, sync::LazyLock};

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, CursorStyle, DispatchPhase, Div, DragMoveEvent,
    ExternalPaths, KeyDownEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PlatformInput, Point, ScrollHandle, Stateful, StyleRefinement,
    Window, anchored, canvas, deferred, div, prelude::*, px, relative, rems,
};
use skrifa::{
    FontRef, MetadataProvider,
    instance::{LocationRef, Size},
};

use super::{
    DropSide, LayoutNode, Pane, SplitAxis, Workspace,
    end_truncated::EndTruncated,
    tab_title::{self, Piece},
    window::{client_frame, shows_tab_bar},
};
use crate::{
    settings::{BellTarget, Decorations, NewTabButton, Settings, TabTitleAlign},
    terminal::CPU_HISTORY,
    theme::Theme,
    ui::terminal_view::open_link,
};

// tab bar text size in rems, the icon offset is scaled by it
const TEXT_SIZE: f32 = 0.875;

/// status bar height in rems
pub(super) const STATUS_BAR_HEIGHT: f32 = 1.5;
// status bar text size in rems, smaller than the tabs so the bar stays quiet
const STATUS_TEXT_SIZE: f32 = 0.75;

// nerd font bell, shown on a background tab that rang
const BELL_ICON: &str = "\u{f0f3}";

// narrowest a divider is to grab, a thinner one is widened by an invisible handle
const DIVIDER_GRAB: f32 = 6.;

/// how far `icon` must move down, in ems, for its lowest point to stand on the baseline
fn icon_drop(icon: &str) -> f32 {
    // every tab asks on every frame with output, and there are only a few distinct icons
    thread_local! {
        static DROPS: RefCell<HashMap<String, f32>> = RefCell::default();
    }
    DROPS.with_borrow_mut(|drops| {
        if let Some(drop) = drops.get(icon) {
            return *drop;
        }
        let drop = measure_icon_drop(icon);
        drops.insert(icon.to_owned(), drop);
        drop
    })
}

fn measure_icon_drop(icon: &str) -> f32 {
    // nerd font icons are centered on the line, not set on the baseline like letters.
    // gpui can't measure glyph outlines on linux, so they are read from the bundled font
    static FONT: LazyLock<Option<FontRef<'static>>> =
        LazyLock::new(|| FontRef::new(crate::FONT_REGULAR).ok());
    let Some(font) = FONT.as_ref() else {
        return 0.;
    };
    let charmap = font.charmap();
    let glyphs = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    let units_per_em = font
        .metrics(Size::unscaled(), LocationRef::default())
        .units_per_em as f32;
    icon.chars()
        .filter_map(|ch| glyphs.bounds(charmap.map(ch)?))
        .map(|bounds| bounds.y_min)
        .reduce(f32::min)
        .map_or(0., |y_min| y_min / units_per_em)
}

/// cpu graph as thin bars, newest on the right. older bars fade, busier ones turn from green
/// to yellow to red, and slots without samples yet are a flat line
fn graph_bars(graph: &[f32], theme: &Theme) -> Div {
    let empty = CPU_HISTORY.saturating_sub(graph.len());
    let newest = &graph[graph.len().saturating_sub(CPU_HISTORY)..];
    let bar = || div().flex_none().w(px(3.)).rounded_t(px(1.));
    let bars = newest.iter().enumerate().map(|(ix, &value)| {
        let color = match value {
            v if v < 0.5 => theme.ansi[2],
            v if v < 0.8 => theme.ansi[3],
            _ => theme.ansi[1],
        };
        let age = (empty + ix) as f32 / (CPU_HISTORY - 1).max(1) as f32;
        bar()
            .h(relative(value.max(0.08)))
            .bg(color.opacity(0.35 + 0.65 * age))
    });
    div()
        .flex()
        .items_end()
        .gap(px(1.))
        .h(rems(0.9))
        .px(px(2.))
        .pt(px(2.))
        .rounded_sm()
        // a faint track, so the graph reads as one widget even while idle
        .bg(theme.text_muted.opacity(0.1))
        .children((0..empty).map(|_| bar().h(px(1.)).bg(theme.border)))
        .children(bars)
}

/// floating menu at the pointer, closed by a click anywhere else
pub(super) fn popup_menu(id: &'static str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .flex()
        .flex_col()
        .py_1()
        .bg(theme.tab_bar_background)
        .border_1()
        .border_color(theme.border)
        .text_color(theme.text)
        // keeps clicks from reaching the terminal below
        .occlude()
}

/// one row of a popup menu
pub(super) fn menu_item(
    id: impl Into<gpui::ElementId>,
    label: &str,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .whitespace_nowrap()
        .hover(|item| item.bg(theme.tab_active_background))
        .child(label.to_owned())
}

/// row holding the tabs, fills the bar next to the new tab button
fn tab_row(scroll: &ScrollHandle) -> Stateful<Div> {
    div()
        .id("tabs")
        .flex()
        .flex_1()
        .min_w_0()
        .h_full()
        // the wheel scrolls hidden tabs into view, it does nothing while all tabs fit.
        // expanded tabs can't shrink below their padding, so many of them overflow too
        .overflow_x_scroll()
        .track_scroll(scroll)
}

/// title text with `icon` drawn at each byte offset of `icon_at`, the link modifier and a click
/// on a port of `ports_at` opens it
fn title_with_icons(
    ix: usize,
    title: &str,
    icon: &str,
    icon_at: &[usize],
    ports_at: &[(Range<usize>, u16)],
) -> Div {
    let drop = rems(icon_drop(icon) * TEXT_SIZE);
    // shrinks below its text, so long titles are cut at the end whatever the align
    let text = |text: &str| {
        div()
            .debug_selector(move || format!("tab-text-{ix}"))
            .flex()
            .min_w_0()
            .overflow_hidden()
            .child(EndTruncated::new(text.to_owned()))
    };
    let mut row = div()
        .debug_selector(move || format!("tab-title-{ix}"))
        .flex()
        .items_center()
        .gap_1()
        .min_w_0()
        .overflow_hidden();
    let mut start = 0;
    let mut icons = icon_at.iter().copied().peekable();
    let mut ports = ports_at.iter().peekable();
    loop {
        let next_port = ports.peek().map(|(range, _)| range.start);
        // an icon goes before a port at the same offset, the blocks came in that order
        if let Some(at) = icons.next_if(|&at| next_port.is_none_or(|port| at <= port)) {
            if at > start {
                row = row.child(text(&title[start..at]));
                start = at;
            }
            row = row.child(
                div()
                    .debug_selector(move || format!("tab-icon-{ix}"))
                    .flex_none()
                    .relative()
                    .top(drop)
                    .child(icon.to_owned()),
            );
        } else if let Some((range, port)) = ports.next() {
            if range.start > start {
                row = row.child(text(&title[start..range.start]));
            }
            let port = *port;
            row = row.child(
                div()
                    .debug_selector(move || format!("tab-port-{port}"))
                    .flex_none()
                    .child(title[range.clone()].to_owned())
                    .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                        if Settings::get(cx).links.modifier.held(&event.modifiers) {
                            open_link(format!("http://localhost:{port}"), cx);
                            // the tab under it is neither picked nor dragged
                            cx.stop_propagation();
                        }
                    }),
            );
            start = range.end;
        } else {
            break;
        }
    }
    if start < title.len() {
        row = row.child(text(&title[start..]));
    }
    row
}

/// tab sized from settings, with its title and icons
#[cfg(test)]
fn tab(
    ix: usize,
    title: &str,
    icon: &str,
    icon_at: &[usize],
    ports_at: &[(Range<usize>, u16)],
    settings: &Settings,
) -> Stateful<Div> {
    tab_with(
        ix,
        title_with_icons(ix, title, icon, icon_at, ports_at),
        false,
        settings,
    )
}

/// tab sized from settings around `content`, a compact one only as wide as its content
fn tab_with(
    ix: usize,
    content: impl IntoElement,
    compact: bool,
    settings: &Settings,
) -> Stateful<Div> {
    let expand = settings.expand_tabs && !compact;
    let title = div()
        .flex()
        .flex_1()
        .min_w_0()
        .map(|row| match settings.tab_title_align {
            TabTitleAlign::Left => row.justify_start(),
            TabTitleAlign::Center => row.justify_center(),
            TabTitleAlign::Right => row.justify_end(),
        })
        .child(content);
    div()
        .id(("tab", ix))
        .flex()
        .items_center()
        .gap_1()
        .h_full()
        .px_3()
        // zero basis with flex_1 splits the row equally, whatever the titles are
        .when(expand, |tab| tab.flex_1().min_w_0())
        // keep their width and overflow the row instead of squeezing together
        .when(!expand && !compact, |tab| {
            tab.flex_none().w(px(settings.tab_width as f32))
        })
        .when(compact, |tab| tab.flex_none())
        .child(title)
}

/// a tab picked up with the mouse, drawn as its own preview while dragging
#[derive(Clone)]
pub(super) struct DraggedTab {
    /// pane the tab was dragged out of
    pub(super) pane: usize,
    pub(super) ix: usize,
    title: String,
    icon: String,
    icon_at: Vec<usize>,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let settings = Settings::get(cx);
        div()
            .id("dragged-tab")
            .flex()
            .items_center()
            .gap_1()
            .h(rems(2.))
            .px_3()
            .max_w(px(settings.tab_width as f32))
            .whitespace_nowrap()
            .overflow_hidden()
            .bg(theme.tab_active_background)
            .text_color(theme.text)
            .border_1()
            .border_color(theme.border)
            .child(title_with_icons(
                self.ix,
                &self.title,
                &self.icon,
                &self.icon_at,
                &[],
            ))
    }
}

/// divider handle, dragged to resize a split. the preview it draws is never seen
#[derive(Clone)]
struct SplitDrag;

impl Render for SplitDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// translucent half pane showing where a dropped tab splits
fn drop_overlay(side: DropSide, theme: &Theme) -> AnyElement {
    div()
        .absolute()
        .bg(theme.text.opacity(0.15))
        .map(|overlay| match side {
            DropSide::Left => overlay.top_0().bottom_0().left_0().w(relative(0.5)),
            DropSide::Right => overlay.top_0().bottom_0().right_0().w(relative(0.5)),
            DropSide::Top => overlay.left_0().right_0().top_0().h(relative(0.5)),
            DropSide::Bottom => overlay.left_0().right_0().bottom_0().h(relative(0.5)),
        })
        .into_any_element()
}

impl Workspace {
    /// title shown for tab `ix` of pane `pane_id`, also listed by the tab picker
    pub(super) fn tab_title(&self, pane_id: usize, ix: usize, cx: &App) -> String {
        let tab_state = &self.pane(pane_id).expect("pane from caller").tabs[ix];
        if let Some(custom) = &tab_state.custom_title {
            return custom.clone();
        }
        // the program title stands in until the first refresh, or when the blocks are empty
        if tab_state.title.is_empty() {
            tab_state
                .view
                .read(cx)
                .terminal()
                .read(cx)
                .title(&Settings::get(cx).default_title)
        } else {
            tab_state.title.clone()
        }
    }

    fn render_tab(&self, pane: usize, ix: usize, cx: &Context<Self>) -> Stateful<Div> {
        let is_active = self.pane(pane).is_some_and(|state| ix == state.active);
        let settings = Settings::get(cx);
        let title = tab_title::spin(&self.tab_title(pane, ix, cx), self.spinner_frame);
        let tab_state = &self.pane(pane).expect("pane from caller").tabs[ix];
        // a tab with a profile theme takes its colors, so profiles are easy to tell apart
        let terminal = tab_state.view.read(cx).terminal().read(cx);
        let profile_theme = terminal.profile_theme();
        let theme = terminal.theme(cx);
        let indicator = theme.text;
        let icon = tab_state.icon.clone();
        let pinned = tab_state.pinned;
        let id = tab_state.view.entity_id();
        let renaming = self
            .renaming
            .as_ref()
            .filter(|(renamed, _)| *renamed == id)
            .map(|(_, input)| input.clone());
        // a title of its own or a pinned tab keeps the icon in front, when the blocks show one
        let (title, icon_at, ports_at) = match (&tab_state.custom_title, pinned) {
            (Some(custom), _) => {
                let icon_at = if tab_state.icon_at.is_empty() {
                    vec![]
                } else {
                    vec![0]
                };
                (custom.clone(), icon_at, Vec::new())
            }
            (None, true) => (String::new(), vec![0], Vec::new()),
            (None, false) => (title, tab_state.icon_at.clone(), tab_state.ports_at.clone()),
        };
        let bell = tab_state.bell;
        let tab_color = tab_state.color;
        let waiting = tab_state.attention.waiting(terminal.inputs()).is_some();
        let queued = terminal.queue().entries().len();
        let flash = tab_state
            .view
            .read(cx)
            .bell_color(cx)
            .filter(|_| settings.bell.visual.target == BellTarget::Tab);
        let background = match profile_theme {
            Some(profile) => profile.terminal_background,
            None if is_active => theme.tab_active_background,
            None => theme.tab_bar_background,
        };
        // a spinner frame has the same length, so the ranges still fit
        let content = match renaming {
            Some(input) => div()
                .id(("rename-tab", ix))
                .debug_selector(move || format!("rename-tab-{ix}"))
                .flex_1()
                .min_w(rems(6.))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "enter" => this.finish_rename(true, window, cx),
                        "escape" => this.finish_rename(false, window, cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                }))
                // clicks in the field move its caret, they don't pick or drag the tab
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(
                    cx.listener(|this, _, window, cx| this.finish_rename(true, window, cx)),
                )
                .child(input)
                .into_any_element(),
            None => title_with_icons(ix, &title, &icon, &icon_at, &ports_at).into_any_element(),
        };
        let drag = settings.drag_tabs && self.renaming.is_none();
        let group_color = self.group_color_of(pane, ix, cx);
        tab_with(ix, content, pinned, settings)
            .relative()
            // a line in the group color under the tabs of a group
            .when_some(group_color, |tab, color| {
                tab.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(color),
                )
            })
            .group("tab")
            .border_r_1()
            .border_color(theme.border)
            // the terminal background, not the bright tab colors, so the tab blends with its pane
            .when_some(profile_theme, |tab, theme| {
                tab.bg(theme.terminal_background)
            })
            .when(is_active && profile_theme.is_none(), |tab| {
                tab.bg(theme.tab_active_background)
            })
            // a color set from outside, like a script marking its tab
            .when_some(tab_color, |tab, color| tab.bg(color))
            .when_some(flash, |tab, flash| tab.bg(background.blend(flash)))
            .text_color(if is_active {
                theme.text
            } else {
                theme.text_muted
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.activate_tab(pane, ix, window, cx);
                if event.click_count() == 2 {
                    this.start_rename(pane, ix, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.tab_menu = Some((id, event.position));
                    cx.notify();
                }),
            )
            // pinned tabs only close on purpose
            .when(!pinned, |tab| {
                tab.on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| {
                        this.request_close_tab(pane, ix, window, cx)
                    }),
                )
            })
            .when(drag, |tab| {
                tab.on_drag(
                    DraggedTab {
                        pane,
                        ix,
                        title,
                        icon,
                        icon_at,
                    },
                    |dragged: &DraggedTab, _, _, cx| cx.new(|_| dragged.clone()),
                )
                .drag_over::<DraggedTab>(move |style, dragged, _, _| {
                    // an insertion marker only makes sense inside the same tab row
                    if dragged.pane != pane {
                        style
                    } else if dragged.ix < ix {
                        // remove then insert at ix puts the tab after ix when moving right
                        style.border_r_2().border_color(indicator)
                    } else if dragged.ix > ix {
                        style.border_l_2().border_color(indicator)
                    } else {
                        style
                    }
                })
                .on_drop(cx.listener(
                    move |this, dragged: &DraggedTab, window, cx| {
                        this.move_tab_to_pane(dragged.pane, dragged.ix, pane, ix, window, cx);
                    },
                ))
            })
            .when(bell, |tab| {
                tab.child(
                    div()
                        .debug_selector(move || format!("tab-bell-{ix}"))
                        .flex_none()
                        .text_color(theme.text)
                        .child(BELL_ICON),
                )
            })
            .when(queued > 0, |tab| {
                tab.child(
                    div()
                        .debug_selector(move || format!("tab-queue-{ix}"))
                        .flex_none()
                        .text_xs()
                        .child(format!("⧗{queued}")),
                )
            })
            .when(waiting, |tab| {
                tab.child(
                    div()
                        .debug_selector(move || format!("tab-attention-{ix}"))
                        .flex_none()
                        .size(rems(0.5))
                        .rounded_full()
                        .bg(theme.attention),
                )
            })
            .when(settings.show_tab_close_button && !pinned, |tab| {
                tab.child(
                    div()
                        .id(("close-tab", ix))
                        .debug_selector(move || format!("close-tab-{ix}"))
                        .px_1()
                        .rounded_sm()
                        .invisible()
                        .group_hover("tab", |close| close.visible())
                        .when(is_active, |close| close.visible())
                        .hover(|close| close.bg(theme.border))
                        .child("×")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.request_close_tab(pane, ix, window, cx);
                        })),
                )
            })
    }

    /// "+" button of pane `pane_id`, opening a new tab there
    fn render_new_tab_button(&self, pane: usize, cx: &Context<Self>) -> Stateful<Div> {
        let theme = Theme::get(cx);
        let has_profiles = Settings::get(cx).profiles.len() > 1;
        div()
            .id(("new-tab", pane))
            .debug_selector(|| "new-tab".into())
            .flex()
            .items_center()
            .px_3()
            .text_color(theme.text_muted)
            .hover(|button| button.text_color(theme.text))
            .child("+")
            .on_click(cx.listener(move |this, _, window, cx| {
                let profile = Settings::get(cx).default_profile().clone();
                this.add_profile_tab(pane, &profile, true, window, cx);
            }))
            .when(has_profiles, |button| {
                button
                    // hints that a right click picks another profile
                    .child(
                        div()
                            .debug_selector(|| "profile-hint".into())
                            .ml_0p5()
                            .text_xs()
                            .child("▾"),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            this.profile_menu = Some((pane, event.position));
                            cx.notify();
                        }),
                    )
            })
    }

    /// one pane: its tab row on top and its active terminal below
    fn render_pane(&self, pane: &Pane, cx: &Context<Self>) -> AnyElement {
        let settings = Settings::get(cx);
        let theme = Theme::get(cx);
        let pane_id = pane.id;
        let show_bar = shows_tab_bar(settings, pane.tabs.len());
        let flash = pane
            .tabs
            .get(pane.active)
            .and_then(|tab| tab.view.read(cx).bell_color(cx))
            .filter(|_| settings.bell.visual.target == BellTarget::Tab);
        let framed = settings.window.decorations == Decorations::Server;
        // fixed width tabs only take the room they need, so the button follows the last one.
        // expanded tabs fill the row anyway, which puts the button at the right end.
        // without a frame the room after the tabs is left free for moving the window
        let hug_tabs = (settings.new_tab_button == NewTabButton::AfterTabs || !framed)
            && !settings.expand_tabs;
        let tab_row = tab_row(&pane.tab_scroll)
            .when(hug_tabs, |row| row.flex_initial())
            .children((0..pane.tabs.len()).flat_map(|ix| {
                // a group's chip goes before its first tab, a collapsed group hides its tabs
                let chip = self.render_group_chip(pane_id, ix, cx);
                let tab = (!self.hidden_in_group(pane_id, ix))
                    .then(|| self.render_tab(pane_id, ix, cx).into_any_element());
                chip.into_iter().chain(tab)
            }));
        let new_tab = self.render_new_tab_button(pane_id, cx);
        let drag_area = (!framed).then(|| self.render_drag_area(pane_id, cx));
        let bar = div()
            .flex()
            .flex_none()
            .h(rems(2.))
            .bg(theme.tab_bar_background)
            .border_b_1()
            .border_color(theme.border)
            .map(|bar| match settings.new_tab_button {
                NewTabButton::Left => bar.child(new_tab).child(tab_row).children(drag_area),
                NewTabButton::Right => bar.child(tab_row).children(drag_area).child(new_tab),
                NewTabButton::AfterTabs => bar.child(tab_row).child(new_tab).children(drag_area),
            })
            // the menu bar is the top row when shown, so the buttons go there instead
            .when(
                settings.window.decorations == Decorations::Client
                    && !settings.window.menu_bar.shown()
                    && self.layout.corner_pane_id() == Some(pane_id),
                |bar| bar.child(self.render_window_buttons(cx)),
            );
        // only a pane without focus next to others is dimmed
        let dim =
            if pane_id != self.focused && self.zoomed.is_none() && self.layout.pane_ids().len() > 1
            {
                settings.panes.dim_inactive
            } else {
                0.
            };
        let zone = self
            .drop_zone
            .filter(|(id, _)| *id == pane_id)
            .map(|(_, side)| side);
        let terminal = div()
            .debug_selector(move || format!("pane-terminal-{pane_id}"))
            .flex_1()
            .min_h_0()
            .relative()
            // drops land on the terminal area, so dropping on the tab row only reorders
            .on_drag_move::<DraggedTab>(cx.listener(
                move |this, event: &DragMoveEvent<DraggedTab>, _, cx| {
                    this.update_drop_zone(pane_id, event.event.position, event.bounds, cx);
                },
            ))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.drop_tab_on_pane(pane_id, dragged.pane, dragged.ix, window, cx);
            }))
            // files from other apps are typed into the pane they land on, which takes focus
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
                this.focus_pane(pane_id, window, cx);
                if let Some(pane) = this.pane(pane_id)
                    && let Some(tab) = pane.tabs.get(pane.active)
                {
                    tab.view.update(cx, |view, cx| view.drop_paths(paths, cx));
                }
            }))
            // cached, so output or a redraw in one pane does not lay out the grids of the others
            .children(pane.tabs.get(pane.active).map(|tab| {
                // gpui only redraws a cached view on notify while the window tracks it, and a
                // reused cache forgets views the workspace read earlier in the frame
                tab.view.read(cx);
                tab.view
                    .clone()
                    .cached(StyleRefinement::default().size_full())
            }))
            // a hidden tab bar has no tab to flash, the pane edges stay out of the way of text
            .when_some(flash.filter(|_| !show_bar), |terminal, flash| {
                terminal.child(div().absolute().inset_0().border_2().border_color(flash))
            })
            .when_some(zone, |terminal, side| {
                terminal.child(drop_overlay(side, theme))
            })
            // joined panes say so loudly, typing here reaches the others too
            .when(self.broadcasts(pane_id), |terminal| {
                terminal.child(
                    div()
                        .debug_selector(move || format!("pane-broadcast-{pane_id}"))
                        .absolute()
                        .inset_0()
                        .border_2()
                        .border_color(theme.danger_button)
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .right_0()
                                .px_2()
                                .rounded_bl_md()
                                .bg(theme.danger_button)
                                .text_color(theme.danger_button_text)
                                .text_sm()
                                .child("⇶ broadcast"),
                        ),
                )
            })
            .children(self.render_queue_list(pane, cx))
            .children(self.render_pty_banner(pane, cx))
            .children(self.render_profile_frame(pane, cx))
            .when(self.scroll_synced(pane_id), |terminal| {
                terminal.child(
                    div()
                        .debug_selector(move || format!("pane-scroll-sync-{pane_id}"))
                        .absolute()
                        .bottom_0()
                        .right_0()
                        .px_2()
                        .rounded_tl_md()
                        .bg(theme.tab_active_background)
                        .text_color(theme.text_muted)
                        .text_sm()
                        .child("⇅ synced"),
                )
            })
            .when(dim > 0., |terminal| {
                // deferred, since the cached terminal view would otherwise paint over it
                terminal.child(
                    div()
                        .debug_selector(move || format!("pane-dim-{pane_id}"))
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .bg(gpui::black().opacity(dim)),
                )
            });
        div()
            .id(("pane", pane_id))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.focus_pane(pane_id, window, cx);
                }),
            )
            .when(settings.focus_follows_mouse, |pane| {
                pane.on_mouse_move(
                    cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                        if event.pressed_button.is_none() && !cx.has_active_drag() {
                            this.hover_pane(pane_id, window, cx);
                        }
                    }),
                )
            })
            .when(show_bar, |pane| pane.child(bar))
            .child(terminal)
            .into_any_element()
    }

    /// one split: two children side by side or stacked, with a draggable divider
    fn render_split(
        &self,
        axis: SplitAxis,
        ratio: f32,
        first: &LayoutNode,
        second: &LayoutNode,
        split_path: &[bool],
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut first_path = split_path.to_vec();
        first_path.push(false);
        let mut second_path = split_path.to_vec();
        second_path.push(true);
        let theme = Theme::get(cx);
        let width = Settings::get(cx).panes.divider_width;
        let grab = width.max(DIVIDER_GRAB);
        // centered on the divider, so it reaches into both panes
        let handle = div()
            .id(format!("divider-{split_path:?}"))
            .debug_selector(|| "divider".into())
            .absolute()
            .map(|handle| match axis {
                SplitAxis::Horizontal => handle
                    .top_0()
                    .h_full()
                    .w(px(grab))
                    .left(px((width - grab) / 2.))
                    .cursor(CursorStyle::ResizeLeftRight),
                SplitAxis::Vertical => handle
                    .left_0()
                    .w_full()
                    .h(px(grab))
                    .top(px((width - grab) / 2.))
                    .cursor(CursorStyle::ResizeUpDown),
            })
            .on_drag(SplitDrag, |_, _, _, cx| cx.new(|_| SplitDrag));
        let divider = div()
            .flex_none()
            .relative()
            .bg(theme.border)
            .map(|divider| match axis {
                SplitAxis::Horizontal => divider.w(px(width)).h_full(),
                SplitAxis::Vertical => divider.h(px(width)).w_full(),
            })
            .child(handle);
        let path = split_path.to_vec();
        div()
            .flex()
            .map(|split| match axis {
                SplitAxis::Horizontal => split.flex_row(),
                SplitAxis::Vertical => split.flex_col(),
            })
            .size_full()
            .min_w_0()
            .min_h_0()
            // fires for the whole split while its divider is dragged
            .on_drag_move::<SplitDrag>(cx.listener(
                move |this, event: &DragMoveEvent<SplitDrag>, _, cx| {
                    let bounds = event.bounds;
                    let position = event.event.position;
                    let ratio = match axis {
                        SplitAxis::Horizontal => (position.x - bounds.left()) / bounds.size.width,
                        SplitAxis::Vertical => (position.y - bounds.top()) / bounds.size.height,
                    };
                    this.resize_split(&path, ratio, cx);
                },
            ))
            .child(
                div()
                    .flex_grow(ratio)
                    .flex_shrink(1.)
                    .flex_basis(relative(0.))
                    .min_w_0()
                    .min_h_0()
                    .child(self.render_layout(first, &first_path, cx)),
            )
            .child(divider)
            .child(
                div()
                    .flex_grow(1. - ratio)
                    .flex_shrink(1.)
                    .flex_basis(relative(0.))
                    .min_w_0()
                    .min_h_0()
                    .child(self.render_layout(second, &second_path, cx)),
            )
            .into_any_element()
    }

    /// draw the pane tree, `path` points at `node` so a split can be found again when resized
    fn render_layout(&self, node: &LayoutNode, path: &[bool], cx: &Context<Self>) -> AnyElement {
        match node {
            LayoutNode::Leaf(pane) => self.render_pane(pane, cx),
            LayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => self.render_split(*axis, *ratio, first, second, path, cx),
            LayoutNode::Empty => div().into_any_element(),
        }
    }

    /// profile list opened with a right click on "+", picking one opens a tab with it
    /// bar under the panes with the status pieces of the active tab on both sides
    fn render_status_bar(&self, cx: &Context<Self>) -> Stateful<Div> {
        let theme = Theme::get(cx);
        let side = |name: &'static str, pieces: &[Piece]| {
            let items = pieces
                .iter()
                .enumerate()
                // a space between two ports is a gap here
                .filter(|(_, piece)| !piece.text.trim().is_empty())
                .map(|(ix, piece)| {
                    let item = div().id((name, ix)).flex_none().whitespace_nowrap();
                    let item = match &piece.graph {
                        Some(graph) => item.child(graph_bars(graph, theme)),
                        None => item.child(tab_title::spin(&piece.text, self.spinner_frame)),
                    };
                    match piece.click {
                        Some(click) => item
                            .debug_selector(move || format!("status-{click:?}"))
                            .cursor_pointer()
                            .hover(|item| item.underline())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.status_clicked(click, window, cx)
                            })),
                        None => item,
                    }
                });
            div()
                .flex()
                .items_center()
                .gap_3()
                .min_w_0()
                .overflow_hidden()
                .children(items)
        };
        div()
            .id("status-bar")
            .debug_selector(|| "status-bar".into())
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap_6()
            .w_full()
            .h(rems(STATUS_BAR_HEIGHT))
            .px_3()
            .bg(theme.tab_bar_background)
            .border_t_1()
            .border_color(theme.border)
            .text_size(rems(STATUS_TEXT_SIZE))
            .text_color(theme.text_muted)
            .hover(|bar| bar.text_color(theme.text))
            .child(side("status-left", &self.status[0]))
            .child(side("status-right", &self.status[1]))
    }

    fn render_profile_menu(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let (pane, position) = self.profile_menu?;
        let theme = Theme::get(cx);
        let items = Settings::get(cx)
            .profiles
            .iter()
            .enumerate()
            .map(|(ix, profile)| {
                menu_item(("profile", ix), &profile.name, theme)
                    .debug_selector(move || format!("profile-{ix}"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.profile_menu = None;
                        let profile = Settings::get(cx).profiles[ix].clone();
                        this.add_profile_tab(pane, &profile, true, window, cx);
                    }))
            });
        let menu = popup_menu("profile-menu", theme)
            .children(items)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.profile_menu = None;
                cx.notify();
            }));
        Some(
            deferred(anchored().position(position).snap_to_window().child(menu)).into_any_element(),
        )
    }

    /// command palette, about page, close tab or paste dialog, centered near the top like in zed
    fn render_overlay(&self, cx: &App) -> Option<AnyElement> {
        // the tab overview covers the whole window
        if let Some(overview) = &self.overview {
            return Some(deferred(overview.clone()).into_any_element());
        }
        if let Some(topology) = &self.topology {
            return Some(deferred(topology.clone()).into_any_element());
        }
        let view = match (
            &self.palette,
            &self.about,
            &self.confirm_close,
            &self.confirm_paste,
            &self.command_input,
        ) {
            (Some(palette), _, _, _, _) => palette.clone().into_any_element(),
            (_, Some(about), _, _, _) => about.clone().into_any_element(),
            (_, _, Some(confirm), _, _) => confirm.clone().into_any_element(),
            (_, _, _, Some(paste), _) => paste.clone().into_any_element(),
            (_, _, _, _, Some(input)) => input.clone().into_any_element(),
            _ => return None,
        };
        Some(
            deferred(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    // a gap below the bars, the menu bar and a tab bar together would
                    // otherwise end right where it starts
                    .pt(rems((self.top_bars_height(cx) + 2.).max(4.)))
                    .child(view),
            )
            .into_any_element(),
        )
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // size blocks follow resizes right away, not on the next timed refresh
        let size = self.active_size(cx);
        if size != self.titled_size {
            self.titled_size = size;
            self.refresh_titles();
        }
        let settings = Settings::get(cx);
        let theme = Theme::get(cx);
        // ui scales in rems of the ui font size
        window.set_rem_size(px(settings.ui_font_size));
        let layout = match self.zoomed.and_then(|id| self.pane(id)) {
            Some(pane) => self.render_pane(pane, cx),
            None => self.render_layout(&self.layout, &[], cx),
        };
        // a terminal view only animates its own flash, and not at all in a background tab
        if settings.bell.visual.target == BellTarget::Tab
            && self
                .layout
                .panes()
                .into_iter()
                .flat_map(|pane| &pane.tabs)
                .any(|tab| tab.view.read(cx).bell_color(cx).is_some())
        {
            window.request_animation_frame();
        }
        // its own row above the panes, each pane has its own tab bar but the menus belong to
        // the whole window
        let menu_bar = settings.window.menu_bar.shown();
        let workspace = div()
            .key_context("Workspace")
            // remembered while dragging, the drop outside the window has no drag value to read
            .on_drag_move::<DraggedTab>(cx.listener(
                |this, event: &DragMoveEvent<DraggedTab>, _, cx| {
                    let dragged = event.drag(cx);
                    this.tab_drag = Some((dragged.pane, dragged.ix));
                },
            ))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    let dragged = this.tab_drag.take();
                    let outside = !Bounds::new(Point::default(), window.viewport_size())
                        .contains(&event.position);
                    if let Some((pane, ix)) = dragged
                        && outside
                        && cx.has_active_drag()
                    {
                        this.tab_dropped_outside(pane, ix, event.position, window, cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.tab_drag = None),
            )
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::activate_tab_action))
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::toggle_fullscreen))
            .on_action(cx.listener(Self::open_about))
            .on_action(cx.listener(Self::quit_action))
            .on_action(cx.listener(Self::minimize))
            .on_action(cx.listener(Self::zoom))
            .on_action(cx.listener(Self::run_action))
            .on_action(cx.listener(Self::run_named_command))
            .when(menu_bar, |workspace| {
                workspace
                    .capture_key_down(cx.listener(|this, _, _, cx| {
                        this.menu_bar.update(cx, |bar, _| bar.key_pressed());
                    }))
                    .on_modifiers_changed(cx.listener(
                        |this, event: &ModifiersChangedEvent, window, cx| {
                            this.menu_bar.update(cx, |bar, cx| {
                                bar.modifiers_changed(&event.modifiers, window, cx)
                            });
                        },
                    ))
            })
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.terminal_background.opacity(settings.window.opacity))
            .font_family(settings.ui_font_family.clone())
            .text_size(rems(TEXT_SIZE))
            .when(self.shows_top_row(cx), |workspace| {
                workspace.child(self.render_menu_row(cx))
            })
            .child(layout)
            .when(settings.status_bar.enable, |workspace| {
                workspace.child(self.render_status_bar(cx))
            })
            .children(self.render_profile_menu(cx))
            .children(self.render_tab_menu(cx))
            .children(self.render_group_menu(cx))
            .children(self.render_notifications(cx))
            .children(self.render_overlay(cx))
            .child(canvas(
                |_, _, _| {},
                |_, _, window, _| track_file_drags(window),
            ));
        client_frame(workspace, window, cx)
    }
}

// gpui leaves the window in keyboard mode when a file is dragged in after typing, so nothing
// counts as hovered and the drop is ignored. a mouse move puts it back in mouse mode
fn track_file_drags(window: &mut Window) {
    window.on_mouse_event(|event: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture
            || !window.last_input_was_keyboard()
            || !cx.has_active_drag()
        {
            return;
        }
        let event = event.clone();
        // events can't be dispatched while one is being handled
        window.defer(cx, move |window, cx| {
            window.dispatch_event(PlatformInput::MouseMove(event), cx);
        });
    });
}

#[cfg(test)]
mod tests {
    use gpui::{
        Bounds, Pixels, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, point,
        size,
    };

    use super::*;

    /// tab row alone, so layout can be checked without spawning shells
    struct TabRow {
        titles: Vec<String>,
        icon: String,
        icon_at: Vec<usize>,
        settings: Settings,
        scroll: ScrollHandle,
    }

    impl Render for TabRow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let tabs =
                self.titles.iter().enumerate().map(|(ix, title)| {
                    tab(ix, title, &self.icon, &self.icon_at, &[], &self.settings)
                });
            div().size_full().flex().text_size(rems(TEXT_SIZE)).child(
                div()
                    .flex()
                    .h(px(30.))
                    .w(px(600.))
                    .child(tab_row(&self.scroll).children(tabs)),
            )
        }
    }

    /// lay out `count` tabs in a 600px wide row with settings overridden by `json`
    fn layout<'a>(
        json: &str,
        count: usize,
        cx: &'a mut TestAppContext,
    ) -> (gpui::Entity<TabRow>, &'a mut VisualTestContext) {
        let settings = Settings::parse(json).unwrap();
        let (row, cx) = cx.add_window_view(|_, _| TabRow {
            titles: (1..=count).map(|n| format!("tab {n}")).collect(),
            icon: "I".into(),
            icon_at: vec![0],
            settings,
            scroll: ScrollHandle::new(),
        });
        cx.simulate_resize(size(px(1000.), px(600.)));
        cx.run_until_parked();
        (row, cx)
    }

    fn tab_bounds(row: &gpui::Entity<TabRow>, cx: &mut VisualTestContext) -> Vec<Bounds<Pixels>> {
        row.update(cx, |row, _| {
            (0..row.titles.len())
                .map(|ix| row.scroll.bounds_for_item(ix).unwrap())
                .collect()
        })
    }

    fn assert_close(a: Pixels, b: Pixels) {
        // layout snaps to pixels, so equal shares may differ by a fraction
        assert!((a - b).abs() <= px(1.), "{a:?} != {b:?}");
    }

    #[gpui::test]
    fn expanded_tabs_share_the_row_equally(cx: &mut TestAppContext) {
        for count in [1, 2, 3, 7] {
            let (row, cx) = layout(r#"{"expand_tabs": true}"#, count, cx);
            let tabs = tab_bounds(&row, cx);
            for tab in &tabs {
                assert_close(tab.size.width, px(600. / count as f32));
            }
            assert_close(tabs.last().unwrap().right(), tabs[0].left() + px(600.));
        }
    }

    #[gpui::test]
    fn fixed_tabs_use_tab_width(cx: &mut TestAppContext) {
        let (row, cx) = layout(r#"{"expand_tabs": false, "tab_width": 90}"#, 3, cx);
        row.update(cx, |row, cx| {
            row.titles[1] = "a much much much much longer title than the others".into();
            cx.notify();
        });
        cx.run_until_parked();
        for tab in tab_bounds(&row, cx) {
            assert_eq!(tab.size.width, px(90.));
        }
    }

    #[gpui::test]
    fn expanded_tabs_ignore_tab_width(cx: &mut TestAppContext) {
        let (row, cx) = layout(r#"{"expand_tabs": true, "tab_width": 90}"#, 3, cx);
        assert_close(tab_bounds(&row, cx)[0].size.width, px(200.));
    }

    #[gpui::test]
    fn expanded_tabs_ignore_title_length(cx: &mut TestAppContext) {
        let (row, cx) = layout(r#"{"expand_tabs": true}"#, 2, cx);
        row.update(cx, |row, cx| {
            row.titles[0] = "a much much much much longer title than the other one".into();
            cx.notify();
        });
        cx.run_until_parked();
        let tabs = tab_bounds(&row, cx);
        assert_close(tabs[0].size.width, tabs[1].size.width);
    }

    /// wheel down over the row, returns the row's horizontal scroll offset
    fn scroll_down(row: &gpui::Entity<TabRow>, cx: &mut VisualTestContext) -> Pixels {
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(300.), px(15.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-100.))),
            ..Default::default()
        });
        cx.run_until_parked();
        row.update(cx, |row, _| row.scroll.offset().x)
    }

    #[gpui::test]
    fn wheel_scrolls_overflowing_tabs(cx: &mut TestAppContext) {
        // 10 tabs of 90px overflow the 600px row
        let (row, cx) = layout(r#"{"expand_tabs": false, "tab_width": 90}"#, 10, cx);
        let tabs = tab_bounds(&row, cx);
        assert!(
            tabs.last().unwrap().right() > px(600.),
            "tabs should overflow: {tabs:?}"
        );
        assert_eq!(scroll_down(&row, cx), px(-100.));
        // scrolling stops at the last tab
        for _ in 0..100 {
            scroll_down(&row, cx);
        }
        let max = row.update(cx, |row, _| row.scroll.max_offset().x);
        assert_eq!(scroll_down(&row, cx), -max);
    }

    #[gpui::test]
    fn wheel_does_nothing_for_expanded_tabs(cx: &mut TestAppContext) {
        let (row, cx) = layout(r#"{"expand_tabs": true}"#, 7, cx);
        assert_eq!(scroll_down(&row, cx), px(0.));
        let tabs = tab_bounds(&row, cx);
        assert_close(tabs.last().unwrap().right(), px(600.));
    }

    #[gpui::test]
    fn too_many_expanded_tabs_can_still_be_reached(cx: &mut TestAppContext) {
        let (row, cx) = layout(r#"{"expand_tabs": true}"#, 100, cx);
        assert!(tab_bounds(&row, cx).last().unwrap().right() > px(600.));
        assert_eq!(scroll_down(&row, cx), px(-100.));
    }

    fn title_bounds(cx: &mut VisualTestContext) -> Bounds<Pixels> {
        cx.debug_bounds("tab-title-0").unwrap()
    }

    fn text_bounds(cx: &mut VisualTestContext) -> Bounds<Pixels> {
        cx.debug_bounds("tab-text-0").unwrap()
    }

    fn align_json(align: &str) -> String {
        format!(r#"{{"expand_tabs": false, "tab_width": 200, "tab_title_align": "{align}"}}"#)
    }

    fn icon_bounds(cx: &mut VisualTestContext) -> Bounds<Pixels> {
        cx.debug_bounds("tab-icon-0").unwrap()
    }

    #[gpui::test]
    fn title_follows_align(cx: &mut TestAppContext) {
        // px_3 padding puts the icon and title 12px inside each tab edge, gap_1 is 4px between them
        let (row, cx) = layout(&align_json("left"), 1, cx);
        let tab = tab_bounds(&row, cx)[0];
        let text = text_bounds(cx);
        assert!(text.size.width < px(160.));
        assert_close(icon_bounds(cx).left(), tab.left() + px(12.));
        assert_close(text.left(), icon_bounds(cx).right() + px(4.));

        let (_, cx) = layout(&align_json("right"), 1, cx);
        assert_close(text_bounds(cx).right(), tab.right() - px(12.));

        // the icon moves with the text it sits next to
        let (_, cx) = layout(&align_json("center"), 1, cx);
        assert_close(title_bounds(cx).center().x, tab.center().x);
        assert_close(text_bounds(cx).left(), icon_bounds(cx).right() + px(4.));
    }

    #[gpui::test]
    fn long_title_is_cut_for_every_align(cx: &mut TestAppContext) {
        let long = "a much much much much much longer title than the tab";
        for icon_first in [true, false] {
            for align in ["left", "center", "right"] {
                let (row, cx) = layout(&align_json(align), 1, cx);
                row.update(cx, |row, cx| {
                    row.titles[0] = long.into();
                    row.icon_at = vec![if icon_first { 0 } else { long.len() }];
                    cx.notify();
                });
                cx.run_until_parked();
                let tab = tab_bounds(&row, cx)[0];
                let text = text_bounds(cx);
                let icon = icon_bounds(cx);
                // the icon keeps its place and width, the title is cut next to it
                assert!(icon.size.width > px(0.));
                if icon_first {
                    assert_close(icon.left(), tab.left() + px(12.));
                    assert_close(text.left(), icon.right() + px(4.));
                    assert_close(text.right(), tab.right() - px(12.));
                } else {
                    assert_close(text.left(), tab.left() + px(12.));
                    assert_close(text.right(), icon.left() - px(4.));
                    assert_close(icon.right(), tab.right() - px(12.));
                }
            }
        }
    }

    #[gpui::test]
    fn icon_is_drawn_where_its_block_is(cx: &mut TestAppContext) {
        let (row, cx) = layout(&align_json("left"), 1, cx);
        let tab = tab_bounds(&row, cx)[0];
        let (icon, text) = (icon_bounds(cx), text_bounds(cx));
        assert_close(icon.left(), tab.left() + px(12.));
        assert!(icon.right() <= text.left());

        row.update(cx, |row, cx| {
            row.icon_at = vec!["tab 1".len()];
            cx.notify();
        });
        cx.run_until_parked();
        let (icon, text) = (icon_bounds(cx), text_bounds(cx));
        assert_close(text.left(), tab.left() + px(12.));
        assert_close(icon.left(), text.right() + px(4.));

        // no icon block, no icon
        row.update(cx, |row, cx| {
            row.icon_at = Vec::new();
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("tab-icon-0").is_none());
        assert_close(text_bounds(cx).left(), tab.left() + px(12.));
    }

    #[test]
    fn icon_drop_puts_glyph_bottom_on_baseline() {
        // letters already stand on the baseline
        assert!(icon_drop("x").abs() < 0.01);
        // nerd icons float above it
        assert!(icon_drop("\u{e795}") > 0.05);
        assert!(icon_drop("\u{f06a9}") > 0.);
        // the lowest glyph decides, so a mixed icon keeps the letter on the baseline
        assert_eq!(icon_drop("x\u{e795}"), icon_drop("x"));
        // letters with descenders move up
        assert!(icon_drop("g") < 0.);
        // nothing to measure
        assert_eq!(icon_drop(""), 0.);
        assert_eq!(icon_drop("\u{10fffd}"), 0.);
    }

    #[gpui::test]
    fn icon_is_moved_down_to_the_baseline(cx: &mut TestAppContext) {
        let (row, cx) = layout(r#"{"expand_tabs": false, "tab_width": 200}"#, 1, cx);
        let letter_top = icon_bounds(cx).top();
        assert_close(letter_top, text_bounds(cx).top());
        row.update(cx, |row, cx| {
            row.icon = "\u{e795}".into();
            cx.notify();
        });
        cx.run_until_parked();
        // rems are 16px in the test window
        let drop = px(icon_drop("\u{e795}") * TEXT_SIZE * 16.);
        assert!(drop > px(0.));
        assert_close(icon_bounds(cx).top(), text_bounds(cx).top() + drop);
    }
}
