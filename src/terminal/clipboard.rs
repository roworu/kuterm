//! clipboard access asked for by programs (osc 52) and pasting into the terminal

use std::sync::Arc;

use alacritty_terminal::term::{ClipboardType, TermMode};
use gpui::{ClipboardItem, Context};

use super::*;
use crate::settings::ClipboardAccess;

impl Terminal {
    // a denied read gets no reply, like a terminal without osc 52
    pub(super) fn request_clipboard(
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
                    ClipboardRequest::Write(_, text) => Some(text.clone()),
                    ClipboardRequest::Read(..) => None,
                };
                self.clipboard_requests.insert(id, request);
                cx.emit(Event::ClipboardAsk { id, text });
            }
        }
    }
}

/// clipboard access waiting for the user to allow it
pub(super) enum ClipboardRequest {
    Write(ClipboardType, String),
    /// builds the reply to the program from the clipboard text
    Read(
        ClipboardType,
        Arc<dyn Fn(&str) -> String + Sync + Send + 'static>,
    ),
}

/// bytes a paste sends: bracketed paste wraps it and drops control characters, so the text
/// can't fake the end of the paste, otherwise newlines become enter presses
pub(super) fn pasted(text: &str, bracketed: bool) -> String {
    if bracketed {
        let text: String = text
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
            .collect();
        format!("\x1b[200~{text}\x1b[201~")
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r")
    }
}

impl Terminal {
    /// paste text from `source`, wrapping it when program asked for bracketed paste. false
    /// when it was refused
    pub fn paste(&mut self, source: InputSource, text: &str) -> bool {
        let bracketed = self.last_content.mode.contains(TermMode::BRACKETED_PASTE);
        self.input(source, pasted(text, bracketed).into_bytes())
    }

    /// true when pasting `text` should be confirmed: without bracketed paste a newline runs
    /// it right away and control characters reach the program as keys
    pub fn paste_is_unsafe(&self, text: &str) -> bool {
        !self.last_content.mode.contains(TermMode::BRACKETED_PASTE)
            && text.chars().any(|c| c.is_control() && c != '\t')
    }

    /// let or refuse a clipboard access the program asked for
    pub fn answer_clipboard(&mut self, id: usize, allow: bool, cx: &mut Context<Self>) {
        let Some(request) = self.clipboard_requests.remove(&id) else {
            return;
        };
        if allow {
            self.use_clipboard(request, cx);
        }
    }

