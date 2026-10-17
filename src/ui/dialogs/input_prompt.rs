//! asks for one value of a command

use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Window,
    div, prelude::*,
};

use crate::{theme::Theme, ui::text_input::TextInput};

use super::panel;

/// value typed for one input of a command
pub struct InputFilled(pub String);

/// asks for one value of a command, a text field with the input's hint
pub struct InputPrompt {
    /// input name, shown above the field
    name: String,
    hint: String,
    input: Entity<TextInput>,
}

impl EventEmitter<DismissEvent> for InputPrompt {}
impl EventEmitter<InputFilled> for InputPrompt {}

impl InputPrompt {
    /// prompt for the input `name`, showing `hint` while the field is empty
    pub fn new(name: &str, hint: &str, cx: &mut Context<Self>) -> Self {
        Self {
            name: name.to_string(),
            hint: hint.to_string(),
            input: cx.new(|cx| TextInput::new(hint, cx)),
        }
    }

    /// start with `text` in the field, like a value being edited
    pub fn set_text(&self, text: String, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_content(text, cx));
    }

    /// enter fills the input, escape cancels the whole command
    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => cx.emit(InputFilled(self.input.read(cx).content().to_string())),
            "escape" => cx.emit(DismissEvent),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Focusable for InputPrompt {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for InputPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        panel(theme)
            .id("command-input")
            .debug_selector(|| "command-input".into())
            .key_context("CommandInput")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .p_4()
            .gap_2()
            .child(div().text_color(theme.text_muted).child(self.name.clone()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(theme.border)
                    .child(div().flex_1().min_w_0().child(self.input.clone())),
            )
            .child(div().text_color(theme.text_muted).child(format!(
                "enter runs the command, escape cancels. {}",
                self.hint
            )))
    }
}
