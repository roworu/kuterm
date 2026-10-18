//! asks before pasting text that would run right away or has control characters

use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Window, div,
    prelude::*,
};

use crate::{terminal::paste_check, theme::Theme};

use super::{dialog_answer, panel};

// lines and chars per line the paste dialog shows of the text
const PASTE_PREVIEW_LINES: usize = 6;
const PASTE_PREVIEW_CHARS: usize = 80;

/// emitted when pasting is confirmed
pub struct PasteConfirmed;

/// asks before pasting text that would run right away or has control characters
pub struct ConfirmPaste {
    focus_handle: FocusHandle,
    /// first lines of the text, control characters made visible, hidden and lookalike
    /// characters as marked pieces
    preview: Vec<Vec<PreviewPiece>>,
    /// the text has hidden or lookalike characters
    flagged: bool,
    /// lines not in the preview
    more_lines: usize,
    /// true while "paste" is the button enter presses, false for "cancel"
    paste_selected: bool,
}

impl EventEmitter<DismissEvent> for ConfirmPaste {}
impl EventEmitter<PasteConfirmed> for ConfirmPaste {}

impl ConfirmPaste {
    /// dialog previewing `text`, ready to be focused
    pub fn new(text: &str, cx: &mut Context<Self>) -> Self {
        let (preview, more_lines) = paste_preview(text);
        Self {
            focus_handle: cx.focus_handle(),
            // the whole text, what is past the preview is pasted too
            flagged: !paste_check::flagged(text).is_empty(),
            preview,
            more_lines,
            // enter right after the paste key must not paste by habit
            paste_selected: false,
        }
    }

    /// shown lines, exposed for tests
    #[cfg(test)]
    pub(crate) fn preview(&self) -> Vec<String> {
        self.preview.iter().map(|line| preview_text(line)).collect()
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match dialog_answer(event, &mut self.paste_selected) {
            Some(true) => cx.emit(PasteConfirmed),
            Some(false) => cx.emit(DismissEvent),
            None => {}
        }
        cx.notify();
        cx.stop_propagation();
    }
}

/// part of a previewed line, marked ones are hidden or lookalike characters
#[derive(Clone, Debug, PartialEq)]
struct PreviewPiece {
    text: String,
    marked: bool,
}

#[cfg(test)]
fn preview_text(line: &[PreviewPiece]) -> String {
    line.iter().map(|piece| piece.text.as_str()).collect()
}

/// first lines of a paste with control characters in caret notation, like ^[ for escape,
/// hidden and lookalike characters as marked <U+200B>, and how many lines are left out
fn paste_preview(text: &str) -> (Vec<Vec<PreviewPiece>>, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let preview = lines
        .iter()
        .take(PASTE_PREVIEW_LINES)
        .map(|line| {
            let flagged = paste_check::flagged(line);
            let mut pieces: Vec<PreviewPiece> = Vec::new();
            let mut length = 0;
            for (at, c) in line.char_indices() {
                let marked = flagged.contains(&at);
                let shown = match c {
                    c if marked => format!("<U+{:04X}>", c as u32),
                    '\t' => "    ".into(),
                    c if (c as u32) < 0x20 => format!("^{}", char::from(c as u8 + b'@')),
                    '\x7f' => "^?".into(),
                    c if c.is_control() => format!("<{:02x}>", c as u32),
                    c => c.to_string(),
                };
                let count = shown.chars().count();
                if length + count > PASTE_PREVIEW_CHARS - 1 {
                    pieces.push(PreviewPiece {
                        text: "…".into(),
                        marked: false,
                    });
                    break;
                }
                length += count;
                match pieces.last_mut() {
                    Some(last) if !marked && !last.marked => last.text.push_str(&shown),
                    _ => pieces.push(PreviewPiece {
                        text: shown,
                        marked,
                    }),
                }
            }
            pieces
        })
        .collect();
    (preview, lines.len().saturating_sub(PASTE_PREVIEW_LINES))
}

