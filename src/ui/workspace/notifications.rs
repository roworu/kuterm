//! notifications stacked in the bottom right corner of the window, not the system ones

use std::{rc::Rc, time::Duration};

use gpui::{AnyElement, Context, Entity, Task, Window, deferred, div, prelude::*, rems};

use super::{Workspace, tab_bar::STATUS_BAR_HEIGHT};
use crate::{
    settings::{ProgramNotifications, Settings},
    terminal::{Link, foreground_process},
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

/// button of a desktop notification, run on the tab it tells about
#[derive(Clone, Debug, PartialEq)]
pub(super) enum SystemAction {
    /// switch to the tab
    Show,
    /// type this command line into the tab and run it
    RunAgain(String),
    /// run the action of the hint pattern that found this `file:line`
    OpenError(Link),
}

impl SystemAction {
    fn key(&self) -> &'static str {
        match self {
            // servers run "default" on a click on the notification itself
            SystemAction::Show => "default",
            SystemAction::RunAgain(_) => "run-again",
            SystemAction::OpenError(_) => "open-error",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            SystemAction::Show => "show",
            SystemAction::RunAgain(_) => "run again",
            SystemAction::OpenError(_) => "open first error",
        }
    }
}

/// buttons for a finished command: show, run again when its line is known, and open first
/// error when it failed and printed a `file:line`
pub(super) fn finished_actions(
    exit: Option<i32>,
    line: Option<String>,
    location: Option<Link>,
) -> Vec<SystemAction> {
    let failed = exit.is_some_and(|code| code != 0);
    let mut actions = vec![SystemAction::Show];
    actions.extend(line.map(SystemAction::RunAgain));
    actions.extend(location.filter(|_| failed).map(SystemAction::OpenError));
    actions
}

