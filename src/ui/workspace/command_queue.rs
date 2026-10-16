//! command queue of a tab: a field to queue commands while something runs, a palette to
//! edit, reorder or drop them, and the list of waiting ones over the pane

use std::time::SystemTime;

use gpui::{AnyElement, AppContext, Context, Entity, Window, div, prelude::*};

use super::{Pane, Workspace};
use crate::{
    settings::{Command, CommandAction, Inputs},
    terminal::{Terminal, parse_queue_entry},
    theme::Theme,
    ui::command_palette::{CommandPalette, InputFilled, InputPrompt},
};

// waiting commands listed over a pane, the rest are counted
const LISTED: usize = 3;

/// what a row of the queue palette does
#[derive(Clone, Copy, Debug, PartialEq)]
enum QueueOp {
    Edit(u64),
    MoveFirst(u64),
    Resume,
    Clear,
}

/// rows of the queue palette for `entries` (id and command), each with what it does
fn queue_rows(entries: &[(u64, String)], paused: bool) -> Vec<(String, &'static str, QueueOp)> {
    let mut rows: Vec<_> = entries
        .iter()
        .map(|(id, command)| (command.clone(), "edit, empty drops it", QueueOp::Edit(*id)))
        .collect();
    rows.extend(
        entries
            .iter()
            .skip(1)
            .map(|(id, command)| (command.clone(), "run first", QueueOp::MoveFirst(*id))),
    );
    if paused {
        rows.push(("resume the queue".into(), "queue", QueueOp::Resume));
    }
    if !entries.is_empty() {
        rows.push(("clear the queue".into(), "queue", QueueOp::Clear));
    }
    rows
}

impl Workspace {
    fn active_terminal(&self, cx: &gpui::App) -> Option<Entity<Terminal>> {
        let pane = self.pane(self.focused)?;
        Some(pane.tabs.get(pane.active)?.view.read(cx).terminal().clone())
    }