impl Focusable for ConfirmPaste {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConfirmPaste {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
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
            .id("confirm-paste")
            .debug_selector(|| "confirm-paste".into())
            .track_focus(&self.focus_handle)
            .key_context("ConfirmPaste")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .p_4()
            .gap_2()
            .child("paste this text?")
            .child(div().text_color(theme.text_muted).child(if self.flagged {
                "it has characters that don't show or pass for others, marked below. the text \
                 may run something else than it seems to"
            } else {
                "it has new lines or control characters, the program gets them as typed keys"
            }))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(theme.terminal_background)
                    .text_color(theme.terminal_foreground)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .children(self.preview.iter().map(|line| {
                        div().flex().children(line.iter().map(|piece| {
                            div()
                                .when(piece.marked, |badge| {
                                    badge
                                        .debug_selector(|| "paste-badge".into())
                                        .px_0p5()
                                        .rounded_sm()
                                        .bg(theme.danger_button)
                                        .text_color(theme.danger_button_text)
                                })
                                .child(piece.text.clone())
                        }))
                    }))
                    .when(self.more_lines > 0, |preview| {
                        preview.child(
                            div()
                                .text_color(theme.text_muted)
                                .child(format!("… {} more lines", self.more_lines)),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("confirm-paste-cancel", "cancel", !self.paste_selected)
                            .when(!self.paste_selected, |button| {
                                button.bg(theme.tab_active_background)
                            })
                            .hover(|button| button.bg(theme.tab_active_background))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    )
                    .child(
                        button("confirm-paste-ok", "paste", self.paste_selected)
                            .bg(theme.danger_button)
                            .text_color(theme.danger_button_text)
                            .hover(|button| button.opacity(0.85))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PasteConfirmed))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paste_preview_marks_hidden_characters() {
        let (preview, _) = paste_preview("ls\u{202e}txt.exe");
        let marked: Vec<&str> = preview[0]
            .iter()
            .filter(|piece| piece.marked)
            .map(|piece| piece.text.as_str())
            .collect();
        assert_eq!(marked, ["<U+202E>"]);
        assert_eq!(preview_text(&preview[0]), "ls<U+202E>txt.exe");
    }

    #[test]
    fn paste_preview_shows_control_characters_and_cuts_long_text() {
        let text = "echo \x1b[31mred\t!\n".to_string() + &"x".repeat(200) + "\n3\n4\n5\n6\n7\n8";
        let (preview, more) = super::paste_preview(&text);
        let preview: Vec<String> = preview.iter().map(|line| preview_text(line)).collect();
        assert_eq!(preview[0], "echo ^[[31mred    !");
        assert_eq!(preview[1].chars().count(), super::PASTE_PREVIEW_CHARS);
        assert!(preview[1].ends_with('…'));
        assert_eq!(preview.len(), super::PASTE_PREVIEW_LINES);
        assert_eq!(more, 2);
    }

    #[test]
    fn paste_preview_shows_delete_in_caret_notation() {
        let (preview, more) = paste_preview("rm x\x7f\x7fy\x05");
        assert_eq!(preview_text(&preview[0]), "rm x^?^?y^E");
        assert_eq!(more, 0);
    }

    /// height of the dialog drawn for `text`, and what it emitted after typing `keys`
    fn drawn(text: &str, keys: &str, cx: &mut gpui::TestAppContext) -> (gpui::Pixels, Vec<bool>) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(|cx| cx.set_global(Theme::default()));
        // a host that doesn't stretch the dialog to the window, so it is as tall as its rows
        struct Host(gpui::Entity<ConfirmPaste>);
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().flex().items_start().child(self.0.clone())
            }
        }
        let text = text.to_string();
        let (host, cx) =
            cx.add_window_view(|_, cx| Host(cx.new(|cx| ConfirmPaste::new(&text, cx))));
        let dialog = host.read_with(cx, |host, _| host.0.clone());
        let answers = Rc::new(RefCell::new(Vec::new()));
        let (yes, no) = (answers.clone(), answers.clone());
        let _subscriptions = cx.update(|window, cx| {
            dialog.read(cx).focus_handle.clone().focus(window, cx);
            [
                cx.subscribe(&dialog, move |_, _: &PasteConfirmed, _| {
                    yes.borrow_mut().push(true)
                }),
                cx.subscribe(&dialog, move |_, _: &DismissEvent, _| {
                    no.borrow_mut().push(false)
                }),
            ]
        });
        cx.run_until_parked();
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
        let height = cx
            .debug_bounds("confirm-paste")
            .expect("dialog not drawn")
            .size
            .height;
        let answers = answers.borrow().clone();
        (height, answers)
    }

    #[gpui::test]
    fn long_paste_tells_how_many_lines_are_not_shown(cx: &mut gpui::TestAppContext) {
        let six = "1\n2\n3\n4\n5\n6";
        let (fits, _) = drawn(six, "", cx);
        let (cut, _) = drawn(&format!("{six}\n7\n8"), "", cx);
        // one more row under the preview for the lines left out
        assert!(cut > fits, "{cut:?} is not taller than {fits:?}");
    }

    #[gpui::test]
    fn hidden_characters_past_the_preview_still_warn(cx: &mut gpui::TestAppContext) {
        let text = "1\n2\n3\n4\n5\n6\n7\nrm\u{200b} -rf";
        let dialog = cx.new(|cx| ConfirmPaste::new(text, cx));
        assert!(dialog.read_with(cx, |dialog, _| dialog.flagged));
    }

    #[gpui::test]
    fn keys_other_than_answers_leave_the_dialog_up(cx: &mut gpui::TestAppContext) {
        let (_, answers) = drawn("a\nb", "x space right", cx);
        assert!(answers.is_empty(), "{answers:?}");
        // right picked "paste", so enter pastes
        let (_, answers) = drawn("a\nb", "right enter", cx);
        assert_eq!(answers, [true]);
    }
}
