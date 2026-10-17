//! notifications shown by the desktop, outside the window

use gpui::{Context, Entity, Window};

use super::Workspace;
use crate::{terminal::Link, ui::terminal_view::TerminalView};

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

/// a notification of the desktop, for when the window is not looked at
pub(super) fn system_notification(title: &str, body: &str) {
    // over dbus like the ones with buttons, notify-send may not be installed
    #[cfg(target_os = "linux")]
    {
        // nothing comes back without buttons
        drop(desktop_notification(
            title.to_string(),
            body.to_string(),
            Vec::new(),
        ));
        return;
    }
    #[allow(unreachable_code)]
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
pub(super) fn desktop_notification(
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
            // nothing to click, nothing to wait for
            if buttons.is_empty() {
                return Ok(None);
            }
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

impl Workspace {
    /// desktop notification whose buttons run `actions` on the tab of `view`. other systems
    /// than linux show it without buttons
    pub(super) fn system_notification_with_actions(
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
    pub(super) fn run_system_action(
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
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::index::{Column, Line, Point};

    use super::*;
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

    mod buttons {
        use crate::ui::workspace::test_support::*;
        use crate::{settings::PatternAction, terminal::Link};

        use super::super::SystemAction;

        fn link(target: &str) -> Link {
            let at = alacritty_terminal::index::Point::default();
            Link {
                range: at..=at,
                rest: Vec::new(),
                target: target.into(),
                action: PatternAction::Copy,
                name: Some("path".into()),
            }
        }

        fn run(
            ws: &Entity<Workspace>,
            cx: &mut VisualTestContext,
            view: &Entity<TerminalView>,
            action: SystemAction,
        ) {
            ws.update_in(cx, |ws, window, cx| {
                ws.run_system_action(view, action, window, cx)
            });
            cx.run_until_parked();
        }

        #[gpui::test]
        fn show_brings_the_tab_to_front(cx: &mut TestAppContext) {
            let (ws, cx) = open(cx, 2);
            let first = ws.update(cx, |ws, _| ws.focused_pane().tabs[0].view.clone());
            run(&ws, cx, &first, SystemAction::Show);
            assert_eq!(current(&ws, cx), 0);
            assert!(cx.update(|window, _| window.is_window_active()));
        }

        #[gpui::test]
        fn open_error_runs_the_link_in_its_tab(cx: &mut TestAppContext) {
            let (ws, cx) = open(cx, 2);
            let first = ws.update(cx, |ws, _| ws.focused_pane().tabs[0].view.clone());
            run(
                &ws,
                cx,
                &first,
                SystemAction::OpenError(link("src/main.rs:12")),
            );
            assert_eq!(current(&ws, cx), 0);
            assert_eq!(clipboard(cx).as_deref(), Some("src/main.rs:12"));
        }

        #[gpui::test]
        fn a_closed_tab_leaves_nothing_to_do(cx: &mut TestAppContext) {
            let (ws, cx) = open(cx, 3);
            let first = ws.update(cx, |ws, _| ws.focused_pane().tabs[0].view.clone());
            activate(&ws, cx, 0);
            close_active(&ws, cx);
            activate(&ws, cx, 1);
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into()));
            run(
                &ws,
                cx,
                &first,
                SystemAction::OpenError(link("src/main.rs:12")),
            );
            assert_eq!(current(&ws, cx), 1);
            assert_eq!(clipboard(cx).as_deref(), Some("before"));
        }
    }
}
