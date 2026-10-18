//! notifications stacked in the bottom right corner of the window, not the system ones

use std::{rc::Rc, time::Duration};

use gpui::{AnyElement, Context, Entity, Task, Window, deferred, div, prelude::*, rems};

use super::system_notifications::*;
use super::{Workspace, tab_bar::STATUS_BAR_HEIGHT};
use crate::{
    settings::{ProgramNotifications, Settings},
    terminal::foreground_process,
    theme::Theme,
    ui::terminal_view::TerminalView,
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
// a longer detail, like a command to allow, goes on a line of its own and wraps
const INLINE_DETAIL_CHARS: usize = 40;

pub(super) struct Notification {
    id: usize,
    pub(super) text: String,
    /// shown after `text` in its own style, like the copied text
    pub(super) detail: Option<String>,
    /// tab switched to when the notification is clicked
    tab: Option<Entity<TerminalView>>,
    /// a problem the user has to fix, like a broken config file
    pub(super) error: bool,
    /// answers to a question, a notification with buttons stays until one is clicked
    buttons: Vec<NotificationButton>,
    _dismiss: Task<()>,
}

/// what clicking an answer button does
type ButtonAction = Rc<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

/// answer button of a notification that asks something
pub(super) struct NotificationButton {
    label: &'static str,
    run: ButtonAction,
}

impl NotificationButton {
    /// button showing `label`, a click runs `run` and dismisses the notification
    pub(super) fn new(
        label: &'static str,
        run: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    ) -> Self {
        Self {
            label,
            run: Rc::new(run),
        }
    }
}

impl Workspace {
    /// show `text` until it times out or is clicked, a click switches to `tab`
    pub(crate) fn show_notification(
        &mut self,
        text: impl Into<String>,
        tab: Option<Entity<TerminalView>>,
        cx: &mut Context<Self>,
    ) {
        self.push_notification(text.into(), None, tab, false, Vec::new(), cx);
    }

    /// ask something with answer `buttons`, shown until answered even with notifications
    /// turned off, since a program waits for the answer
    pub(super) fn ask(
        &mut self,
        text: impl Into<String>,
        detail: Option<String>,
        tab: Option<Entity<TerminalView>>,
        buttons: Vec<NotificationButton>,
        cx: &mut Context<Self>,
    ) {
        self.push_notification(text.into(), detail, tab, false, buttons, cx);
    }

    /// show a problem until it is clicked, even with notifications turned off, since
    /// nothing else in the window tells about it
    pub(super) fn show_error(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.push_notification(text.into(), None, None, true, Vec::new(), cx);
    }

    fn push_notification(
        &mut self,
        text: String,
        detail: Option<String>,
        tab: Option<Entity<TerminalView>>,
        error: bool,
        buttons: Vec<NotificationButton>,
        cx: &mut Context<Self>,
    ) {
        let settings = &Settings::get(cx).notifications;
        let asks = !buttons.is_empty();
        if !settings.enable && !error && !asks {
            return;
        }
        let timeout = if error || asks { 0. } else { settings.timeout };
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
            buttons,
            _dismiss,
        });
        // questions stay until answered, someone waits for the answer
        if self.notifications.len() > NOTIFICATIONS_MAX
            && let Some(oldest) = self
                .notifications
                .iter()
                .position(|notification| notification.buttons.is_empty())
        {
            self.notifications.remove(oldest);
        }
        cx.notify();
    }

    /// tell what was copied, when `notifications.copy` is on
    pub(super) fn notify_copied(&mut self, text: &str, cx: &mut Context<Self>) {
        if Settings::get(cx).notifications.copy {
            let detail = Some(copied_preview(text));
            self.push_notification("copied".into(), detail, None, false, Vec::new(), cx);
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
        let Some(notification) = self.notifications.iter().find(|n| n.id == id) else {
            return;
        };
        let asks = !notification.buttons.is_empty();
        let tab = notification
            .tab
            .as_ref()
            .and_then(|view| self.find_view(view));
        if let Some((pane, ix)) = tab {
            self.activate_tab(pane, ix, window, cx);
        }
        // a question goes away only with an answer
        if !asks {
            self.dismiss_notification(id, cx);
        }
    }

    fn click_notification_button(
        &mut self,
        id: usize,
        button: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let run = self
            .notifications
            .iter()
            .find(|notification| notification.id == id)
            .and_then(|notification| notification.buttons.get(button))
            .map(|button| button.run.clone());
        self.dismiss_notification(id, cx);
        if let Some(run) = run {
            run(self, window, cx);
        }
    }

    /// ask whether the program in the tab of `view` may use the clipboard, `text` is what it
    /// would copy, none when it wants to read
    pub(super) fn ask_clipboard(
        &mut self,
        view: &Entity<TerminalView>,
        id: usize,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let terminal = view.read(cx).terminal().clone();
        let answer = |allow: bool| {
            let terminal = terminal.clone();
            NotificationButton::new(if allow { "allow" } else { "deny" }, move |_, _, cx| {
                terminal.update(cx, |terminal, cx| terminal.answer_clipboard(id, allow, cx));
            })
        };
        let tab = self.find_view(view).map_or(0, |(_, ix)| ix + 1);
        let (question, detail) = match text {
            Some(text) => (
                format!("tab {tab} wants to copy"),
                Some(copied_preview(&text)),
            ),
            None => (format!("tab {tab} wants to read the clipboard"), None),
        };
        let buttons = vec![answer(true), answer(false)];
        self.ask(question, detail, Some(view.clone()), buttons, cx);
    }

    /// true when the tab of `view` is the one in front of the focused window
    pub(super) fn tab_seen(&self, view: &Entity<TerminalView>, window: &Window) -> bool {
        window.is_window_active()
            && self.find_view(view).is_some_and(|(pane, ix)| {
                pane == self.focused && self.pane(pane).is_some_and(|pane| pane.active == ix)
            })
    }

    /// a command marked by shell integration ended, told when it ran long out of sight
    pub(super) fn command_finished(
        &mut self,
        view: &Entity<TerminalView>,
        duration: Duration,
        exit: Option<i32>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let after = Settings::get(cx).shell_integration.notify_after;
        if after == 0. || duration.as_secs_f32() < after || self.tab_seen(view, window) {
            return;
        }
        let tab = self.find_view(view).map_or(0, |(_, ix)| ix + 1);
        let text = match exit {
            Some(0) | None => format!("command finished in tab {tab}"),
            Some(code) => format!("command failed with {code} in tab {tab}"),
        };
        if !window.is_window_active() {
            if Settings::get(cx).notifications.actions {
                let terminal = view.read(cx).terminal().clone();
                let line = terminal.read(cx).last_command_line();
                // the output is only searched when the button may be offered
                let location = exit
                    .is_some_and(|code| code != 0)
                    .then(|| TerminalView::last_output_location(&terminal, cx))
                    .flatten();
                let actions = finished_actions(exit, line, location);
                self.system_notification_with_actions(view, "kuterm", &text, actions, window, cx);
            } else {
                system_notification("kuterm", &text);
            }
        }
        self.show_notification(text, Some(view.clone()), cx);
    }

    /// a notification a program sent, shown as `notifications.from_programs` says
    pub(super) fn program_notification(
        &mut self,
        view: &Entity<TerminalView>,
        title: &str,
        body: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let show = match Settings::get(cx).notifications.from_programs {
            ProgramNotifications::Always => true,
            ProgramNotifications::Unfocused => !self.tab_seen(view, window),
            ProgramNotifications::Never => false,
        };
        if !show {
            return;
        }
        let text = match (title.is_empty(), body.is_empty()) {
            (true, _) => body.to_string(),
            (false, true) => title.to_string(),
            (false, false) => format!("{title}: {body}"),
        };
        if !window.is_window_active() {
            system_notification(if title.is_empty() { "kuterm" } else { title }, body);
        }
        self.show_notification(text, Some(view.clone()), cx);
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
            let stacked = notification
                .detail
                .as_ref()
                .is_some_and(|detail| detail.chars().count() > INLINE_DETAIL_CHARS);
            let buttons = notification.buttons.iter().enumerate().map(|(ix, button)| {
                div()
                    .id(("notification-button", ix))
                    .debug_selector(move || format!("notification-{id}-button-{ix}"))
                    .flex_none()
                    .px_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(theme.border)
                    .hover(|item| item.bg(theme.tab_bar_background))
                    .child(button.label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.click_notification_button(id, ix, window, cx);
                    }))
            });
            let buttons: Vec<_> = buttons.collect();
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
                .when(stacked, |item| item.flex_col().items_start())
                .when(!stacked, |item| item.items_center())
                .gap_2()
                // long text wraps, so the buttons after it stay inside the max width
                .child(div().min_w_0().child(notification.text.clone()))
                // terminal colors set the copied text apart from the label
                .children(notification.detail.clone().map(|detail| {
                    div()
                        .px_1()
                        .min_w_0()
                        .rounded_sm()
                        .bg(theme.terminal_background)
                        .text_color(theme.terminal_foreground)
                        .child(detail)
                }))
                .map(|item| {
                    if stacked {
                        item.child(div().flex().gap_2().children(buttons))
                    } else {
                        item.children(buttons)
                    }
                })
                .on_click(
                    cx.listener(move |this, _, window, cx| this.click_notification(id, window, cx)),
                )
        });
        Some(
            deferred(
                div()
                    .absolute()
                    // above the status bar, which shows its busiest blocks on the right
                    .bottom(rems(if Settings::get(cx).status_bar.enable {
                        STATUS_BAR_HEIGHT
                    } else {
                        0.
                    }))
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
    use super::*;
    use crate::ui::workspace::test_support::*;

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

    #[gpui::test]
    fn notify_command_shows_until_timeout(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"notifications": {"timeout": 2}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        run_actions(&ws, cx, r#"{"notify": "hello"}"#);
        assert_eq!(notifications(&ws, cx), ["hello"]);
        assert!(cx.debug_bounds("notification-0").is_some());
        cx.executor().advance_clock(Duration::from_millis(1500));
        cx.run_until_parked();
        assert_eq!(notifications(&ws, cx), ["hello"]);
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert!(notifications(&ws, cx).is_empty());
    }

    #[gpui::test]
    fn notifications_stay_above_the_status_bar(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"status_bar": {"enable": true}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        run_actions(&ws, cx, r#"{"notify": "hello"}"#);
        let note = cx.debug_bounds("notification-0").expect("no notification");
        let bar = cx.debug_bounds("status-bar").expect("no status bar");
        assert!(
            note.bottom() <= bar.top(),
            "notification at {note:?} covers the status bar at {bar:?}"
        );
    }

    #[gpui::test]
    fn zero_timeout_keeps_notification_until_clicked(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"notifications": {"timeout": 0}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        run_actions(&ws, cx, r#"{"notify": "sticky"}"#);
        cx.executor().advance_clock(Duration::from_secs(3600));
        cx.run_until_parked();
        assert_eq!(notifications(&ws, cx), ["sticky"]);
        let center = cx.debug_bounds("notification-0").unwrap().center();
        click(cx, center, 1);
        release(cx, center);
        cx.run_until_parked();
        assert!(notifications(&ws, cx).is_empty());
    }

    #[gpui::test]
    fn disabled_notifications_show_nothing(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"notifications": {"enable": false}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        run_actions(&ws, cx, r#"{"notify": "hello"}"#);
        assert!(notifications(&ws, cx).is_empty());
        assert!(cx.debug_bounds("notification-0").is_none());
    }

    #[gpui::test]
    fn copy_and_paste_keys_notify(cx: &mut TestAppContext) {
        let settings =
            Settings::parse(r#"{"notifications": {"timeout": 0, "copy": true, "paste": true}}"#)
                .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        print_line(&ws, cx, "pick these words");
        let position = cell_of(&ws, cx, "pick these words", 12, false);
        click(cx, position, 2);
        release(cx, position);
        cx.simulate_keystrokes("ctrl-shift-c");
        cx.simulate_keystrokes("ctrl-shift-v");
        cx.run_until_parked();
        assert_eq!(
            notifications(&ws, cx),
            ["copied words", "pasted from clipboard"]
        );
    }

    #[gpui::test]
    fn copy_and_paste_notifications_can_be_turned_off(cx: &mut TestAppContext) {
        let settings =
            Settings::parse(r#"{"notifications": {"copy": false, "paste": false}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        print_line(&ws, cx, "pick these words");
        let position = cell_of(&ws, cx, "pick these words", 12, false);
        click(cx, position, 2);
        release(cx, position);
        cx.simulate_keystrokes("ctrl-shift-c");
        run_actions(&ws, cx, r#""copy", "paste""#);
        cx.simulate_keystrokes("ctrl-shift-v");
        cx.run_until_parked();
        assert!(notifications(&ws, cx).is_empty());
    }

    #[gpui::test]
    fn clipboard_ask_waits_for_an_answer_button(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"notifications": {"enable": false}, "terminal": {"clipboard": {"write": "ask", "read": "ask"}}}"#,
        )
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
        let terminal = terminal(&ws, cx, 0);
        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                alacritty_terminal::event::Event::ClipboardStore(
                    alacritty_terminal::term::ClipboardType::Clipboard,
                    "from program".into(),
                ),
                cx,
            )
        });
        cx.run_until_parked();
        // asked even with notifications off, a program waits for it
        assert_eq!(notifications(&ws, cx), ["tab 1 wants to copy from program"]);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("before".into())
        );
        // clicking the text only does not answer
        let center = cx.debug_bounds("notification-0").unwrap().center();
        click(cx, center, 1);
        release(cx, center);
        cx.run_until_parked();
        assert_eq!(notifications(&ws, cx).len(), 1);
        let allow = cx.debug_bounds("notification-0-button-0").unwrap().center();
        click(cx, allow, 1);
        release(cx, allow);
        cx.run_until_parked();
        assert!(notifications(&ws, cx).is_empty());
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("from program".into())
        );

        // deny leaves the clipboard alone
        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                alacritty_terminal::event::Event::ClipboardStore(
                    alacritty_terminal::term::ClipboardType::Clipboard,
                    "second".into(),
                ),
                cx,
            )
        });
        cx.run_until_parked();
        let deny = cx.debug_bounds("notification-1-button-1").unwrap().center();
        click(cx, deny, 1);
        release(cx, deny);
        cx.run_until_parked();
        assert!(notifications(&ws, cx).is_empty());
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("from program".into())
        );
    }

    #[gpui::test]
    fn questions_outlive_the_notification_cap(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"notifications": {"timeout": 0}, "terminal": {"clipboard": {"write": "ask"}}}"#,
        )
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        let terminal = terminal(&ws, cx, 0);
        for ix in 0..super::NOTIFICATIONS_MAX + 1 {
            terminal.update(cx, |terminal, cx| {
                terminal.process_event(
                    alacritty_terminal::event::Event::ClipboardStore(
                        alacritty_terminal::term::ClipboardType::Clipboard,
                        format!("copy{ix}"),
                    ),
                    cx,
                )
            });
        }
        run_actions(&ws, cx, r#"{"notify": "plain"}"#);
        cx.run_until_parked();
        let shown = notifications(&ws, cx);
        assert_eq!(shown.len(), super::NOTIFICATIONS_MAX + 1);
        assert!(shown.iter().all(|text| text.contains("wants to copy")));
    }

    #[gpui::test]
    fn program_and_long_command_notifications_follow_settings(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"notifications": {"timeout": 0, "from_programs": "always"}, "shell_integration": {"notify_after": 5}}"#,
        )
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let notify = Event::Notify {
            title: "build".into(),
            body: "done".into(),
        };
        terminal(&ws, cx, 1).update(cx, |_, cx| cx.emit(notify.clone()));
        cx.run_until_parked();
        assert_eq!(notifications(&ws, cx), ["build: done"]);

        // a short command, or one in the tab in front, tells nothing
        let finished = |seconds: u64| Event::CommandFinished {
            duration: Duration::from_secs(seconds),
            exit: Some(1),
        };
        terminal(&ws, cx, 0).update(cx, |_, cx| cx.emit(finished(2)));
        terminal(&ws, cx, 1).update(cx, |_, cx| cx.emit(finished(60)));
        cx.run_until_parked();
        assert_eq!(notifications(&ws, cx).len(), 1);
        // a long one in a background tab does
        terminal(&ws, cx, 0).update(cx, |_, cx| cx.emit(finished(60)));
        cx.run_until_parked();
        assert_eq!(
            notifications(&ws, cx),
            ["build: done", "command failed with 1 in tab 1"]
        );

        // "unfocused" leaves out notifications of the tab in front
        let mut settings = cx.update(|_, cx| Settings::get(cx).clone());
        settings.notifications.from_programs = crate::settings::ProgramNotifications::Unfocused;
        cx.update(|_, cx| cx.set_global(settings));
        terminal(&ws, cx, 1).update(cx, |_, cx| cx.emit(notify.clone()));
        cx.run_until_parked();
        assert_eq!(notifications(&ws, cx).len(), 2);
    }

    #[gpui::test]
    fn program_notifications_show_what_they_have_and_never_hides_them(cx: &mut TestAppContext) {
        let settings =
            Settings::parse(r#"{"notifications": {"timeout": 0, "from_programs": "always"}}"#)
                .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.update(|window, _| window.activate_window());
        let notify = |title: &str, body: &str, cx: &mut VisualTestContext| {
            let event = Event::Notify {
                title: title.into(),
                body: body.into(),
            };
            terminal(&ws, cx, 0).update(cx, |_, cx| cx.emit(event));
            cx.run_until_parked();
        };
        notify("build", "", cx);
        notify("", "tests passed", cx);
        assert_eq!(notifications(&ws, cx), ["build", "tests passed"]);

        let mut settings = cx.update(|_, cx| Settings::get(cx).clone());
        settings.notifications.from_programs = crate::settings::ProgramNotifications::Never;
        cx.update(|_, cx| cx.set_global(settings));
        notify("build", "done", cx);
        assert_eq!(notifications(&ws, cx).len(), 2);
    }

    #[gpui::test]
    fn long_command_that_passed_is_told_as_finished(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"notifications": {"timeout": 0}, "shell_integration": {"notify_after": 5}}"#,
        )
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        cx.update(|window, _| window.activate_window());
        for exit in [Some(0), None] {
            let finished = Event::CommandFinished {
                duration: Duration::from_secs(60),
                exit,
            };
            terminal(&ws, cx, 0).update(cx, |_, cx| cx.emit(finished));
        }
        cx.run_until_parked();
        assert_eq!(
            notifications(&ws, cx),
            ["command finished in tab 1", "command finished in tab 1"]
        );
    }

    #[gpui::test]
    fn clipboard_read_ask_says_it_reads(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"terminal": {"clipboard": {"read": "ask"}}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.write_to_clipboard(ClipboardItem::new_string("secret".into()));
        let load = std::sync::Arc::new(|text: &str| format!("load:{text}"));
        terminal(&ws, cx, 0).update(cx, |terminal, cx| {
            terminal.process_event(
                alacritty_terminal::event::Event::ClipboardLoad(
                    alacritty_terminal::term::ClipboardType::Clipboard,
                    load,
                ),
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(
            notifications(&ws, cx),
            ["tab 1 wants to read the clipboard"]
        );
        // a question comes with its answers
        assert!(cx.debug_bounds("notification-0-button-0").is_some());
        assert!(cx.debug_bounds("notification-0-button-1").is_some());
    }

    #[gpui::test]
    fn notify_when_done_on_an_idle_shell_waits_a_few_polls(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"notifications": {"timeout": 0}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        wait_until(cx, "shell never got ready", |cx| {
            ws.update(cx, |ws, _| ws.focused_pane().tabs[0].ready)
        });
        run_actions(&ws, cx, r#"{"notify_when_done": "idle done"}"#);
        // nothing started, so it takes more idle polls than after a program ran
        for _ in 0..DONE_IDLE_POLLS - 1 {
            cx.executor().advance_clock(DONE_POLL);
            cx.run_until_parked();
        }
        assert!(notifications(&ws, cx).is_empty());
        wait_until(cx, "never notified", |cx| {
            !notifications(&ws, cx).is_empty()
        });
        assert_eq!(notifications(&ws, cx), ["idle done"]);
    }

    #[gpui::test]
    fn notify_when_done_tells_when_the_tab_closes(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"notifications": {"timeout": 0}, "close_running_tab_warn": false}"#,
        )
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        start_program(&ws, cx);
        run_actions(&ws, cx, r#"{"notify_when_done": "gone"}"#);
        pump(cx);
        assert!(notifications(&ws, cx).is_empty());
        close_active(&ws, cx);
        assert_eq!(views(&ws, cx).len(), 1);
        wait_until(cx, "never notified", |cx| {
            !notifications(&ws, cx).is_empty()
        });
        assert_eq!(notifications(&ws, cx), ["gone"]);
    }
}