    /// ask for a command to type into the active tab once its prompt is back
    pub(super) fn open_queue_input(
        &mut self,
        text: Option<String>,
        edit: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal) = self.active_terminal(cx) else {
            return;
        };
        let hint = "typed when the prompt is back, @18:00 <command> waits for 18:00";
        let prompt = cx.new(|cx| {
            let prompt = InputPrompt::new("queue a command", hint, cx);
            if let Some(text) = text {
                prompt.set_text(text, cx);
            }
            prompt
        });
        self.open_overlay(&prompt, window, cx);
        self.command_input = Some(prompt.clone());
        self._overlay_subscriptions.push(cx.subscribe_in(
            &prompt,
            window,
            move |this, _, filled: &InputFilled, window, cx| {
                this.close_overlay(window, cx);
                let text = filled.0.clone();
                let queued = terminal.update(cx, |terminal, _| match edit {
                    Some(id) => terminal.update_queue(|queue| queue.edit(id, &text)),
                    None => match parse_queue_entry(&text, SystemTime::now()) {
                        Some((command, at)) => {
                            terminal.update_queue(|queue| queue.push(command, at));
                            true
                        }
                        None => text.trim().is_empty(),
                    },
                });
                if !queued {
                    this.show_error("queue takes a command, or @HH:MM and a command", cx);
                }
                cx.notify();
            },
        ));
    }

    /// palette of the active tab's queue: pick one to edit it, move it first, or resume
    pub(super) fn open_queue_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self.active_terminal(cx) else {
            return;
        };
        let queue = terminal.read(cx).queue();
        let entries: Vec<(u64, String)> = queue
            .entries()
            .iter()
            .map(|entry| (entry.id, entry.command.clone()))
            .collect();
        let rows = queue_rows(&entries, queue.paused());
        if rows.is_empty() {
            self.show_notification("the queue is empty", None, cx);
            return;
        }
        let ops: Vec<QueueOp> = rows.iter().map(|(_, _, op)| *op).collect();
        let commands = rows
            .into_iter()
            .enumerate()
            .map(|(ix, (name, category, _))| Command {
                name,
                category: Some(category.into()),
                pinned: false,
                inputs: Inputs::new(),
                // rows carry their index as their only action
                actions: vec![CommandAction::ActivateTab(ix).into()],
            })
            .collect();
        let palette = cx.new(|cx| CommandPalette::picker(commands, "queued commands...", cx));
        self.open_overlay(&palette, window, cx);
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            move |this, _, command: &Command, window, cx| {
                this.close_overlay(window, cx);
                let Some(CommandAction::ActivateTab(ix)) =
                    command.actions.first().map(|step| &step.action)
                else {
                    return;
                };
                match ops.get(*ix) {
                    Some(QueueOp::Edit(id)) => {
                        let text = terminal
                            .read(cx)
                            .queue()
                            .entries()
                            .iter()
                            .find_map(|entry| (entry.id == *id).then(|| entry.command.clone()));
                        this.open_queue_input(text, Some(*id), window, cx);
                    }
                    Some(QueueOp::MoveFirst(id)) => {
                        terminal.update(cx, |terminal, _| {
                            terminal.update_queue(|queue| queue.move_to(*id, 0))
                        });
                    }
                    Some(QueueOp::Resume) => {
                        terminal.update(cx, |terminal, _| terminal.update_queue(|q| q.resume()))
                    }
                    Some(QueueOp::Clear) => {
                        terminal.update(cx, |terminal, _| terminal.update_queue(|q| q.clear()))
                    }
                    None => {}
                }
                cx.notify();
            },
        ));
        self.palette = Some(palette);
    }

    /// go on with the active tab's queue after a failure paused it
    pub(super) fn resume_queue(&mut self, cx: &mut Context<Self>) {
        if let Some(terminal) = self.active_terminal(cx) {
            terminal.update(cx, |terminal, _| terminal.update_queue(|q| q.resume()));
            cx.notify();
        }
    }

    /// type due entries of every tab, timed ones wait for their time
    pub(super) fn tick_queues(&mut self, cx: &mut Context<Self>) {
        let terminals: Vec<_> = self
            .layout
            .panes()
            .into_iter()
            .flat_map(|pane| &pane.tabs)
            .map(|tab| tab.view.read(cx).terminal().clone())
            .filter(|terminal| !terminal.read(cx).queue().entries().is_empty())
            .collect();
        for terminal in terminals {
            if terminal.update(cx, |terminal, _| terminal.run_queue()) {
                cx.notify();
            }
        }
    }

    /// the waiting commands of the pane's active tab while a command runs, at the prompt the
    /// hint line under it shows them
    pub(super) fn render_queue_list(&self, pane: &Pane, cx: &Context<Self>) -> Option<AnyElement> {
        let terminal = pane
            .tabs
            .get(pane.active)?
            .view
            .read(cx)
            .terminal()
            .read(cx);
        let queue = terminal.queue();
        if queue.entries().is_empty() || terminal.at_prompt() {
            return None;
        }
        let theme = Theme::get(cx);
        let pane_id = pane.id;
        let more = queue.entries().len().saturating_sub(LISTED);
        let title = if queue.paused() {
            "⧗ queue, paused after a failure"
        } else {
            "⧗ queued"
        };
        Some(
            div()
                .debug_selector(move || format!("pane-queue-{pane_id}"))
                .absolute()
                .bottom_0()
                .left_0()
                .max_w_1_2()
                .px_2()
                .py_1()
                .rounded_tr_md()
                .bg(theme.tab_bar_background)
                .border_1()
                .border_color(theme.border)
                .text_sm()
                .text_color(theme.text_muted)
                .child(div().child(title))
                .children(queue.entries().iter().take(LISTED).map(|entry| {
                    div()
                        .text_color(theme.text)
                        .truncate()
                        .child(entry.command.clone())
                }))
                .when(more > 0, |list| list.child(format!("+{more} more")))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{QueueOp, queue_rows};

    #[test]
    fn rows_edit_every_entry_and_move_later_ones_first() {
        let entries = [(4, "a".to_string()), (7, "b".to_string())];
        let ops: Vec<QueueOp> = queue_rows(&entries, true)
            .into_iter()
            .map(|(_, _, op)| op)
            .collect();
        assert_eq!(
            ops,
            [
                QueueOp::Edit(4),
                QueueOp::Edit(7),
                QueueOp::MoveFirst(7),
                QueueOp::Resume,
                QueueOp::Clear
            ]
        );
        assert!(queue_rows(&[], false).is_empty());
    }
}
