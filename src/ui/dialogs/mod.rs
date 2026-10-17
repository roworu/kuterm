//! panels that float over the terminal: the command palette and the prompts and dialogs it
//! and the workspace open

mod about;
mod command_palette;
mod confirm_close;
mod confirm_paste;
mod input_prompt;

use gpui::{KeyDownEvent, div, prelude::*, rems};

use crate::theme::Theme;

pub use about::About;
pub use command_palette::{CommandPalette, FinderQuery, Highlighted};
pub use confirm_close::{CloseConfirmed, CloseTarget, ConfirmClose};
pub use confirm_paste::{ConfirmPaste, PasteConfirmed};
pub use input_prompt::{InputFilled, InputPrompt};

/// floating box near the top of the window, shared by the palette and the about page
fn panel(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .w(rems(34.))
        .max_w_full()
        .bg(theme.tab_bar_background)
        .border_1()
        .border_color(theme.border)
        .rounded_md()
        .shadow_lg()
        .text_color(theme.text)
        .overflow_hidden()
}

/// answer a key gives to a yes or no dialog, true for yes. arrows move `yes_selected`, the
/// button enter presses
fn dialog_answer(event: &KeyDownEvent, yes_selected: &mut bool) -> Option<bool> {
    let modifiers = &event.keystroke.modifiers;
    // answers need a bare key, so a shell shortcut like ctrl+y typed by habit answers nothing.
    // shift is still allowed on the letters
    let plain = !modifiers.modified();
    let letter = !(modifiers.control || modifiers.alt || modifiers.platform || modifiers.function);
    match event.keystroke.key.as_str() {
        "enter" if plain => Some(*yes_selected),
        "y" if letter => Some(true),
        "n" if letter => Some(false),
        "escape" if plain => Some(false),
        // "cancel" sits left of the yes button
        "left" | "up" => {
            *yes_selected = false;
            None
        }
        "right" | "down" => {
            *yes_selected = true;
            None
        }
        _ => None,
    }
}
