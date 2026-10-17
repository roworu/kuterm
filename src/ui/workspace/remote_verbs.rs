//! remote control requests aimed at one pane: typing, reading text and blocks, closing,
//! splitting, titles and colors, palette commands, and runs the user has to allow

use std::{cell::RefCell, rc::Rc, time::UNIX_EPOCH};

use futures::channel::oneshot;
use gpui::{App, Context, EntityId, Window};

use super::{RunAction, Workspace, notifications::NotificationButton};
use crate::{
    remote::{BlockInfo, Reply, Request},
    settings::{CommandAction, Commands, Values},
    terminal::{InputSource, Terminal},
    ui::terminal_view::TerminalView,
};

/// how `--pane` found a pane, tried in this order
#[derive(Clone, Copy, PartialEq)]
pub enum PaneMatch {
    Name,
    Title,
    Id,
}

/// blocks of `terminal` asked for: block `number` (1 based, negative counts back from the last
/// finished one), or all
pub(super) fn query_blocks(
    terminal: &mut Terminal,
    number: Option<i64>,
    all: bool,
) -> Vec<BlockInfo> {
    let blocks: Vec<_> = terminal.blocks().iter().cloned().enumerate().collect();
    let picked: Vec<_> = if all {
        blocks
    } else {
        let finished: Vec<_> = blocks
            .iter()
            .filter(|(_, block)| block.ended.is_some())
            .cloned()
            .collect();
        let found = match number.unwrap_or(-1) {
            0 => None,
            n if n < 0 => finished
                .len()
                .checked_sub(n.unsigned_abs() as usize)
                .map(|ix| finished[ix].clone()),
            n => blocks.get(n as usize - 1).cloned(),
        };
        found.into_iter().collect()
    };
    picked
        .into_iter()
        .map(|(ix, block)| {
            let (output, hidden) = terminal
                .output_text(block.id)
                .map_or((None, false), |(text, hidden)| (Some(text), hidden));
            BlockInfo {
                id: block.id,
                number: ix + 1,
                command: block.command.clone(),
                exit: block.exit,
                cwd: block.cwd.clone(),
                started: block
                    .started_at
                    .duration_since(UNIX_EPOCH)
                    .map_or(0., |since| since.as_secs_f64()),
                seconds: block
                    .ended
                    .map(|ended| ended.saturating_duration_since(block.started).as_secs_f64()),
                output,
                hidden,
                link: format!("kuterm://block/{}", block.id),
                source: block.source.list(),
            }
        })
        .collect()
}

impl Workspace {
    /// tabs `spec` names, by how it named them
    pub fn find_pane(&self, spec: &str, by: PaneMatch, cx: &App) -> Option<EntityId> {
        self.layout.panes().into_iter().find_map(|pane| {
            pane.tabs.iter().enumerate().find_map(|(ix, tab)| {
                let found = match by {
                    PaneMatch::Name => tab.name.as_deref() == Some(spec),
                    PaneMatch::Title => self.tab_title(pane.id, ix, cx) == spec,
                    PaneMatch::Id => spec.parse() == Ok(tab.pane_id),
                };
                found.then(|| tab.view.entity_id())
            })
        })
    }

    /// the tab in front of the focused pane
    pub fn active_tab_id(&self) -> Option<EntityId> {
        let pane = self.focused_pane();
        Some(pane.tabs.get(pane.active)?.view.entity_id())
    }

