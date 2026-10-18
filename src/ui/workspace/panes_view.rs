//! drawing the panes: each pane with its tab bar, terminal and the marks over it, splits with
//! their dividers, and the whole layout

use gpui::{
    AnyElement, Context, CursorStyle, DragMoveEvent, ExternalPaths, MouseButton, MouseMoveEvent,
    StyleRefinement, Window, div, prelude::*, px, relative, rems,
};

use super::{DropSide, LayoutNode, Pane, SplitAxis, Workspace, tab_bar::*, window::shows_tab_bar};
use crate::{
    settings::{BellTarget, Decorations, NewTabButton, Settings},
    theme::Theme,
};

/// divider handle, dragged to resize a split. the preview it draws is never seen
#[derive(Clone)]
pub(super) struct SplitDrag;

impl Render for SplitDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

// narrowest a divider is to grab, a thinner one is widened by an invisible handle
pub(super) const DIVIDER_GRAB: f32 = 6.;

/// translucent half pane showing where a dropped tab splits
pub(super) fn drop_overlay(side: DropSide, theme: &Theme) -> AnyElement {
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
    /// one pane: its tab row on top and its active terminal below
    pub(super) fn render_pane(&self, pane: &Pane, cx: &Context<Self>) -> AnyElement {
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
    pub(super) fn render_split(
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
    pub(super) fn render_layout(
        &self,
        node: &LayoutNode,
        path: &[bool],
        cx: &Context<Self>,
    ) -> AnyElement {
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
}

#[cfg(test)]
mod tests {
    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn dragging_the_divider_resizes_the_split(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        let right = split_pane(&ws, cx, DropSide::Right);
        let divider = cx.debug_bounds("divider").unwrap().center();
        drag_to(cx, divider, point(gpui::px(300.), divider.y));
        let ratio = ws.update(cx, |ws, _| match &ws.layout {
            LayoutNode::Split { ratio, .. } => *ratio,
            _ => panic!("expected a split"),
        });
        assert!((0.25..0.4).contains(&ratio), "{ratio}");
        let left = cx.debug_bounds("pane-terminal-0").unwrap();
        let right = cx
            .debug_bounds(&*format!("pane-terminal-{right}").leak())
            .unwrap();
        assert!(left.size.width < right.size.width, "{left:?} {right:?}");
    }

    #[gpui::test]
    fn files_dropped_after_typing_land_in_the_pane_under_them(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        let right = split_pane(&ws, cx, DropSide::Right);
        // typing leaves gpui in keyboard mode, which ignored drops before
        cx.simulate_keystrokes("x");
        cx.run_until_parked();
        let dir = std::env::temp_dir().join(format!("kuterm dropped {}", std::process::id()));
        let at = cx.debug_bounds("pane-terminal-0").unwrap().center();
        cx.simulate_event(gpui::FileDropEvent::Entered {
            position: at,
            paths: gpui::ExternalPaths(vec![dir.clone()].into()),
        });
        cx.simulate_event(gpui::FileDropEvent::Pending { position: at });
        cx.simulate_event(gpui::FileDropEvent::Submit { position: at });
        cx.run_until_parked();
        assert_ne!(ws.update(cx, |ws, _| ws.focused), right);
        let left = terminal(&ws, cx, 0);
        let quoted = format!("'{}'", dir.display());
        wait_until(cx, "the dropped path was never typed", |cx| {
            left.read_with(cx, |t, _| t.visible_screen_text(false).0.contains(&quoted))
        });
    }
}
