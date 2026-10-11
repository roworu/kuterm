//! notifications stacked in the bottom right corner of the window, not the system ones

use std::time::Duration;

use gpui::{AnyElement, Context, Entity, Task, Window, deferred, div, prelude::*, rems};

use super::Workspace;
use crate::{
    settings::Settings, terminal::foreground_process, theme::Theme, ui::terminal_view::TerminalView,
};

// older notifications are dropped once this many are shown
const NOTIFICATIONS_MAX: usize = 5;
// how often a watched tab is checked for its program to finish
const DONE_POLL: Duration = Duration::from_millis(250);
// a command may end before any poll sees it running, so an idle shell counts as done after
// this many polls. a running one is done after two idle polls, so the short gap between
// "a; b" does not end the watch early
const DONE_IDLE_POLLS: u32 = 4;
const DONE_IDLE_POLLS_AFTER_START: u32 = 2;
// copied text longer than this many chars is cut, so it and its label fit one notification line
const COPIED_PREVIEW_CHARS: usize = 28;

pub(super) struct Notification {
    id: usize,
    pub(super) text: String,
    /// shown after `text` in its own style, like the copied text
    pub(super) detail: Option<String>,
    /// tab switched to when the notification is clicked
    tab: Option<Entity<TerminalView>>,
    /// a problem the user has to fix, like a broken config file
    pub(super) error: bool,
    _dismiss: Task<()>,
}

impl Workspace {
    /// show `text` until it times out or is clicked, a click switches to `tab`
    pub(super) fn show_notification(
        &mut self,
        text: impl Into<String>,
        tab: Option<Entity<TerminalView>>,
        cx: &mut Context<Self>,
    ) {
        self.push_notification(text.into(), None, tab, false, cx);
    }

    /// show a problem until it is clicked, even with notifications turned off, since
    /// nothing else in the window tells about it
    pub(super) fn show_error(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.push_notification(text.into(), None, None, true, cx);
    }

