//! the root view of a window: panes, bars and whatever floats over them

use gpui::{
    AnyElement, App, Bounds, Context, DispatchPhase, DragMoveEvent, ModifiersChangedEvent,
    MouseButton, MouseMoveEvent, MouseUpEvent, PlatformInput, Point, Window, canvas, deferred, div,
    prelude::*, px, rems,
};

use super::{Workspace, tab_bar::*, window::client_frame};
use crate::settings::{BellTarget, Settings};

// gpui leaves the window in keyboard mode when a file is dragged in after typing, so nothing
// counts as hovered and the drop is ignored. a mouse move puts it back in mouse mode
pub(super) fn track_file_drags(window: &mut Window) {
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

impl Workspace {
    /// command palette, about page, close tab or paste dialog, centered near the top like in zed
    pub(super) fn render_overlay(&self, cx: &App) -> Option<AnyElement> {
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
