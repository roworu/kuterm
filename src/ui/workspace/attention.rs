//! ai agents that stopped working without finishing, so they wait for the user

use std::time::{Duration, Instant};

use gpui::{Context, Entity, Task, Window};

use super::{Workspace, system_notifications::system_notification};
use crate::{settings::Settings, ui::terminal_view::TerminalView};

/// whether the agent of a tab waits for the user, kept per tab
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Attention {
    since: Option<Instant>,
    /// input count of the terminal when the wait began, any input after it ends the wait
    inputs: u64,
}

impl Attention {
    /// the program stopped being busy without its command finishing. true when an agent
    /// starts waiting now, so it is told once per wait
    pub(super) fn idle(&mut self, agent: bool, inputs: u64, now: Instant) -> bool {
        if !agent || self.waiting(inputs).is_some() {
            return false;
        }
        self.wait(inputs, now);
        true
    }

    /// a program waits for an answer since `now`, kept when it already waited
    pub(super) fn wait(&mut self, inputs: u64, now: Instant) {
        if self.waiting(inputs).is_none() {
            self.since = Some(now);
            self.inputs = inputs;
        }
    }

    /// the program is busy again or its command finished
    pub(super) fn clear(&mut self) {
        self.since = None;
    }

    /// when the wait began, none when nothing waits or input came since
    pub(super) fn waiting(&self, inputs: u64) -> Option<Instant> {
        self.since.filter(|_| inputs == self.inputs)
    }

    /// drop a wait input came after, true when there was one, so its mark goes away
    pub(super) fn forget_answered(&mut self, inputs: u64) -> bool {
        let answered = self.since.is_some() && inputs != self.inputs;
        if answered {
            self.since = None;
        }
        answered
    }
}

// a program often takes its progress down and exits in separate writes, so stopping being
// busy is a wait only when the command did not finish within this
pub(super) const IDLE_GRACE: Duration = Duration::from_millis(500);

impl Workspace {
    /// the program of the tab of `view` stopped being busy, a wait once `IDLE_GRACE` passed
    /// without its command finishing
    pub(super) fn agent_stopped_later(
        &mut self,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn_in(window, {
            let view = view.clone();
            async move |this, cx| {
                cx.background_executor().timer(IDLE_GRACE).await;
                this.update_in(cx, |this, window, cx| this.agent_stopped(&view, window, cx))
                    .ok();
            }
        });
        if let Some(tab) = self.tab_mut(view) {
            tab.idle_task = task;
        }
    }

    /// the program of the tab of `view` is busy again or its command finished
    pub(super) fn attention_ended(&mut self, view: &Entity<TerminalView>, cx: &mut Context<Self>) {
        if let Some(tab) = self.tab_mut(view) {
            tab.attention.clear();
            tab.idle_task = Task::ready(());
            cx.notify();
        }
    }

    /// the program of the tab of `view` stopped being busy without finishing. an agent now
    /// waits for the user: its tab is marked and, out of sight, told about once
    pub(super) fn agent_stopped(
        &mut self,
        view: &Entity<TerminalView>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !Settings::get(cx).notifications.agent_waiting {
            return;
        }
        let inputs = view.read(cx).terminal().read(cx).inputs();
        let Some(tab) = self.tab_mut(view) else {
            return;
        };
        if !tab.attention.idle(tab.agent, inputs, Instant::now()) {
            return;
        }
        cx.notify();
        if self.tab_seen(view, window) {
            return;
        }
        let number = self.find_view(view).map_or(0, |(_, ix)| ix + 1);
        let text = format!("the agent in tab {number} waits for you");
        if !window.is_window_active() {
            system_notification("kuterm", &text);
        }
        self.show_notification(text, Some(view.clone()), cx);
    }