    fn push_notification(
        &mut self,
        text: String,
        detail: Option<String>,
        tab: Option<Entity<TerminalView>>,
        error: bool,
        cx: &mut Context<Self>,
    ) {
        let settings = &Settings::get(cx).notifications;
        if !settings.enable && !error {
            return;
        }
        let timeout = if error { 0. } else { settings.timeout };
        let id = self.next_notification_id;
        self.next_notification_id += 1;
        let _dismiss = if timeout > 0. {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_secs_f32(timeout))
                    .await;
                this.update(cx, |this, cx| this.dismiss_notification(id, cx))
                    .ok();
            })
        } else {
            Task::ready(())
        };
        self.notifications.push(Notification {
            id,
            text,
            detail,
            tab,
            error,
            _dismiss,
        });
        if self.notifications.len() > NOTIFICATIONS_MAX {
            self.notifications.remove(0);
        }
        cx.notify();
    }

    /// tell what was copied, when `notifications.copy` is on
    pub(super) fn notify_copied(&mut self, text: &str, cx: &mut Context<Self>) {
        if Settings::get(cx).notifications.copy {
            let detail = Some(copied_preview(text));
            self.push_notification("copied".into(), detail, None, false, cx);
        }
    }

    /// tell that clipboard was pasted, when `notifications.paste` is on
    pub(super) fn notify_pasted(&mut self, cx: &mut Context<Self>) {
        if Settings::get(cx).notifications.paste {
            self.show_notification("pasted from clipboard", None, cx);
        }
    }

    fn dismiss_notification(&mut self, id: usize, cx: &mut Context<Self>) {
        self.notifications
            .retain(|notification| notification.id != id);
        cx.notify();
    }

    fn click_notification(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self
            .notifications
            .iter()
            .find(|notification| notification.id == id)
            .and_then(|notification| notification.tab.as_ref())
            .and_then(|view| self.find_view(view));
        if let Some((pane, ix)) = tab {
            self.activate_tab(pane, ix, window, cx);
        }
        self.dismiss_notification(id, cx);
    }

    /// notify with `text` once the program running in `view` returns to the shell
    pub(super) fn notify_when_done(
        &mut self,
        view: Entity<TerminalView>,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let shell_pid = view.read(cx).terminal().read(cx).shell_pid;
        cx.spawn(async move |this, cx| {
            let mut started = false;
            let mut idle_polls = 0;
            loop {
                cx.background_executor().timer(DONE_POLL).await;
                let Ok(tab) = this.update(cx, |this, _| {
                    let (pane, ix) = this.find_view(&view)?;
                    this.pane(pane)?
                        .tabs
                        .get(ix)
                        .map(|tab| tab.ready && tab.pending_input.is_empty())
                }) else {
                    return;
                };
                // a closed tab has nothing left running
                let Some(sent) = tab else {
                    break;
                };
                // typed input still waits for a starting shell
                if !sent {
                    continue;
                }
                // /proc reads may block, keep them off the main thread
                let busy = cx
                    .background_executor()
                    .spawn(async move {
                        foreground_process(shell_pid)
                            .is_some_and(|process| process.pid != shell_pid)
                    })
                    .await;
                if busy {
                    started = true;
                    idle_polls = 0;
                    continue;
                }
                idle_polls += 1;
                let needed = if started {
                    DONE_IDLE_POLLS_AFTER_START
                } else {
                    DONE_IDLE_POLLS
                };
                if idle_polls >= needed {
                    break;
                }
            }
            this.update(cx, |this, cx| this.show_notification(text, Some(view), cx))
                .ok();
        })
        .detach();
    }

    pub(super) fn render_notifications(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.notifications.is_empty() {
            return None;
        }
        let theme = Theme::get(cx);
        let notifications = self.notifications.iter().map(|notification| {
            let id = notification.id;
            div()
                .id(("notification", id))
                .debug_selector(move || format!("notification-{id}"))
                .max_w(rems(24.))
                .px_3()
                .py_2()
                .bg(theme.tab_bar_background)
                .border_1()
                .border_color(if notification.error {
                    theme.danger_button
                } else {
                    theme.border
                })
                .rounded_md()
                .shadow_lg()
                .text_color(theme.text)
                .cursor_pointer()
                .hover(|item| item.bg(theme.tab_active_background))
                .flex()
                .items_center()
                .gap_2()
                .child(notification.text.clone())
                // terminal colors set the copied text apart from the label
                .children(notification.detail.clone().map(|detail| {
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(theme.terminal_background)
                        .text_color(theme.terminal_foreground)
                        .child(detail)
                }))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.click_notification(id, window, cx)),
                )
        });
        Some(
            deferred(
                div()
                    .absolute()
                    .bottom_0()
                    .right_0()
                    .p_3()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap_2()
                    // keeps clicks from reaching the terminal below
                    .occlude()
                    .children(notifications),
            )
            .into_any_element(),
        )
    }
}

// whitespace runs, newlines too, become one space so a notification stays a single line
fn copied_preview(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= COPIED_PREVIEW_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(COPIED_PREVIEW_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::{COPIED_PREVIEW_CHARS, copied_preview};

    #[test]
    fn short_text_is_shown_whole() {
        assert_eq!(copied_preview("hello world"), "hello world");
    }

    #[test]
    fn newlines_and_runs_of_spaces_become_one_space() {
        assert_eq!(copied_preview("  a\n\tb   c\n"), "a b c");
    }

    #[test]
    fn long_text_is_cut_with_ellipsis() {
        let preview = copied_preview(&"x".repeat(COPIED_PREVIEW_CHARS + 10));
        assert_eq!(preview.chars().count(), COPIED_PREVIEW_CHARS);
        assert!(preview.ends_with('…'));
        assert_eq!(
            copied_preview(&"y".repeat(COPIED_PREVIEW_CHARS)),
            "y".repeat(COPIED_PREVIEW_CHARS)
        );
    }

    #[test]
    fn cut_respects_multibyte_chars() {
        let preview = copied_preview(&"日".repeat(COPIED_PREVIEW_CHARS * 2));
        assert_eq!(preview.chars().count(), COPIED_PREVIEW_CHARS);
    }
}