    /// switch to the tab that ran block `id` and select its output lines, false when no tab
    /// of this window has it
    pub fn reveal_block(
        &mut self,
        id: u64,
        lines: Option<(usize, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let tab = self.layout.panes().into_iter().find_map(|pane| {
            pane.tabs.iter().enumerate().find_map(|(ix, tab)| {
                let terminal = tab.view.read(cx).terminal();
                let has = terminal.read(cx).has_block(id);
                has.then_some((pane.id, ix))
            })
        });
        let Some((pane, ix)) = tab else {
            return false;
        };
        window.activate_window();
        self.activate_tab(pane, ix, window, cx);
        self.terminal_at(pane, ix, cx)
            .update(cx, |terminal, _| terminal.select_block_lines(id, lines))
    }

    /// give the active tab a name `--pane` finds it by
    pub fn name_active_tab(&mut self, name: String) {
        let focused = self.focused;
        if let Some(pane) = self.pane_mut(focused) {
            let active = pane.active;
            pane.tabs[active].name = Some(name);
        }
    }

    /// answer `request` about the tab `id` of this window through `reply`
    pub fn remote(
        &mut self,
        id: EntityId,
        request: Request,
        reply: oneshot::Sender<Reply>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((pane, ix)) = self.find_id(id) else {
            reply.send(Reply::failed("the pane is gone")).ok();
            return;
        };
        let terminal = self.terminal_at(pane, ix, cx);
        // nothing outside may read what an incognito tab shows
        if terminal.read(cx).incognito
            && matches!(request, Request::GetText { .. } | Request::Query { .. })
        {
            reply
                .send(Reply::failed("the pane is incognito, it can't be read"))
                .ok();
            return;
        }
        // a sleeping tab's history is read too
        if matches!(request, Request::GetText { .. } | Request::Query { .. }) {
            terminal.update(cx, |terminal, _| terminal.wake());
        }
        let answer = match request {
            Request::SendText { text, .. } => {
                self.input_to(pane, ix, InputSource::Remote, text.into_bytes(), cx);
                Reply::done()
            }
            Request::GetText { scrollback, .. } => {
                let (text, hidden) = terminal.read(cx).visible_screen_text(scrollback);
                Reply {
                    ok: true,
                    text: Some(text),
                    hidden,
                    ..Reply::default()
                }
            }
            Request::FocusTab { .. } => {
                window.activate_window();
                self.activate_tab(pane, ix, window, cx);
                Reply::done()
            }
            Request::CloseTab { .. } => {
                self.request_close_tab(pane, ix, window, cx);
                Reply::done()
            }
            Request::Split { direction, .. } => {
                let action = match direction.as_str() {
                    "left" => CommandAction::SplitLeft,
                    "up" => CommandAction::SplitUp,
                    "down" => CommandAction::SplitDown,
                    _ => CommandAction::SplitRight,
                };
                self.activate_tab(pane, ix, window, cx);
                self.run_action(&RunAction(action), window, cx);
                Reply::done()
            }
            Request::SetTabTitle { title, .. } => {
                let tab = &mut self.pane_mut(pane).expect("pane of found tab").tabs[ix];
                tab.custom_title = (!title.trim().is_empty()).then_some(title);
                cx.notify();
                Reply::done()
            }
            Request::SetTabColor { color, .. } => {
                let parsed = color
                    .map(|color| {
                        serde_json_lenient::from_value::<gpui::Hsla>(color.clone().into())
                            .map_err(|_| format!("{color:?} is no color like \"#ff8800\""))
                    })
                    .transpose();
                match parsed {
                    Ok(color) => {
                        self.pane_mut(pane).expect("pane of found tab").tabs[ix].color = color;
                        cx.notify();
                        Reply::done()
                    }
                    Err(error) => Reply::failed(error),
                }
            }
            Request::RunCommand { name, inputs, .. } => {
                match self.run_remote_command(&name, &inputs, pane, ix, window, cx) {
                    Ok(()) => Reply::done(),
                    Err(error) => Reply::failed(error),
                }
            }
            Request::Query { block, all, .. } => {
                let blocks = terminal.update(cx, |terminal, _| query_blocks(terminal, block, all));
                if blocks.is_empty() {
                    Reply::failed("no such block, it may have left the history")
                } else {
                    Reply {
                        ok: true,
                        hidden: blocks.iter().any(|block| block.hidden),
                        blocks,
                        ..Reply::default()
                    }
                }
            }
            Request::Run {
                command, client, ..
            } => {
                let source = match client.as_deref() {
                    Some("mcp") => InputSource::Mcp,
                    _ => InputSource::Remote,
                };
                self.ask_to_run(pane, ix, command, source, reply, window, cx);
                return;
            }
            Request::Queue { command, .. } => {
                match crate::terminal::parse_queue_entry(&command, std::time::SystemTime::now()) {
                    Some((command, at)) => {
                        terminal.update(cx, |terminal, _| {
                            terminal.update_queue(|queue| queue.push(command, at))
                        });
                        cx.notify();
                        Reply::done()
                    }
                    None => Reply::failed("queue takes a command, or @HH:MM and a command"),
                }
            }
            _ => Reply::failed("not a pane request"),
        };
        reply.send(answer).ok();
    }

    /// run the command of `name` in the tab at (`pane`, `ix`) with `inputs`, failing when the
    /// command is unknown or wants a value the caller left out, since nothing can ask for it
    fn run_remote_command(
        &mut self,
        name: &str,
        inputs: &Values,
        pane: usize,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(command) = Commands::get(cx).find(name).cloned() else {
            return Err(format!("no command named {name:?}"));
        };
        let missing: Vec<String> = command
            .inputs
            .iter()
            .filter(|(name, input)| !input.matrix && !inputs.contains_key(*name))
            .map(|(name, _)| format!("--input {name}=<value>"))
            .collect();
        if !missing.is_empty() {
            return Err(format!("command {name:?} needs {}", missing.join(" ")));
        }
        if let Some(unknown) = inputs
            .keys()
            .find(|name| !command.inputs.contains_key(*name))
        {
            return Err(format!("command {name:?} has no input {unknown:?}"));
        }
        self.activate_tab(pane, ix, window, cx);
        self.start_command(command, inputs.clone(), window, cx);
        Ok(())
    }

    /// type `command` and enter into tab `ix` of `pane` once the user allows it here, or
    /// right away when they allowed every run in this tab before
    #[allow(clippy::too_many_arguments)]
    fn ask_to_run(
        &mut self,
        pane: usize,
        ix: usize,
        command: String,
        source: InputSource,
        reply: oneshot::Sender<Reply>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = &self.pane(pane).expect("pane of found tab").tabs[ix];
        let view = tab.view.clone();
        if tab.remote_allowed {
            let answer = run_in(self, &view, &command, source, cx);
            reply.send(answer).ok();
            return;
        }
        let reply = Rc::new(RefCell::new(Some(reply)));
        let answer = |label: &'static str, allow: bool, always: bool| {
            let (reply, command, view) = (reply.clone(), command.clone(), view.clone());
            NotificationButton::new(label, move |this: &mut Workspace, _, cx| {
                let Some(reply) = reply.borrow_mut().take() else {
                    return;
                };
                if !allow {
                    reply.send(Reply::failed("the user denied the run")).ok();
                    return;
                }
                if always
                    && let Some((pane, ix)) = this.find_view(&view)
                    && let Some(pane) = this.pane_mut(pane)
                {
                    pane.tabs[ix].remote_allowed = true;
                }
                let answer = run_in(this, &view, &command, source, cx);
                reply.send(answer).ok();
            })
        };
        let number = ix + 1;
        let buttons = vec![
            answer("allow", true, false),
            answer("allow for this tab", true, true),
            answer("deny", false, false),
        ];
        self.ask(
            format!("a program wants to run this in tab {number}"),
            Some(command),
            Some(view),
            buttons,
            cx,
        );
        let _ = window;
    }
}

/// type `command` and enter into the tab of `view`
fn run_in(
    this: &mut Workspace,
    view: &gpui::Entity<TerminalView>,
    command: &str,
    source: InputSource,
    cx: &mut Context<Workspace>,
) -> Reply {
    match this.find_view(view) {
        Some((pane, ix)) => {
            this.input_to(pane, ix, source, format!("{command}\r").into_bytes(), cx);
            Reply::done()
        }
        None => Reply::failed("the pane is gone"),
    }
}