    /// switch to the tab whose agent has waited longest, other than the active one
    pub(super) fn next_attention_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut waiting = Vec::new();
        for pane in self.layout.panes() {
            for (ix, tab) in pane.tabs.iter().enumerate() {
                let inputs = tab.view.read(cx).terminal().read(cx).inputs();
                if let Some(since) = tab.attention.waiting(inputs)
                    && !(pane.id == self.focused && ix == pane.active)
                {
                    waiting.push((since, pane.id, ix));
                }
            }
        }
        if let Some((_, pane, ix)) = waiting.into_iter().min() {
            self.activate_tab(pane, ix, window, cx);
        }
    }

    /// palette listing the tabs running an agent with how long each waits, picking one
    /// switches to it
    pub(super) fn open_agent_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let mut rows = Vec::new();
        for pane in self.layout.panes() {
            for (ix, tab) in pane.tabs.iter().enumerate() {
                let terminal = tab.view.read(cx).terminal().read(cx);
                let since = tab.attention.waiting(terminal.inputs());
                if !tab.agent && since.is_none() {
                    continue;
                }
                let state = match since {
                    Some(since) => format!("waiting {}", waited((now - since).as_secs())),
                    None if terminal.progress().is_some() => "working".into(),
                    None => "idle".into(),
                };
                // waiting tabs first, the longest waiting on top
                rows.push((since.unwrap_or(now), state, pane.id, ix));
            }
        }
        rows.sort_by_key(|(since, ..)| *since);
        let rows = rows
            .into_iter()
            .map(|(_, state, pane, ix)| (state, pane, ix));
        self.open_tab_rows(rows.collect(), window, cx);
    }
}

/// "3m" or "1h 5m" for how long a tab waited
pub(super) fn waited(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[gpui::test]
    fn an_agent_waits_only_when_its_command_does_not_end_soon(cx: &mut gpui::TestAppContext) {
        use crate::{terminal::Event, ui::workspace::test_support::*};

        let (ws, cx) = open(cx, 2);
        let waiting = |ws: &Entity<Workspace>, cx: &mut VisualTestContext| {
            ws.update(cx, |ws, cx| {
                let tab = &mut ws.pane_mut(ws.focused).unwrap().tabs[0];
                tab.agent = true;
                let inputs = tab.view.read(cx).terminal().read(cx).inputs();
                tab.attention.waiting(inputs).is_some()
            })
        };
        let agent = terminal(&ws, cx, 0);
        waiting(&ws, cx);
        // progress taken down, then the command ends in a later read
        agent.update(cx, |_, cx| cx.emit(Event::BusyChanged(false)));
        cx.run_until_parked();
        agent.update(cx, |_, cx| {
            cx.emit(Event::CommandFinished {
                duration: Duration::from_secs(1),
                exit: Some(0),
            })
        });
        cx.executor().advance_clock(IDLE_GRACE);
        cx.run_until_parked();
        assert!(!waiting(&ws, cx));
        // no end follows, so it waits
        agent.update(cx, |_, cx| cx.emit(Event::BusyChanged(false)));
        cx.executor().advance_clock(IDLE_GRACE);
        cx.run_until_parked();
        assert!(waiting(&ws, cx));
    }

    #[test]
    fn busy_then_idle_waits_until_input() {
        let now = Instant::now();
        let mut attention = Attention::default();
        assert_eq!(attention.waiting(0), None);
        assert!(attention.idle(true, 3, now));
        assert_eq!(attention.waiting(3), Some(now));
        // idle again in the same wait tells nothing new
        assert!(!attention.idle(true, 3, now + Duration::from_secs(1)));
        assert_eq!(attention.waiting(3), Some(now));
        // typing into the tab ends it, the next idle starts a new one
        assert_eq!(attention.waiting(4), None);
        assert!(!attention.forget_answered(3));
        assert!(attention.forget_answered(4));
        assert!(!attention.forget_answered(4));
        let later = now + Duration::from_secs(5);
        assert!(attention.idle(true, 4, later));
        assert_eq!(attention.waiting(4), Some(later));
    }

    #[test]
    fn busy_again_or_finished_ends_the_wait() {
        let now = Instant::now();
        let mut attention = Attention::default();
        assert!(attention.idle(true, 0, now));
        attention.clear();
        assert_eq!(attention.waiting(0), None);
        assert!(attention.idle(true, 0, now));
    }

    #[test]
    fn other_programs_never_wait() {
        let mut attention = Attention::default();
        assert!(!attention.idle(false, 0, Instant::now()));
        assert_eq!(attention.waiting(0), None);
    }

    #[test]
    fn wait_reads_in_the_biggest_unit() {
        assert_eq!(waited(5), "5s");
        assert_eq!(waited(180), "3m");
        assert_eq!(waited(3900), "1h 5m");
    }
}
