//! reacting to events reported by alacritty's event loop

use alacritty_terminal::event::Event as AlacTermEvent;
use gpui::Context;

use super::{ClipboardRequest, Event, Terminal};

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
            AlacTermEvent::ClipboardStore(kind, data) => {
                let access = self.clipboard.write;
                self.request_clipboard(access, ClipboardRequest::Write(kind, data), cx);
            }
            AlacTermEvent::ClipboardLoad(kind, format) => {
                let access = self.clipboard.read;
                self.request_clipboard(access, ClipboardRequest::Read(kind, format), cx);
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
                self.last_output = std::time::Instant::now();
                // output comes again, so whatever paused it let go. output already read before
                // the pause comes in for a moment after it
                if self
                    .xoff_at
                    .is_some_and(|at| at.elapsed() > super::pty_state::XOFF_GRACE)
                {
                    self.xoff_at = None;
                }
                for event in self.handle_scan_events() {
                    cx.emit(event);
                }
                cx.emit(Event::Wakeup);
            }
            AlacTermEvent::Bell => cx.emit(Event::Bell),
            AlacTermEvent::MouseCursorDirty | AlacTermEvent::CursorBlinkingChange => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use alacritty_terminal::event::Event as AlacTermEvent;
    use gpui::AppContext;

    use crate::{settings::TerminalSettings, terminal::tests::spawn};

    #[gpui::test]
    fn output_after_the_grace_ends_a_ctrl_s_pause(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(crate::theme::Theme::default()));
        let terminal = cx.new(|_| spawn(&TerminalSettings::default()).terminal);
        // output read before the pause still comes in right after it, it resumes nothing
        terminal.update(cx, |terminal, cx| {
            terminal.xoff_at = Some(Instant::now());
            terminal.process_event(AlacTermEvent::Wakeup, cx);
            assert!(terminal.xoff_paused());
        });
        // output well after it means something let the pty go again
        terminal.update(cx, |terminal, cx| {
            terminal.xoff_at = Some(Instant::now() - Duration::from_secs(2));
            terminal.process_event(AlacTermEvent::Wakeup, cx);
            assert!(!terminal.xoff_paused());
        });
    }
}