    pub(super) fn use_clipboard(&mut self, request: ClipboardRequest, cx: &mut Context<Self>) {
        match request {
            ClipboardRequest::Write(ClipboardType::Clipboard, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            ClipboardRequest::Write(ClipboardType::Selection, text) => {
                cx.write_to_primary(ClipboardItem::new_string(text))
            }
            ClipboardRequest::Read(kind, format) => {
                let item = match kind {
                    ClipboardType::Clipboard => cx.read_from_clipboard(),
                    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                    ClipboardType::Selection => cx.read_from_primary(),
                    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
                    ClipboardType::Selection => None,
                };
                let text = item.and_then(|item| item.text()).unwrap_or_default();
                self.write_to_pty(format(&text).into_bytes());
            }
            // no primary selection outside linux, and it must not land in the clipboard
            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
            ClipboardRequest::Write(ClipboardType::Selection, _) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::Event as AlacTermEvent;
    use gpui::AppContext;

    use super::*;
    use crate::terminal::tests::spawn;

    #[test]
    fn bracketed_paste_drops_control_characters() {
        assert_eq!(
            super::pasted("a\x1b[201~b\x03c\u{9b}d\te\nf\r", true),
            "\x1b[200~a[201~bcd\te\nf\r\x1b[201~"
        );
        assert_eq!(super::pasted("a\nb\r\nc", false), "a\rb\rc");
    }

    #[test]
    fn paste_is_unsafe_only_without_bracketed_paste() {
        let mut builder = spawn(&TerminalSettings::default());
        let terminal = &mut builder.terminal;
        terminal.last_content.mode = TermMode::empty();
        assert!(!terminal.paste_is_unsafe("ls -la\tdone"));
        assert!(terminal.paste_is_unsafe("ls\n"));
        assert!(terminal.paste_is_unsafe("ls\r"));
        assert!(terminal.paste_is_unsafe("\x1b[A"));
        terminal.last_content.mode = TermMode::BRACKETED_PASTE;
        assert!(!terminal.paste_is_unsafe("ls\nrm -rf x\x1b[201~"));
    }

    #[gpui::test]
    fn clipboard_access_follows_settings(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        use alacritty_terminal::term::ClipboardType;
        use gpui::ClipboardItem;

        use super::Event;
        use crate::settings::{ClipboardAccess, ClipboardSettings};

        cx.update(|cx| cx.set_global(crate::theme::Theme::default()));
        let settings = TerminalSettings {
            clipboard: ClipboardSettings {
                write: ClipboardAccess::Deny,
                read: ClipboardAccess::Ask,
            },
            ..TerminalSettings::default()
        };
        let builder = spawn(&settings);
        let terminal = cx.new(|_| builder.terminal);
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&terminal, move |_, event: &Event, _| {
                events.borrow_mut().push(event.clone())
            })
            .detach()
        });
        cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
        let store =
            |text: &str| AlacTermEvent::ClipboardStore(ClipboardType::Clipboard, text.into());
        let clipboard =
            |cx: &mut gpui::TestAppContext| cx.read_from_clipboard().and_then(|item| item.text());

        terminal.update(cx, |terminal, cx| {
            terminal.process_event(store("denied"), cx)
        });
        assert_eq!(clipboard(cx), Some("before".into()));
        let load = std::sync::Arc::new(|text: &str| format!("load:{text}"));
        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                AlacTermEvent::ClipboardLoad(ClipboardType::Clipboard, load),
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(
            *events.borrow(),
            [Event::ClipboardAsk { id: 0, text: None }]
        );

        terminal.update(cx, |terminal, cx| {
            terminal.clipboard.write = ClipboardAccess::Ask;
            terminal.process_event(store("asked"), cx);
        });
        cx.run_until_parked();
        assert_eq!(
            events.borrow().last(),
            Some(&Event::ClipboardAsk {
                id: 1,
                text: Some("asked".into())
            })
        );
        assert_eq!(clipboard(cx), Some("before".into()));
        terminal.update(cx, |terminal, cx| terminal.answer_clipboard(1, true, cx));
        assert_eq!(clipboard(cx), Some("asked".into()));
        // an answer counts once
        cx.write_to_clipboard(ClipboardItem::new_string("after".into()));
        terminal.update(cx, |terminal, cx| terminal.answer_clipboard(1, true, cx));
        assert_eq!(clipboard(cx), Some("after".into()));

        terminal.update(cx, |terminal, cx| {
            terminal.clipboard.write = ClipboardAccess::Allow;
            terminal.process_event(store("allowed"), cx);
        });
        assert_eq!(clipboard(cx), Some("allowed".into()));
    }

    #[gpui::test]
    fn primary_selection_is_kept_apart_from_the_clipboard(cx: &mut gpui::TestAppContext) {
        use alacritty_terminal::term::ClipboardType;
        use gpui::ClipboardItem;

        use crate::settings::{ClipboardAccess, ClipboardSettings};
        use crate::terminal::tests::wait_for_text;

        cx.update(|cx| cx.set_global(crate::theme::Theme::default()));
        let settings = TerminalSettings {
            clipboard: ClipboardSettings {
                write: ClipboardAccess::Allow,
                read: ClipboardAccess::Allow,
            },
            ..TerminalSettings::default()
        };
        let terminal = cx.new(|_| spawn(&settings).terminal);
        cx.write_to_clipboard(ClipboardItem::new_string("clip".into()));
        // osc 52 with "p" or "s" is the primary selection, the clipboard stays as it was
        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                AlacTermEvent::ClipboardStore(ClipboardType::Selection, "picked".into()),
                cx,
            )
        });
        let primary = cx.update(|cx| cx.read_from_primary().and_then(|item| item.text()));
        assert_eq!(primary.as_deref(), Some("picked"));
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(clipboard.as_deref(), Some("clip"));
        // reading it answers the program with the primary text, not the clipboard
        let load = std::sync::Arc::new(|text: &str| format!("echo got-{text}-end\r"));
        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                AlacTermEvent::ClipboardLoad(ClipboardType::Selection, load),
                cx,
            );
            wait_for_text(terminal, "got-picked-end");
        });
    }
}
