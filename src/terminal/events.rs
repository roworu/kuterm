//! reacting to events reported by alacritty's event loop

use alacritty_terminal::event::Event as AlacTermEvent;
use gpui::{ClipboardItem, Context};

use super::{Event, Terminal};

impl Terminal {
    pub(super) fn process_event(&mut self, event: AlacTermEvent, cx: &mut Context<Self>) {
        match event {
            AlacTermEvent::Title(title) => {
                self.title = title;
                cx.emit(Event::TitleChanged);
            }
            AlacTermEvent::ResetTitle => {
                self.title.clear();
                cx.emit(Event::TitleChanged);
            }
            AlacTermEvent::ClipboardStore(_, data) => {
                cx.write_to_clipboard(ClipboardItem::new_string(data))
            }
            AlacTermEvent::ClipboardLoad(_, format) => {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                self.write_to_pty(format(&text).into_bytes());
            }
            AlacTermEvent::PtyWrite(out) => self.write_to_pty(out.into_bytes()),
            AlacTermEvent::TextAreaSizeRequest(format) => {
                self.write_to_pty(format(self.last_content.terminal_bounds.into()).into_bytes())
            }
            AlacTermEvent::ColorRequest(index, format) => {
                // answer inline so replies keep their order relative to other pty writes
                let color = self.term.lock().colors()[index].unwrap_or_else(|| {
                    let rgba: gpui::Rgba = self.theme(cx).get_color_at_index(index).into();
                    alacritty_terminal::vte::ansi::Rgb {
                        r: (rgba.r * 255.).round() as u8,
                        g: (rgba.g * 255.).round() as u8,
                        b: (rgba.b * 255.).round() as u8,
                    }
                });
                self.write_to_pty(format(color).into_bytes());
            }
            AlacTermEvent::Exit | AlacTermEvent::ChildExit(_) => cx.emit(Event::CloseTerminal),
            AlacTermEvent::Wakeup => cx.emit(Event::Wakeup),
            AlacTermEvent::Bell => cx.emit(Event::Bell),
            AlacTermEvent::MouseCursorDirty | AlacTermEvent::CursorBlinkingChange => {}
        }
    }
}
