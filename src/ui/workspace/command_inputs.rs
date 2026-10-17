//! commands that ask for values before they run

use gpui::{AppContext, Context, Entity, WeakEntity, Window};

use super::{TerminalView, Workspace, tab_cwd};
use crate::{
    settings::{Command, Input, Values},
    ui::dialogs::{CommandPalette, InputFilled, InputPrompt},
};

/// name the picked hint is filled in under, it comes from the tab rather than from an input
const HINT: &str = "hint";

/// a command picked in the palette whose inputs are still being asked for
pub(super) struct PendingCommand {
    pub(super) command: Command,
    /// values filled so far, by input name
    pub(super) values: Values,
    /// tab whose hint the command waits for, set once hints mode is started for it
    pub(super) hint: Option<(usize, usize)>,
}

impl PendingCommand {
    /// name of the first input still to ask for
    fn next_input(&self) -> Option<(&String, &Input)> {
        self.command
            .inputs
            .iter()
            .find(|(name, _)| !self.values.contains_key(*name))
    }
}

impl Workspace {
    /// run a command picked in the palette or by a key, asking for its inputs first
    pub(super) fn pick_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = PendingCommand {
            command,
            values: Values::new(),
            hint: None,
        };
        self.ask_input(pending, window, cx);
    }

    /// ask for the first input still empty, or run the command once all are filled
    fn ask_input(&mut self, pending: PendingCommand, window: &mut Window, cx: &mut Context<Self>) {
        let Some((name, input)) = pending.next_input() else {
            let PendingCommand {
                command, values, ..
            } = pending;
            self.pending = None;
            self.start_command(command, values, window, cx);
            return;
        };
        let (name, input) = (name.clone(), input.clone());
        self.pending = Some(pending);
        if let Some(values) = input.pick {
            self.open_value_picker(&name, values, window, cx);
        } else if let Some(hint) = input.text {
            let prompt = cx.new(|cx| InputPrompt::new(&name, &hint, cx));
            self.open_overlay(&prompt, window, cx);
            self.command_input = Some(prompt.clone());
            self._overlay_subscriptions.push(cx.subscribe_in(
                &prompt,
                window,
                |this, _, filled: &InputFilled, window, cx| {
                    let value = filled.0.clone();
                    this.close_overlay(window, cx);
                    this.input_filled(value, window, cx);
                },
            ));
        } else if let Some(command) = input.pick_from {
            self.pick_from(name, command, cx);
        }
    }

    /// the value typed or picked for the input being asked for, then the next one
    fn input_filled(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        if let Some(name) = pending.next_input().map(|(name, _)| name.clone()) {
            pending.values.insert(name, value);
        }
        self.ask_input(pending, window, cx);
    }

    /// list `values` in the palette, the picked one fills the input
    fn open_value_picker(
        &mut self,
        name: &str,
        values: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = values
            .into_iter()
            .map(|value| Command::new(value, None, Vec::new()))
            .collect();
        let palette = cx.new(|cx| CommandPalette::picker(items, format!("pick {name}..."), cx));
        self.open_overlay(&palette, window, cx);
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            |this, _, picked: &Command, window, cx| {
                let value = picked.name.clone();
                this.close_overlay(window, cx);
                this.input_filled(value, window, cx);
            },
        ));
        self.palette = Some(palette);
    }

    /// read the lines "pick_from" lists in the folder of the active tab, off the ui thread
    fn pick_from(&mut self, name: String, command: String, cx: &mut Context<Self>) {
        let cwd = self
            .pane(self.focused)
            .and_then(|pane| pane.tabs.get(pane.active))
            .and_then(|tab| tab_cwd(tab, cx));
        let ran = command.clone();
        cx.spawn(async move |this: WeakEntity<Workspace>, cx| {
            let output = cx
                .background_spawn(async move { super::title_sources::run(&ran, cwd.as_deref()) })
                .await;
            this.update_in(cx, |this, window, cx| {
                let values: Option<Vec<String>> = output.map(|output| {
                    output
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(str::to_string)
                        .collect()
                });
                let why = match values {
                    Some(values) if !values.is_empty() => {
                        return this.open_value_picker(&name, values, window, cx);
                    }
                    Some(_) => "printed nothing to pick",
                    None => "took too long to pick from",
                };
                this.pending = None;
                this.show_notification(format!("{command:?} {why}"), None, cx);
            })
            .ok();
        })
        .detach();
    }

    /// a hint picked in a tab, filling a command that waits for one and running it
    pub(super) fn hint_picked(
        &mut self,
        view: &Entity<TerminalView>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if pending.hint != self.find_view(view) {
            self.pending = Some(pending);
            return;
        }
        let PendingCommand {
            command,
            mut values,
            ..
        } = pending;
        values.insert(HINT.to_string(), text);
        self.start_command(command, values, window, cx);
    }

    /// run a command with its inputs filled
    pub(super) fn start_command(
        &mut self,
        command: Command,
        mut values: Values,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // a `{hint}` with nothing picked starts hints mode, the pick runs the command
        if command.uses_hint() && !values.contains_key(HINT) {
            let (pane, ix) = (self.focused, self.focused_pane().active);
            let view = self.view_at(pane, ix);
            match view.read(cx).picked_hint().map(str::to_string) {
                Some(hint) => {
                    values.insert(HINT.to_string(), hint);
                }
                None => {
                    view.update(cx, |view, cx| view.show_hints(cx));
                    self.pending = Some(PendingCommand {
                        command,
                        values,
                        hint: Some((pane, ix)),
                    });
                    return;
                }
            }
        }
        self.run_command_at(&command, &values, None, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::workspace::test_support::*;

    /// a workspace whose active tab shows the one hint match "zz5", labelled "a"
    fn open_with_match(
        cx: &mut TestAppContext,
        tabs: usize,
    ) -> (Entity<Workspace>, &mut VisualTestContext) {
        let json = r#"{"notifications": {"timeout": 0, "copy": false}, "hints": {"alphabet": "ab",
            "patterns": [{"name": "tag", "regex": "zz[0-9]", "action": "copy"}]}}"#;
        let (ws, cx) = open_with_settings(cx, tabs, "{}", Settings::parse(json).unwrap());
        print_line(&ws, cx, "zz5");
        (ws, cx)
    }

    fn hint_command() -> Command {
        let json = r#"{"commands": [{"name": "x", "actions": [{"notify": "got {hint:raw}"}]}]}"#;
        Commands::parse(json).unwrap().commands.remove(0)
    }

    fn pick(ws: &Entity<Workspace>, cx: &mut VisualTestContext) {
        ws.update_in(cx, |ws, window, cx| {
            ws.pick_command(hint_command(), window, cx)
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_hint_picked_before_fills_the_command_at_once(cx: &mut TestAppContext) {
        let (ws, cx) = open_with_match(cx, 1);
        let view = active_view(&ws, cx);
        view.update(cx, |view, cx| view.show_hints(cx));
        type_keys(cx, "a");
        assert_eq!(
            view.read_with(cx, |view, _| view.picked_hint().map(str::to_string)),
            Some("zz5".into())
        );
        pick(&ws, cx);
        assert_eq!(notifications(&ws, cx), ["got zz5"]);
        assert!(view.read_with(cx, |view, _| view.hints().is_none()));
        assert!(ws.update(cx, |ws, _| ws.pending.is_none()));
    }

    #[gpui::test]
    fn a_hint_picked_in_another_tab_does_not_fill_the_command(cx: &mut TestAppContext) {
        let (ws, cx) = open_with_match(cx, 2);
        pick(&ws, cx);
        // hints mode started in the active tab, the command waits for its pick
        assert!(active_view(&ws, cx).read_with(cx, |view, _| view.hints().is_some()));
        let other = ws.update(cx, |ws, _| ws.focused_pane().tabs[0].view.clone());
        ws.update_in(cx, |ws, window, cx| {
            ws.hint_picked(&other, "zz0".into(), window, cx)
        });
        cx.run_until_parked();
        assert!(notifications(&ws, cx).is_empty());
        assert!(ws.update(cx, |ws, _| ws.pending.is_some()));
        type_keys(cx, "a");
        assert_eq!(notifications(&ws, cx), ["got zz5"]);
    }
}
