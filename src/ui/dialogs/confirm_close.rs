//! asks before closing tabs or quitting while something runs

use gpui::{
    App, Context, DismissEvent, EntityId, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    Window, div, prelude::*,
};

use crate::theme::Theme;

use super::{dialog_answer, panel};

/// emitted when closing the tab or quitting is confirmed
pub struct CloseConfirmed;

/// what the close dialog asks about
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CloseTarget {
    /// these tabs, closed together, like the tabs on one side of the active one
    Tabs(Vec<EntityId>),
    /// this window, quitting the app when it is the last one
    Window,
    /// every window, quitting the app
    Quit,
}

impl CloseTarget {
    /// true when closing the tab with this view is part of what is asked
    pub fn contains(&self, id: EntityId) -> bool {
        match self {
            Self::Tabs(tabs) => tabs.contains(&id),
            Self::Window | Self::Quit => false,
        }
    }
}

/// asks before closing a tab or quitting while programs still run
pub struct ConfirmClose {
    focus_handle: FocusHandle,
    message: String,
    pub target: CloseTarget,
    /// true while "close" is the button enter presses, false for "cancel"
    close_selected: bool,
}

impl EventEmitter<DismissEvent> for ConfirmClose {}
impl EventEmitter<CloseConfirmed> for ConfirmClose {}

impl ConfirmClose {
    /// dialog showing `message` about `target`, ready to be focused
    pub fn new(message: String, target: CloseTarget, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            message,
            target,
            close_selected: true,
        }
    }

    /// shown question, exposed for tests
    #[cfg(test)]
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match dialog_answer(event, &mut self.close_selected) {
            Some(true) => cx.emit(CloseConfirmed),
            Some(false) => cx.emit(DismissEvent),
            None => {}
        }
        cx.notify();
        // other keys must not reach the terminal behind the dialog
        cx.stop_propagation();
    }
}
impl Focusable for ConfirmClose {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConfirmClose {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        // the selected button is outlined in the text color, it is the one enter presses
        let button = |id: &'static str, label: &'static str, selected: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px_3()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(if selected { theme.text } else { theme.border })
                .child(label)
        };
        panel(theme)
            .id("confirm-close")
            .debug_selector(|| "confirm-close".into())
            .track_focus(&self.focus_handle)
            .key_context("ConfirmClose")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .p_4()
            .gap_2()
            .child(self.message.clone())
            .child(
                div()
                    .text_color(theme.text_muted)
                    .child(match &self.target {
                        CloseTarget::Tabs(_) => "close anyway?",
                        CloseTarget::Window | CloseTarget::Quit => "quit anyway?",
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("confirm-close-cancel", "cancel", !self.close_selected)
                            .when(!self.close_selected, |button| {
                                button.bg(theme.tab_active_background)
                            })
                            .hover(|button| button.bg(theme.tab_active_background))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    )
                    .child(
                        button(
                            "confirm-close-ok",
                            match &self.target {
                                CloseTarget::Tabs(_) => "close",
                                CloseTarget::Window | CloseTarget::Quit => "quit",
                            },
                            self.close_selected,
                        )
                        .bg(theme.danger_button)
                        .text_color(theme.danger_button_text)
                        .hover(|button| button.opacity(0.85))
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(CloseConfirmed))),
                    ),
            )
    }
}
