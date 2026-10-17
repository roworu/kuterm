//! drawing the workspace: tab bar on top, active terminal below
// TODO: need a setting on where to put tabs (top, bottom, left?, right?)

use std::{cell::RefCell, collections::HashMap, ops::Range, sync::LazyLock};

use gpui::{
    AnyElement, App, ClickEvent, Context, Div, DragMoveEvent, Entity, Focusable, KeyDownEvent,
    MouseButton, MouseDownEvent, ScrollHandle, Stateful, Window, anchored, deferred, div,
    prelude::*, px, rems,
};
use skrifa::{
    FontRef, MetadataProvider,
    instance::{LocationRef, Size},
};

use super::{
    Workspace,
    end_truncated::EndTruncated,
    groups::{TabDrop, group_line},
    tab_title::{self},
};
use crate::{
    settings::{BellTarget, Settings, TabTitleAlign},
    theme::Theme,
    ui::{terminal_view::open_link, text_input::TextInput},
};

// tab bar text size in rems, the icon offset is scaled by it
pub(super) const TEXT_SIZE: f32 = 0.875;

/// status bar height in rems
pub(super) const STATUS_BAR_HEIGHT: f32 = 1.5;
// nerd font bell, shown on a background tab that rang
pub(super) const BELL_ICON: &str = "\u{f0f3}";

/// how far `icon` must move down, in ems, for its lowest point to stand on the baseline
pub(super) fn icon_drop(icon: &str) -> f32 {
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

pub(super) fn measure_icon_drop(icon: &str) -> f32 {
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

/// focused input holding `text`, for a tab or group name
pub(super) fn rename_input(
    placeholder: &'static str,
    text: String,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<TextInput> {
    let input = cx.new(|cx| {
        let mut input = TextInput::new(placeholder, cx);
        input.set_content(text, cx);
        input
    });
    input.focus_handle(cx).focus(window, cx);
    input
}

/// field a tab or group name is typed into: enter keeps it, escape drops it, a click
/// elsewhere keeps it. `finish` gets whether to keep it
pub(super) fn rename_field(
    input: Entity<TextInput>,
    finish: fn(&mut Workspace, bool, &mut Window, &mut Context<Workspace>),
    cx: &Context<Workspace>,
) -> Div {
    div()
        .min_w(rems(6.))
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            let keep = match event.keystroke.key.as_str() {
                "enter" => true,
                "escape" => false,
                _ => return,
            };
            finish(this, keep, window, cx);
            cx.stop_propagation();
        }))
        // clicks in the field move its caret, they don't pick or drag the tab
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down_out(cx.listener(move |this, _, window, cx| finish(this, true, window, cx)))
        .child(input)
}

