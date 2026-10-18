//! remote control requests aimed at one pane: typing, reading text and blocks, closing,
//! splitting, titles and colors, palette commands, and runs the user has to allow

use std::{cell::RefCell, rc::Rc, time::UNIX_EPOCH};

use futures::channel::oneshot;
use gpui::{App, Context, EntityId, Window};

use super::{RunAction, Workspace, notifications::NotificationButton};
use crate::{
    automation::{BlockInfo, Reply, Request},
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
    let blocks = terminal.blocks().iter().cloned().enumerate();
    let picked: Vec<_> = match number.unwrap_or(-1) {
        _ if all => blocks.collect(),
        0 => Vec::new(),
        n if n < 0 => {
            let mut finished = blocks.filter(|(_, block)| block.ended.is_some()).rev();
            finished
                .nth(n.unsigned_abs() as usize - 1)
                .into_iter()
                .collect()
        }
        n => blocks.skip(n as usize - 1).take(1).collect(),
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
        let answer = match request {
            Request::SendText { text, .. } => {
                if self.input_to(pane, ix, InputSource::Remote, text.into_bytes(), cx) {
                    Reply::done()
                } else {
                    Reply::failed("input was refused, a program asks for a password")
                }
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
                self.ask_to_run(pane, ix, command, source, reply, cx);
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
            .filter(|(name, _)| !inputs.contains_key(*name))
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
    fn ask_to_run(
        &mut self,
        pane: usize,
        ix: usize,
        command: String,
        source: InputSource,
        reply: oneshot::Sender<Reply>,
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
            if this.input_to(pane, ix, source, format!("{command}\r").into_bytes(), cx) {
                Reply::done()
            } else {
                Reply::failed("input was refused, a program asks for a password")
            }
        }
        None => Reply::failed("the pane is gone"),
    }
}

#[cfg(test)]
mod tests {

    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn remote_input_refused_at_a_password_prompt_is_no_success(cx: &mut TestAppContext) {
        use crate::automation::Request;
        use crate::ui::windows::handle_request;

        let (ws, cx) = open(cx, 1);
        cx.update(|window, _| window.activate_window());
        let shell = terminal(&ws, cx, 0);
        shell.update(cx, |t, _| {
            t.input(
                InputSource::Keyboard,
                b"python3 -c \"import getpass; getpass.getpass('P' 'w: ')\"\r".as_slice(),
            )
        });
        wait_until(cx, "the password prompt never showed", |cx| {
            shell.update(cx, |t, _| t.visible_screen_text(false).0.contains("Pw:"))
        });
        let (tx, mut rx) = futures::channel::oneshot::channel();
        let request = Request::SendText {
            text: "secret\r".into(),
            pane: None,
        };
        cx.cx.update(|cx| handle_request(request, tx, cx));
        let reply = rx.try_recv().unwrap().expect("no reply yet");
        assert!(!reply.ok, "{reply:?}");
    }

    #[gpui::test]
    fn remote_requests_open_tabs_type_and_list(cx: &mut TestAppContext) {
        use crate::automation::{Reply, Request};
        use crate::ui::windows::handle_request;

        // replies that need no answer from the user come right away
        fn ask(cx: &mut VisualTestContext, request: Request) -> Reply {
            let (tx, mut rx) = futures::channel::oneshot::channel();
            cx.cx.update(|cx| handle_request(request, tx, cx));
            rx.try_recv().unwrap().expect("no reply yet")
        }

        let (ws, cx) = open(cx, 1);
        cx.update(|window, _| window.activate_window());
        let reply = ask(
            cx,
            Request::NewTab {
                cwd: Some(std::env::temp_dir()),
                profile: None,
                name: Some("build".into()),
            },
        );
        assert!(reply.ok, "{reply:?}");
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 2);
        let reply = ask(
            cx,
            Request::NewTab {
                cwd: None,
                profile: Some("missing".into()),
                name: None,
            },
        );
        assert!(!reply.ok);

        let reply = ask(cx, Request::ListTabs);
        assert_eq!(reply.tabs.len(), 2);
        assert!(reply.tabs[1].active && !reply.tabs[0].active);
        assert_eq!(reply.tabs[1].number, 2);
        assert_eq!(reply.tabs[1].name.as_deref(), Some("build"));
        assert_ne!(reply.tabs[0].id, reply.tabs[1].id);
        let first = reply.tabs[0].id.to_string();

        // the named tab is found while another one is active
        ws.update_in(cx, |ws, window, cx| {
            ws.activate_tab(ws.focused, 0, window, cx)
        });
        let reply = ask(
            cx,
            Request::SendText {
                text: "echo remote_$((2*21))\r".into(),
                pane: Some("build".into()),
            },
        );
        assert!(reply.ok);
        let terminal = terminal(&ws, cx, 1);
        // the tab is in the background, nothing paints it, so its screen is read here
        wait_until(cx, "sent text never ran", |cx| {
            terminal.update(cx, |t, _| {
                t.sync();
                line_starting_with(t, "remote_42").is_some()
            })
        });
        let reply = ask(
            cx,
            Request::GetText {
                pane: Some("build".into()),
                scrollback: false,
            },
        );
        assert!(reply.text.unwrap().contains("remote_42"));

        // by id, and titles set from outside
        let reply = ask(
            cx,
            Request::SetTabTitle {
                pane: Some(first),
                title: "from outside".into(),
            },
        );
        assert!(reply.ok);
        let reply = ask(
            cx,
            Request::FocusTab {
                pane: Some("from outside".into()),
            },
        );
        assert!(reply.ok);
        assert_eq!(ws.read_with(cx, |ws, _| ws.focused_pane().active), 0);
        let reply = ask(
            cx,
            Request::FocusTab {
                pane: Some("nothing like it".into()),
            },
        );
        assert!(!reply.ok);
    }

    /// answer `request` about tab `ix` of the focused pane, none while it waits for the user
    fn ask_tab(
        ws: &Entity<Workspace>,
        cx: &mut VisualTestContext,
        ix: usize,
        request: crate::automation::Request,
    ) -> (
        Option<crate::automation::Reply>,
        futures::channel::oneshot::Receiver<crate::automation::Reply>,
    ) {
        let (tx, mut rx) = futures::channel::oneshot::channel();
        ws.update_in(cx, |ws, window, cx| {
            let id = ws.focused_pane().tabs[ix].view.entity_id();
            ws.remote(id, request, tx, window, cx)
        });
        cx.run_until_parked();
        (rx.try_recv().unwrap(), rx)
    }

    fn press(cx: &mut VisualTestContext, button: &str) {
        let at = cx
            .debug_bounds(&*button.to_string().leak())
            .unwrap()
            .center();
        click(cx, at, 1);
        release(cx, at);
        cx.run_until_parked();
    }

    #[gpui::test]
    fn remote_tab_colors_take_css_colors_and_clear(cx: &mut TestAppContext) {
        use crate::automation::Request;

        let (ws, cx) = open(cx, 1);
        let color = |ws: &Entity<Workspace>, cx: &mut VisualTestContext| {
            ws.update(cx, |ws, _| ws.focused_pane().tabs[0].color)
        };
        let set = |color: Option<&str>| Request::SetTabColor {
            pane: None,
            color: color.map(str::to_string),
        };
        let (reply, _) = ask_tab(&ws, cx, 0, set(Some("#ff8800")));
        assert!(reply.unwrap().ok);
        let orange: gpui::Hsla = gpui::rgb(0xff8800).into();
        assert_eq!(color(&ws, cx), Some(orange));
        // a bad color keeps the one there
        let (reply, _) = ask_tab(&ws, cx, 0, set(Some("orange-ish")));
        let reply = reply.unwrap();
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("is no color"));
        assert_eq!(color(&ws, cx), Some(orange));
        let (reply, _) = ask_tab(&ws, cx, 0, set(None));
        assert!(reply.unwrap().ok);
        assert_eq!(color(&ws, cx), None);
    }

    #[gpui::test]
    fn remote_queries_pick_blocks_by_number(cx: &mut TestAppContext) {
        with_own_state("remote_queries", || {
            use crate::automation::Request;

            let (ws, cx) = open(cx, 1);
            finished_block(&ws, cx, 0, "echo first_block");
            finished_block(&ws, cx, 0, "echo second_block");
            let query = |block: i64| Request::Query {
                pane: None,
                block: Some(block),
                all: false,
            };
            let commands = |ws: &Entity<Workspace>, cx: &mut VisualTestContext, block: i64| {
                let (reply, _) = ask_tab(ws, cx, 0, query(block));
                let reply = reply.unwrap();
                assert!(reply.ok, "{reply:?}");
                reply
                    .blocks
                    .into_iter()
                    .map(|block| (block.number, block.command))
                    .collect::<Vec<_>>()
            };
            // counted from the oldest block still in history
            assert_eq!(commands(&ws, cx, 2), [(2, "echo second_block".to_string())]);
            assert_eq!(commands(&ws, cx, 1), [(1, "echo first_block".to_string())]);
            // 0 names no block
            let (reply, _) = ask_tab(&ws, cx, 0, query(0));
            assert!(!reply.unwrap().ok);
        });
    }

    #[gpui::test]
    fn remote_runs_wait_for_the_user_unless_allowed_for_the_tab(cx: &mut TestAppContext) {
        use crate::automation::Request;

        let (ws, cx) = open(cx, 1);
        let run = |command: &str| Request::Run {
            pane: None,
            command: command.into(),
            client: None,
        };
        let terminal = terminal(&ws, cx, 0);
        let shows = |cx: &mut VisualTestContext, text: &str| {
            terminal.update(cx, |t, _| {
                t.sync();
                line_starting_with(t, text).is_some()
            })
        };

        // denied, nothing is typed
        let (reply, mut rx) = ask_tab(&ws, cx, 0, run("echo denied_$((1+1))"));
        assert!(reply.is_none(), "answered before the user did");
        assert_eq!(
            notifications(&ws, cx),
            ["a program wants to run this in tab 1 echo denied_$((1+1))"]
        );
        press(cx, "notification-0-button-2");
        let reply = rx.try_recv().unwrap().expect("no answer after deny");
        assert_eq!(reply.error.as_deref(), Some("the user denied the run"));

        // allowed for the tab: this run is typed, and the next one without asking
        let (reply, mut rx) = ask_tab(&ws, cx, 0, run("echo allowed_$((2+2))"));
        assert!(reply.is_none());
        press(cx, "notification-1-button-1");
        assert!(rx.try_recv().unwrap().expect("no answer after allow").ok);
        wait_until(cx, "allowed run never ran", |cx| shows(cx, "allowed_4"));
        let (reply, _) = ask_tab(&ws, cx, 0, run("echo again_$((3+3))"));
        assert!(reply.expect("asked again").ok);
        assert!(notifications(&ws, cx).is_empty());
        wait_until(cx, "second run never ran", |cx| shows(cx, "again_6"));
        assert!(!shows(cx, "denied_2"));
    }
}
