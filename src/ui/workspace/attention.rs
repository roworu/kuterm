//! ai agents that stopped working without finishing, so they wait for the user

use std::time::Instant;

use gpui::{AppContext, Context, Entity, Window};

use super::{Workspace, notifications::system_notification};
use crate::{
    settings::{Command, Inputs, Settings},
    ui::{command_palette::CommandPalette, terminal_view::TerminalView},
};

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
        self.since = Some(now);
        self.inputs = inputs;
        true
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

impl Workspace {
    /// the program of the tab of `view` is busy again or its command finished
    pub(super) fn attention_ended(&mut self, view: &Entity<TerminalView>, cx: &mut Context<Self>) {
        if let Some(tab) = self.tab_mut(view) {
            tab.attention.clear();
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
        let several_panes = self.layout.panes().len() > 1;
        let mut rows = Vec::new();
        for (pane_number, pane) in self.layout.panes().into_iter().enumerate() {
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
                // the number tells the rows apart, so a pick finds its tab again
                let number = if several_panes {
                    format!("{}.{}", pane_number + 1, ix + 1)
                } else {
                    (ix + 1).to_string()
                };
                let command = Command {
                    name: format!("{state}  {}", self.tab_title(pane.id, ix, cx)),
                    category: Some(number),
                    pinned: false,
                    inputs: Inputs::new(),
                    actions: Vec::new(),
                };
                // waiting tabs first, the longest waiting on top
                rows.push((since.unwrap_or(now), command, tab.view.entity_id()));
            }
        }
        rows.sort_by_key(|(since, _, _)| *since);
        let commands = rows.iter().map(|(_, command, _)| command.clone()).collect();
        let palette = cx.new(|cx| CommandPalette::tab_picker(commands, cx));
        self.open_overlay(&palette, window, cx);
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            move |this, _, command: &Command, window, cx| {
                this.close_overlay(window, cx);
                if let Some((_, _, id)) = rows.iter().find(|(_, row, _)| row == command) {
                    this.activate_id(*id, window, cx);
                }
            },
        ));
        self.palette = Some(palette);
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
