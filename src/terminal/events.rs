//! reacting to events reported by alacritty's event loop

use alacritty_terminal::event::Event as AlacTermEvent;
use gpui::Context;

use super::{ClipboardRequest, Event, Terminal};
use crate::settings::ClipboardAccess;

impl Terminal {
    pub(crate) fn process_event(&mut self, event: AlacTermEvent, cx: &mut Context<Self>) {
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
                let access = self.clipboard.write;
                self.request_clipboard(access, ClipboardRequest::Write(data), cx);
            }
            AlacTermEvent::ClipboardLoad(_, format) => {
                let access = self.clipboard.read;
                self.request_clipboard(access, ClipboardRequest::Read(format), cx);
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
            AlacTermEvent::Wakeup => {
                for event in self.handle_scan_events() {
                    cx.emit(event);
                }
                cx.emit(Event::Wakeup);
            }
            AlacTermEvent::Bell => cx.emit(Event::Bell),
            AlacTermEvent::MouseCursorDirty | AlacTermEvent::CursorBlinkingChange => {}
        }
    }

    // a denied read gets no reply, like a terminal without osc 52
    fn request_clipboard(
        &mut self,
        access: ClipboardAccess,
        request: ClipboardRequest,
        cx: &mut Context<Self>,
    ) {
        match access {
            ClipboardAccess::Allow => self.use_clipboard(request, cx),
            ClipboardAccess::Deny => {}
            ClipboardAccess::Ask => {
                let id = self.next_clipboard_request;
                self.next_clipboard_request += 1;
                let text = match &request {
                    ClipboardRequest::Write(text) => Some(text.clone()),
                    ClipboardRequest::Read(_) => None,
                };
                self.clipboard_requests.insert(id, request);
                cx.emit(Event::ClipboardAsk { id, text });
            }
        }
    }
}
