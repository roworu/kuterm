//! commands that ask for values before they run, and the matrix that runs one command once per
//! value, each in a split of a grid

use gpui::{AppContext, Context, Entity, WeakEntity, Window};

use super::{TerminalView, Workspace, notifications::NotificationButton, tab_cwd, tab_title};
use crate::{
    settings::{Command, Inputs, Side, Values},
    ui::command_palette::{CommandPalette, InputFilled, InputPrompt},
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

/// one value of a running matrix command and what its split ran
pub(super) struct MatrixRun {
    /// pane the value runs in
    pub(super) pane: usize,
    pub(super) value: String,
    /// true once the command in that pane ended
    pub(super) done: bool,
    pub(super) exit: Option<i32>,
}

/// the splits of a running matrix command
pub(super) struct Matrix {
    pub(super) runs: Vec<MatrixRun>,
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
        // matrix inputs are not asked for, every one of their values runs
        let next = pending
            .command
            .inputs
            .iter()
            .find(|(name, input)| !input.matrix && !pending.values.contains_key(*name));
        let Some((name, input)) = next else {
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
        let next = pending
            .command
            .inputs
            .iter()
            .find(|(name, input)| !input.matrix && !pending.values.contains_key(*name))
            .map(|(name, _)| name.clone());
        if let Some(name) = next {
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
            .map(|value| Command {
                name: value,
                category: None,
                pinned: false,
                inputs: Inputs::new(),
                actions: Vec::new(),
            })
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
                .background_spawn(async move { tab_title::run(&ran, cwd.as_deref()) })
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
                match values {
                    Some(values) if !values.is_empty() => {
                        this.open_value_picker(&name, values, window, cx)
                    }
                    Some(_) => {
                        this.pending = None;
                        this.show_notification(
                            format!("{command:?} printed nothing to pick"),
                            None,
                            cx,
                        );
                    }
                    None => {
                        this.pending = None;
                        this.show_notification(
                            format!("{command:?} took too long to pick from"),
                            None,
                            cx,
                        );
                    }
                }
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

    /// run a command with its inputs filled, once per value when an input is a matrix
    pub(super) fn start_command(
        &mut self,
        command: Command,
        values: Values,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut values = values;
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
        let matrix = command
            .matrix()
            .map(|(name, values)| (name.to_string(), values.to_vec()));
        let Some((name, matrix_values)) = matrix else {
            self.run_command_at(&command, &values, None, window, cx);
            return;
        };
        // the values land in a grid of splits, filled row by row
        let columns = (matrix_values.len() as f64).sqrt().ceil().max(1.) as usize;
        let mut panes = Vec::new();
        for (ix, value) in matrix_values.iter().enumerate() {
            let (of, side) = match ix {
                0 => (self.focused, Side::Right),
                // a new row starts by splitting the pane above it
                ix if ix % columns == 0 => (panes[ix - columns], Side::Down),
                ix => (panes[ix - 1], Side::Right),
            };
            let (pane, _) = self.split_new_tab(of, side, window, cx);
            panes.push(pane);
            let mut values = values.clone();
            values.insert(name.clone(), value.clone());
            self.run_command_at(&command, &values, Some((pane, 0)), window, cx);
        }
        self.matrix = Some(Matrix {
            runs: panes
                .into_iter()
                .zip(matrix_values)
                .map(|(pane, value)| MatrixRun {
                    pane,
                    value,
                    done: false,
                    exit: None,
                })
                .collect(),
        });
    }

    /// a command of the running matrix ended, the last one tells how the run went
    pub(super) fn matrix_finished(
        &mut self,
        view: &Entity<TerminalView>,
        exit: Option<i32>,
        cx: &mut Context<Self>,
    ) {
        let Some((pane, _)) = self.find_view(view) else {
            return;
        };
        let Some(matrix) = self.matrix.as_mut() else {
            return;
        };
        let Some(run) = matrix
            .runs
            .iter_mut()
            .find(|run| run.pane == pane && !run.done)
        else {
            return;
        };
        run.done = true;
        run.exit = exit;
        if self
            .matrix
            .as_ref()
            .is_some_and(|matrix| matrix.runs.iter().any(|run| !run.done))
        {
            return;
        }
        let matrix = self.matrix.take().expect("matrix is running");
        let passed = matrix.runs.iter().filter(|run| run.exit == Some(0)).count();
        let detail = matrix
            .runs
            .iter()
            .map(|run| {
                let result = match run.exit {
                    Some(0) => "ok".to_string(),
                    Some(exit) => format!("exit {exit}"),
                    None => "no shell integration".to_string(),
                };
                format!("{} {result}", run.value)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let panes: Vec<usize> = matrix.runs.iter().map(|run| run.pane).collect();
        let close_all = NotificationButton::new("close all", move |this, window, cx| {
            for pane in &panes {
                this.close_tab_at(*pane, 0, window, cx);
            }
        });
        self.ask(
            format!("{} of {} passed", passed, matrix.runs.len()),
            Some(detail),
            None,
            vec![close_all],
            cx,
        );
    }
}
