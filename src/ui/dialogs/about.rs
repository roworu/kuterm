//! page with the version and links

use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, Window, div, prelude::*, rems,
};

use crate::theme::Theme;

use super::panel;

/// page with the version and links, closed by any key or a click outside
pub struct About {
    focus_handle: FocusHandle,
}

impl EventEmitter<DismissEvent> for About {}

impl About {
    /// about page ready to be focused
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Focusable for About {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for About {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let line = |label: &'static str, value: &'static str| {
            div()
                .flex()
                .gap_2()
                .child(div().w(rems(6.)).text_color(theme.text_muted).child(label))
                .child(value)
        };
        panel(theme)
            .id("about")
            .debug_selector(|| "about".into())
            .track_focus(&self.focus_handle)
            .key_context("About")
            .on_key_down(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
                cx.emit(DismissEvent);
            }))
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .p_4()
            .gap_1()
            .child(div().text_size(rems(1.5)).child("kuterm"))
            .child(
                div()
                    .pb_2()
                    .text_color(theme.text_muted)
                    .child(env!("CARGO_PKG_DESCRIPTION")),
            )
            .child(line("version", env!("CARGO_PKG_VERSION")))
            .child(line("source", env!("CARGO_PKG_REPOSITORY")))
            .child(line("license", env!("CARGO_PKG_LICENSE")))
            .child(
                div()
                    .pt_2()
                    .text_color(theme.text_muted)
                    .child("press any key to close"),
            )
    }
}