/// row holding the tabs, fills the bar next to the new tab button
pub(super) fn tab_row(scroll: &ScrollHandle) -> Stateful<Div> {
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
pub(super) fn title_with_icons(
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
pub(super) fn tab(
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
pub(super) fn tab_with(
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

    pub(super) fn render_tab(&self, pane: usize, ix: usize, cx: &Context<Self>) -> Stateful<Div> {
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
            Some(input) => rename_field(input, Workspace::finish_rename, cx)
                .id(("rename-tab", ix))
                .debug_selector(move || format!("rename-tab-{ix}"))
                .flex_1()
                .on_click(|_, _, cx| cx.stop_propagation())
                .into_any_element(),
            None => title_with_icons(ix, &title, &icon, &icon_at, &ports_at).into_any_element(),
        };
        let drag = settings.drag_tabs && self.renaming.is_none();
        let group_color = self.group_color_of(pane, ix, cx);
        let pane_tabs = &self.pane(pane).expect("pane from caller").tabs;
        let last_of_group = pane_tabs.get(ix + 1).map(|next| next.group) != Some(tab_state.group);
        // grouping with this tab makes a new group when it has none
        let onto_color = group_color.unwrap_or_else(|| self.next_group_color(cx));
        // where a tab dragged over this one would land, stale once the drag is gone
        let hovered_drop = self
            .tab_drop
            .filter(|&(over_pane, over_ix, _)| over_pane == pane && over_ix == ix)
            .filter(|_| cx.has_active_drag())
            .map(|(_, _, zone)| zone);
        tab_with(ix, content, pinned, settings)
            .relative()
            // a line in the group color under the tabs of a group, over the border between
            // them so it runs on unbroken from the chip, rounded at the group's end
            .when_some(group_color, |tab, color| {
                tab.child(
                    group_line(color)
                        .right(px(-1.))
                        .when(last_of_group, |line| line.right_0().rounded_r_sm()),
                )
            })
            .group("tab")
            .border_r_1()
            .border_color(theme.border)
            .map(|tab| match hovered_drop {
                Some(TabDrop::Before) => tab.border_l_2().border_color(indicator),
                Some(TabDrop::After) => tab.border_r_2().border_color(indicator),
                // the two would be grouped: the tab lights up in the color of its group, or
                // the color a new group gets
                Some(TabDrop::Onto) => tab.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded_md()
                        .border_2()
                        .border_color(onto_color)
                        .bg(onto_color.opacity(0.15)),
                ),
                None => tab,
            })
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
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.tab_menu = Some((id, event.position));
                    this.tab_menu_focus.focus(window, cx);
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
                .on_drag_move::<DraggedTab>(cx.listener(
                    move |this, event: &DragMoveEvent<DraggedTab>, _, cx| {
                        let dragged = event.drag(cx);
                        let over = event.bounds.contains(&event.event.position)
                            && (dragged.pane, dragged.ix) != (pane, ix);
                        let zone = over.then(|| {
                            let x = event.event.position.x - event.bounds.origin.x;
                            (
                                pane,
                                ix,
                                TabDrop::at(f32::from(x), f32::from(event.bounds.size.width)),
                            )
                        });
                        let mine = this.tab_drop.is_some_and(|(over_pane, over_ix, _)| {
                            (over_pane, over_ix) == (pane, ix)
                        });
                        // every tab hears every move, each only sets or clears its own
                        if (zone.is_some() || mine) && this.tab_drop != zone {
                            this.tab_drop = zone;
                            cx.notify();
                        }
                    },
                ))
                .on_drop(cx.listener(
                    move |this, dragged: &DraggedTab, window, cx| {
                        let zone = match this.tab_drop.take() {
                            Some((over_pane, over_ix, zone))
                                if (over_pane, over_ix) == (pane, ix) =>
                            {
                                zone
                            }
                            // no move seen over the tab: land on the side it came from
                            _ if dragged.pane == pane && dragged.ix < ix => TabDrop::After,
                            _ => TabDrop::Before,
                        };
                        this.drop_tab_on_tab(
                            (dragged.pane, dragged.ix),
                            (pane, ix),
                            zone,
                            window,
                            cx,
                        );
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
    pub(super) fn render_new_tab_button(&self, pane: usize, cx: &Context<Self>) -> Stateful<Div> {
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

    pub(super) fn render_profile_menu(&self, cx: &Context<Self>) -> Option<AnyElement> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::test_support::*;
    use gpui::{
        Bounds, Pixels, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, point,
        size,
    };

    /// tab row alone, so layout can be checked without spawning shells
    struct TabRow {
        titles: Vec<String>,
        icon: String,
        icon_at: Vec<usize>,
        ports_at: Vec<(Range<usize>, u16)>,
        settings: Settings,
        scroll: ScrollHandle,
    }

    impl Render for TabRow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let tabs = self.titles.iter().enumerate().map(|(ix, title)| {
                tab(
                    ix,
                    title,
                    &self.icon,
                    &self.icon_at,
                    &self.ports_at,
                    &self.settings,
                )
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
            ports_at: Vec::new(),
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

    #[gpui::test]
    fn tab_icon_follows_running_program(cx: &mut TestAppContext) {
        let (before, during) =
            icon_while_sleeping(cx, r#"{"icons": {"dynamic": true, "default": "D"}}"#);
        assert_eq!(before, "D");
        assert_eq!(during, "S");
    }

    #[gpui::test]
    fn disabled_dynamic_icon_stays_default(cx: &mut TestAppContext) {
        let (before, during) =
            icon_while_sleeping(cx, r#"{"icons": {"dynamic": false, "default": "D"}}"#);
        assert_eq!(before, "D");
        assert_eq!(during, "D");
    }

    #[gpui::test]
    fn icon_sits_where_its_block_is(cx: &mut TestAppContext) {
        let settings =
            Settings::parse(r#"{"tab_title": [{"text": "a"}, "icon", {"text": "bc"}]}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        let tab = |cx: &mut VisualTestContext| {
            ws.update(cx, |ws, _| {
                let tab = &ws.focused_pane().tabs[0];
                (tab.title.clone(), tab.icon_at.clone())
            })
        };
        wait_until(cx, "title never built", |cx| tab(cx).0 == "abc");
        assert_eq!(tab(cx).1, [1]);
    }

    #[gpui::test]
    fn profile_icon_never_changes(cx: &mut TestAppContext) {
        let (before, during) = icon_while_sleeping(
            cx,
            r#"{"icons": {"dynamic": true, "default": "D"},
                "profiles": [{"name": "p", "command": "system", "icon": "P"}]}"#,
        );
        assert_eq!(before, "P");
        assert_eq!(during, "P");
    }

    #[gpui::test]
    fn middle_click_closes_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        cx.run_until_parked();
        let old = views(&ws, cx);
        let title = cx.debug_bounds("tab-title-1").unwrap().center();
        cx.simulate_event(MouseDownEvent {
            position: title,
            modifiers: Modifiers::default(),
            button: MouseButton::Middle,
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        assert_eq!(views(&ws, cx), vec![old[0], old[2]]);
    }

    #[gpui::test]
    fn single_profile_has_no_menu(cx: &mut TestAppContext) {
        let (ws, cx) = open_profiles(cx, r#"{"name": "only", "command": "system"}"#);
        assert!(cx.debug_bounds("profile-hint").is_none());
        let plus = cx.debug_bounds("new-tab").unwrap().center();
        right_click(cx, plus);
        assert!(!menu_open(&ws, cx));
        assert!(cx.debug_bounds("profile-0").is_none());
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[gpui::test]
    fn right_click_on_plus_opens_picked_profile(cx: &mut TestAppContext) {
        let (ws, cx) = open_profiles(
            cx,
            r#"{"name": "main", "default": true, "command": "system"},
               {"name": "other", "command": {"with_arguments": {
                   "program": "/bin/sh", "args": ["-c", "echo \"from_$PICKED\"; sleep 5"]}},
                "env": {"PICKED": "other"}}"#,
        );
        assert!(cx.debug_bounds("profile-hint").is_some());
        let plus = cx.debug_bounds("new-tab").unwrap().center();

        // a click outside closes the menu without a new tab
        right_click(cx, plus);
        assert!(menu_open(&ws, cx));
        assert!(cx.debug_bounds("profile-1").is_some());
        let outside = point(gpui::px(450.), gpui::px(400.));
        click(cx, outside, 1);
        release(cx, outside);
        cx.run_until_parked();
        assert!(!menu_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 1);

        right_click(cx, plus);
        let item = cx.debug_bounds("profile-1").unwrap().center();
        click(cx, item, 1);
        release(cx, item);
        cx.run_until_parked();
        assert!(!menu_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 1);
        let terminal = terminal(&ws, cx, 1);
        wait_until(cx, "picked profile never ran", |cx| {
            terminal.read_with(cx, |t, _| line_starting_with(t, "from_other").is_some())
        });

        // a left click still opens the default profile, "+" moved after the new tab
        let plus = cx.debug_bounds("new-tab").unwrap().center();
        click(cx, plus, 1);
        release(cx, plus);
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 3);
    }

    #[gpui::test]
    fn close_button_follows_setting(cx: &mut TestAppContext) {
        for show in [true, false] {
            let json = format!(r#"{{"show_tab_close_button": {show}}}"#);
            let settings = Settings::parse(&json).unwrap();
            let (_, cx) = open_with_settings(cx, 2, "{}", settings);
            assert_eq!(cx.debug_bounds("close-tab-1").is_some(), show);
        }
    }

    #[gpui::test]
    fn port_after_plain_text_follows_that_text(cx: &mut TestAppContext) {
        let (row, cx) = layout(&align_json("left"), 1, cx);
        row.update(cx, |row, cx| {
            row.titles = vec!["web 3000".into()];
            row.icon_at = Vec::new();
            row.ports_at = vec![(4..8, 3000)];
            cx.notify();
        });
        cx.run_until_parked();
        let text = text_bounds(cx);
        let port = cx.debug_bounds("tab-port-3000").expect("port not drawn");
        assert!(text.right() <= port.left(), "{text:?} overlaps {port:?}");
        assert!(cx.debug_bounds("tab-icon-0").is_none());
    }
}