impl Workspace {
    /// show `text` until it times out or is clicked, a click switches to `tab`
    pub(super) fn show_notification(
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
        if self.notifications.len() > NOTIFICATIONS_MAX {
            // questions wait for an answer, so plain notifications go first
            let oldest = self
                .notifications
                .iter()
                .position(|notification| notification.buttons.is_empty())
                .unwrap_or(0);
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

    /// desktop notification whose buttons run `actions` on the tab of `view`. other systems
    /// than linux show it without buttons
    fn system_notification_with_actions(
        &self,
        view: &Entity<TerminalView>,
        title: &str,
        body: &str,
        actions: Vec<SystemAction>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(target_os = "linux")]
        {
            let buttons = actions.iter().map(|a| (a.key(), a.label())).collect();
            let picked = desktop_notification(title.to_string(), body.to_string(), buttons);
            let view = view.clone();
            cx.spawn_in(window, async move |this, cx| {
                let Ok(key) = picked.await else {
                    return;
                };
                let Some(action) = actions.into_iter().find(|action| action.key() == key) else {
                    return;
                };
                this.update_in(cx, |this, window, cx| {
                    this.run_system_action(&view, action, window, cx)
                })
                .ok();
            })
            .detach();
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (view, actions, window, cx);
            system_notification(title, body);
        }
    }

    /// a button of a desktop notification about the tab of `view` was clicked
    fn run_system_action(
        &mut self,
        view: &Entity<TerminalView>,
        action: SystemAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.activate_window();
        let Some((pane, ix)) = self.find_view(view) else {
            return;
        };
        self.activate_tab(pane, ix, window, cx);
        match action {
            SystemAction::Show => {}
            SystemAction::RunAgain(line) => {
                view.update(cx, |view, cx| view.commit_text(&format!("{line}\r"), cx))
            }
            SystemAction::OpenError(link) => view.update(cx, |view, cx| view.run_link(&link, cx)),
        }
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
                // long text wraps, so the buttons after it stay inside the max width
                .child(div().min_w_0().child(notification.text.clone()))
                // terminal colors set the copied text apart from the label
                .children(notification.detail.clone().map(|detail| {
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(theme.terminal_background)
                        .text_color(theme.terminal_foreground)
                        .child(detail)
                }))
                .children(notification.buttons.iter().enumerate().map(|(ix, button)| {
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
                }))
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

/// a notification of the desktop, for when the window is not looked at
pub(super) fn system_notification(title: &str, body: &str) {
    let mut command = if cfg!(target_os = "macos") {
        // quoted as applescript strings
        let quote = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
        let mut command = std::process::Command::new("osascript");
        command.arg("-e").arg(format!(
            "display notification {} with title {}",
            quote(body),
            quote(title)
        ));
        command
    } else {
        let mut command = std::process::Command::new("notify-send");
        command.arg("--app-name=kuterm").arg(title).arg(body);
        command
    };
    // the desktop may have no notification service, that only loses the notification
    std::thread::spawn(move || {
        command.status().ok();
    });
}

/// a desktop notification with `buttons` as (key, label), over dbus. the key of the clicked
/// one comes back, nothing when it is closed or there is no notification service
#[cfg(target_os = "linux")]
fn desktop_notification(
    title: String,
    body: String,
    buttons: Vec<(&'static str, &'static str)>,
) -> futures::channel::oneshot::Receiver<&'static str> {
    const NAME: &str = "org.freedesktop.Notifications";
    let (picked, receiver) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let run = || -> zbus::Result<Option<&'static str>> {
            let connection = zbus::blocking::Connection::session()?;
            // listening before the notification shows, so a quick click is not missed
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface(NAME)?
                .build();
            let signals = zbus::blocking::MessageIterator::for_match_rule(rule, &connection, None)?;
            let actions: Vec<&str> = buttons
                .iter()
                .flat_map(|(key, label)| [*key, *label])
                .collect();
            let hints: std::collections::HashMap<&str, zbus::zvariant::Value> = Default::default();
            let reply = connection.call_method(
                Some(NAME),
                "/org/freedesktop/Notifications",
                Some(NAME),
                "Notify",
                &(
                    "kuterm",
                    0u32,
                    "",
                    title.as_str(),
                    body.as_str(),
                    actions,
                    hints,
                    -1i32,
                ),
            )?;
            let id: u32 = reply.body().deserialize()?;
            for message in signals {
                let message = message?;
                let header = message.header();
                match header.member().map(|member| member.as_str()) {
                    Some("ActionInvoked") => {
                        let (from, key): (u32, String) = message.body().deserialize()?;
                        if from == id {
                            return Ok(buttons.iter().find(|(k, _)| *k == key).map(|(k, _)| *k));
                        }
                    }
                    Some("NotificationClosed") => {
                        let (from, _): (u32, u32) = message.body().deserialize()?;
                        if from == id {
                            return Ok(None);
                        }
                    }
                    _ => {}
                }
            }
            Ok(None)
        };
        // the desktop may have no notification service, that only loses the notification
        if let Ok(Some(key)) = run() {
            picked.send(key).ok();
        }
    });
    receiver
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::index::{Column, Line, Point};

    use super::{COPIED_PREVIEW_CHARS, SystemAction, copied_preview, finished_actions};
    use crate::{settings::PatternAction, terminal::Link};

    #[test]
    fn finished_commands_offer_what_they_can() {
        let at = Point::new(Line(0), Column(0));
        let location = Link {
            range: at..=at,
            rest: Vec::new(),
            target: "src/main.rs:12".into(),
            action: PatternAction::Copy,
            name: Some("path".into()),
        };
        let line = || Some("make".to_string());
        let run_again = SystemAction::RunAgain("make".into());
        // success never offers errors, even when the output names a file
        assert_eq!(
            finished_actions(Some(0), line(), Some(location.clone())),
            [SystemAction::Show, run_again.clone()]
        );
        assert_eq!(
            finished_actions(Some(2), line(), Some(location.clone())),
            [
                SystemAction::Show,
                run_again.clone(),
                SystemAction::OpenError(location)
            ]
        );
        assert_eq!(
            finished_actions(Some(2), line(), None),
            [SystemAction::Show, run_again]
        );
        // an unknown command line can't run again
        assert_eq!(finished_actions(None, None, None), [SystemAction::Show]);
    }

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
