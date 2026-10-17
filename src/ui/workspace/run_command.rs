//! running palette commands: each step on its target tab, pane, split or window, with its
//! placeholders filled

use super::*;

/// where one step of a command acts
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum StepTarget {
    /// one tab of one pane
    Tab(usize, usize),
    /// the active tab of every pane of the window
    AllPanes,
    /// a window of its own
    NewWindow,
}

impl Workspace {
    /// run command actions in order against the active tab, or the last background tab
    pub(super) fn run_command(
        &mut self,
        command: &Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_command_at(command, &Values::new(), None, window, cx);
    }

    /// run command actions with their `{name}` placeholders filled from `values`, starting at
    /// (`start`) or the active tab of the focused pane
    pub(super) fn run_command_at(
        &mut self,
        command: &Command,
        values: &Values,
        start: Option<(usize, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // tab later actions act on, the active tab of the focused pane when none
        let mut target: Option<(usize, usize)> = start;
        for step in &command.actions {
            // closing the last tab quits, nothing is left to act on
            if self.layout.first_pane_id().is_none() {
                return;
            }
            // a closed background tab falls back to the active tab of the focused pane
            let current = target
                .filter(|(pane, ix)| self.pane(*pane).is_some_and(|pane| *ix < pane.tabs.len()));
            let Some(at) = self.step_target(step, current, window, cx) else {
                return;
            };
            let (pane, ix) = match at {
                StepTarget::Tab(pane, ix) => (pane, ix),
                // these two do not act on one tab, the step runs on its own
                StepTarget::AllPanes => {
                    self.run_in_every_pane(&step.action, values, cx);
                    target = current;
                    continue;
                }
                StepTarget::NewWindow => {
                    self.run_in_new_window(&step.action, values, cx);
                    target = current;
                    continue;
                }
            };
            // a step waiting for a picked hint stops the command, the pick resumes it
            let action = self.fill_step(&step.action, values, pane, ix, cx);
            let action = &action;
            match action {
                CommandAction::About => self.open_about(&OpenAbout, window, cx),
                CommandAction::ReloadConfig => self.reload_config(window, cx),
                CommandAction::NewTab => {
                    self.add_tab(window, cx);
                    target = None;
                }
                CommandAction::NewIncognitoTab => {
                    let mut profile = Settings::get(cx).default_profile().clone();
                    profile.incognito = true;
                    self.add_profile_tab(self.focused, &profile, true, window, cx);
                    target = None;
                }
                CommandAction::NewTabWithProfile(name) => {
                    if self.add_named_profile_tab(self.focused, name, true, window, cx) {
                        target = None;
                    }
                }
                CommandAction::NewBackgroundTab => {
                    let profile = Settings::get(cx).default_profile().clone();
                    self.add_profile_tab(self.focused, &profile, false, window, cx);
                    target = self.last_tab_of_focused();
                }
                CommandAction::NewBackgroundTabWithProfile(name) => {
                    if self.add_named_profile_tab(self.focused, name, false, window, cx) {
                        target = self.last_tab_of_focused();
                    }
                }
                CommandAction::CloseTab => {
                    self.request_close_tab(pane, ix, window, cx);
                    target = None;
                }
                CommandAction::CloseTabsToRight => {
                    self.request_close_tabs_side(pane, ix, true, window, cx);
                    target = None;
                }
                CommandAction::CloseTabsToLeft => {
                    self.request_close_tabs_side(pane, ix, false, window, cx);
                    target = None;
                }
                CommandAction::CloseOtherTabs => {
                    self.request_close_others(pane, ix, window, cx);
                    target = None;
                }
                CommandAction::RenameTab => self.start_rename(pane, ix, window, cx),
                CommandAction::PinTab => self.toggle_pin(pane, ix, cx),
                CommandAction::DuplicateTab => {
                    self.duplicate_tab(pane, ix, window, cx);
                    target = None;
                }
                CommandAction::MoveTabToNewWindow => {
                    self.move_tab_to_new_window(pane, ix, None, window, cx);
                    // this window may be gone with its last tab
                    return;
                }
                CommandAction::CopyFolder => {
                    let tab = &self.pane(pane).expect("pane of target").tabs[ix];
                    match tab_cwd(tab, cx) {
                        Some(cwd) => {
                            let text = cwd.display().to_string();
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            self.notify_copied(&text, cx);
                        }
                        None => self.show_notification("the tab's folder is unknown", None, cx),
                    }
                }
                CommandAction::SplitLeft
                | CommandAction::SplitRight
                | CommandAction::SplitUp
                | CommandAction::SplitDown => {
                    let side = match action {
                        CommandAction::SplitLeft => DropSide::Left,
                        CommandAction::SplitRight => DropSide::Right,
                        CommandAction::SplitUp => DropSide::Top,
                        _ => DropSide::Bottom,
                    };
                    self.split_with_tab(pane, ix, pane, side, window, cx);
                    target = None;
                }
                CommandAction::ClosePane => {
                    self.request_close_pane(window, cx);
                    target = None;
                }
                CommandAction::FocusPaneLeft => self.focus_pane_toward(DropSide::Left, window, cx),
                CommandAction::FocusPaneRight => {
                    self.focus_pane_toward(DropSide::Right, window, cx)
                }
                CommandAction::FocusPaneUp => self.focus_pane_toward(DropSide::Top, window, cx),
                CommandAction::FocusPaneDown => {
                    self.focus_pane_toward(DropSide::Bottom, window, cx)
                }
                CommandAction::ResizePaneLeft => self.resize_pane(DropSide::Left, cx),
                CommandAction::ResizePaneRight => self.resize_pane(DropSide::Right, cx),
                CommandAction::ResizePaneUp => self.resize_pane(DropSide::Top, cx),
                CommandAction::ResizePaneDown => self.resize_pane(DropSide::Bottom, cx),
                CommandAction::ZoomPane => {
                    let single = self.layout.pane_ids().len() == 1;
                    self.zoomed = match self.zoomed {
                        Some(_) => None,
                        None if single => None,
                        None => Some(self.focused),
                    };
                }
                CommandAction::EqualizePanes => self.layout.equalize(),
                CommandAction::NextTab => {
                    self.next_tab(&NextTab, window, cx);
                    target = None;
                }
                CommandAction::PrevTab => {
                    let count = self.focused_pane().tabs.len();
                    let ix = (self.focused_pane().active + count - 1) % count;
                    self.activate_tab(self.focused, ix, window, cx);
                    target = None;
                }
                CommandAction::ActivateTab(number) => {
                    let count = self.focused_pane().tabs.len();
                    if let Some(ix) = number.checked_sub(1)
                        && ix < count
                    {
                        self.activate_tab(self.focused, ix, window, cx);
                        target = None;
                    }
                }
                CommandAction::PickTab => self.open_tab_picker(window, cx),
                CommandAction::NextAttentionTab => {
                    self.next_attention_tab(window, cx);
                    target = None;
                }
                CommandAction::PickAgentTab => self.open_agent_picker(window, cx),
                CommandAction::FindInAllTabs => self.open_finder(window, cx),
                CommandAction::PickTheme => self.open_theme_picker(window, cx),
                CommandAction::RenamePane => self.start_rename_pane(window, cx),
                CommandAction::ShowHiddenText => {
                    let shown = cx.default_global::<ShowHidden>();
                    shown.0 = !shown.0;
                    window.refresh();
                }
                CommandAction::NewGroup
                | CommandAction::AddTabToGroup
                | CommandAction::RemoveTabFromGroup
                | CommandAction::CloseGroup => self.run_group_action(action, window, cx),
                CommandAction::ToggleBroadcastAll => self.toggle_broadcast_all(cx),
                CommandAction::ToggleBroadcastPane => self.toggle_broadcast_pane(cx),
                CommandAction::ToggleScrollSync => self.toggle_scroll_sync(cx),
                CommandAction::QueueCommand => self.open_queue_input(None, None, window, cx),
                CommandAction::EditQueue => self.open_queue_editor(window, cx),
                CommandAction::ResumeQueue => self.resume_queue(cx),
                CommandAction::Copy => {
                    let selection = copied_text(self.terminal_at(pane, ix, cx).read(cx), cx);
                    match selection {
                        Some(text) => {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            self.notify_copied(&text, cx);
                        }
                        None => self.show_notification("nothing selected to copy", None, cx),
                    }
                }
                CommandAction::Paste => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.view_at(pane, ix)
                            .update(cx, |view, cx| view.paste_text(text, cx));
                    }
                }
                CommandAction::Search => self
                    .view_at(pane, ix)
                    .update(cx, |view, cx| view.open_search(window, cx)),
                CommandAction::Hints => self
                    .view_at(pane, ix)
                    .update(cx, |view, cx| view.show_hints(cx)),
                CommandAction::ViMode => self
                    .view_at(pane, ix)
                    .update(cx, |view, cx| view.toggle_vi_mode(cx)),
                CommandAction::JumpToPrevPrompt | CommandAction::JumpToNextPrompt => {
                    let up = *action == CommandAction::JumpToPrevPrompt;
                    self.terminal_at(pane, ix, cx)
                        .update(cx, |terminal, _| terminal.jump_to_prompt(up));
                }
                CommandAction::SelectLastOutput => {
                    let found = self
                        .terminal_at(pane, ix, cx)
                        .update(cx, |terminal, _| terminal.select_last_output());
                    if !found {
                        self.show_notification(
                            "no command output, needs shell integration",
                            None,
                            cx,
                        );
                    }
                }
                CommandAction::CopyLastOutput => {
                    match self.terminal_at(pane, ix, cx).read(cx).last_output_text() {
                        Some(text) => {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            self.notify_copied(&text, cx);
                        }
                        None => self.show_notification(
                            "no command output, needs shell integration",
                            None,
                            cx,
                        ),
                    }
                }
                CommandAction::OpenScrollback => {
                    let text = self.terminal_at(pane, ix, cx).read(cx).scrollback_ansi();
                    self.open_scrollback(text, window, cx);
                    target = None;
                }
                CommandAction::ScrollUp(lines) | CommandAction::ScrollDown(lines) => {
                    let lines = match action {
                        CommandAction::ScrollUp(_) => *lines,
                        _ => -*lines,
                    };
                    self.terminal_at(pane, ix, cx)
                        .update(cx, |terminal, _| terminal.scroll(lines));
                    let view = self.view_at(pane, ix);
                    self.sync_scroll(&view, lines, cx);
                }
                CommandAction::ScrollTop => {
                    self.terminal_at(pane, ix, cx).update(cx, |terminal, _| {
                        terminal.scroll_to(terminal.history_size())
                    })
                }
                CommandAction::ScrollBottom => self
                    .terminal_at(pane, ix, cx)
                    .update(cx, |terminal, _| terminal.scroll_to(0)),
                CommandAction::Quit => self.request_quit(window, cx),
                CommandAction::SelectAll => self
                    .terminal_at(pane, ix, cx)
                    .update(cx, |terminal, _| terminal.select_all()),
                CommandAction::Clear => self
                    .terminal_at(pane, ix, cx)
                    .update(cx, |terminal, _| terminal.clear()),
                CommandAction::IncreaseFontSize => self.change_font_size(pane, ix, Some(1.), cx),
                CommandAction::DecreaseFontSize => self.change_font_size(pane, ix, Some(-1.), cx),
                CommandAction::ResetFontSize => self.change_font_size(pane, ix, None, cx),
                CommandAction::OpenSettings => self.open_settings(cx),
                CommandAction::Type(text) => {
                    self.input_to(pane, ix, InputSource::Macro, text.clone().into_bytes(), cx)
                }
                CommandAction::SendText(text) => {
                    let bytes = text.clone().into_bytes();
                    let source = InputSource::Macro;
                    if !self
                        .terminal_at(pane, ix, cx)
                        .update(cx, |terminal, _| terminal.input(source, bytes))
                    {
                        self.input_refused(source, cx);
                    }
                }
                CommandAction::Notify(text) => self.show_notification(text.clone(), None, cx),
                CommandAction::NotifyWhenDone(text) => {
                    self.notify_when_done(self.view_at(pane, ix), text.clone(), cx);
                }
            }
        }
        // scrolling and pasting change the grid without a wakeup, and terminal views are
        // cached, so the active tab is redrawn through its own view
        if let Some(pane) = self.pane(self.focused)
            && let Some(tab) = pane.tabs.get(pane.active)
        {
            tab.view.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// the tab `step` acts on: its own target when it names one, else the command's current tab
    pub(super) fn step_target(
        &mut self,
        step: &CommandStep,
        current: Option<(usize, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<StepTarget> {
        match &step.target {
            None => {
                let (pane, ix) = current.unwrap_or_else(|| {
                    let pane = self.focused;
                    (pane, self.focused_pane().active)
                });
                Some(StepTarget::Tab(pane, ix))
            }
            Some(Target::AllPanes) => Some(StepTarget::AllPanes),
            Some(Target::NewWindow) => Some(StepTarget::NewWindow),
            Some(Target::NewSplit(side)) => {
                let (pane, tab) = self.split_new_tab(self.focused, *side, window, cx);
                Some(StepTarget::Tab(pane, tab))
            }
            Some(Target::Pane(spec)) => {
                let found = [PaneMatch::Name, PaneMatch::Title, PaneMatch::Id]
                    .into_iter()
                    .find_map(|by| self.find_pane(spec, by, cx))
                    .and_then(|id| self.find_id(id));
                match found {
                    Some(at) => Some(StepTarget::Tab(at.0, at.1)),
                    None => {
                        self.show_notification(format!("no pane {spec:?}"), None, cx);
                        None
                    }
                }
            }
        }
    }

    /// write a text step into the active tab of every pane, like broadcast input does
    pub(super) fn run_in_every_pane(
        &mut self,
        action: &CommandAction,
        values: &Values,
        cx: &mut Context<Self>,
    ) {
        let typed = match action {
            CommandAction::Type(_) => true,
            CommandAction::SendText(_) => false,
            _ => {
                self.show_notification(
                    "\"all_panes_in_tab\" only writes \"type\" and \"send_text\" steps",
                    None,
                    cx,
                );
                return;
            }
        };
        for pane in self.layout.pane_ids() {
            let Some(ix) = self.pane(pane).map(|pane| pane.active) else {
                continue;
            };
            let text = match self.fill_step(action, values, pane, ix, cx) {
                CommandAction::Type(text) | CommandAction::SendText(text) => text,
                _ => unreachable!("checked above"),
            };
            if typed {
                self.input_to(pane, ix, InputSource::Macro, text.into_bytes(), cx);
            } else {
                let bytes = text.into_bytes();
                let source = InputSource::Macro;
                if !self
                    .terminal_at(pane, ix, cx)
                    .update(cx, |terminal, _| terminal.input(source, bytes))
                {
                    self.input_refused(source, cx);
                }
            }
        }
    }

    /// run a step in a window of its own
    pub(super) fn run_in_new_window(
        &mut self,
        action: &CommandAction,
        values: &Values,
        cx: &mut Context<Self>,
    ) {
        let command = Command::new("", None, vec![CommandStep::new(action.clone())]);
        let Some(handle) = crate::ui::windows::open_workspace_window(None, cx) else {
            self.show_notification("could not open a window", None, cx);
            return;
        };
        handle
            .update(cx, |workspace, window, cx| {
                workspace.run_command_at(&command, values, None, window, cx);
            })
            .ok();
    }

    /// a copy of `action` with its `{name}` placeholders filled in for the tab at (`pane`, `ix`),
    /// picking the built in values from that tab
    pub(super) fn fill_step(
        &self,
        action: &CommandAction,
        values: &Values,
        pane: usize,
        ix: usize,
        cx: &App,
    ) -> CommandAction {
        let Some(text) = action.text() else {
            return action.clone();
        };
        let tab = &self.pane(pane).expect("pane of target").tabs[ix];
        let hint = self
            .view_at(pane, ix)
            .read(cx)
            .picked_hint()
            .map(str::to_string);
        let filled = crate::settings::fill(text, |name| {
            if let Some(value) = values.get(name) {
                return Some(value.clone());
            }
            match name {
                "selection" => copied_text(self.terminal_at(pane, ix, cx).read(cx), cx),
                "last_output" => self.terminal_at(pane, ix, cx).read(cx).last_output_text(),
                "cwd" => tab_cwd(tab, cx).map(|cwd| cwd.display().to_string()),
                "clipboard" => cx.read_from_clipboard().and_then(|item| item.text()),
                "hint" => hint.clone(),
                _ => None,
            }
        });
        action.with_text(filled)
    }

    pub(super) fn run_action(
        &mut self,
        action: &RunAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = Command::new("", None, vec![action.0.clone().into()]);
        self.run_command(&command, window, cx);
    }

    /// run the command a key is bound to, looked up by name so reloads apply
    pub(super) fn run_named_command(
        &mut self,
        action: &RunNamedCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match Commands::get(cx).find(&action.0) {
            Some(command) => {
                let command = command.clone();
                self.record_recent(command.label());
                self.pick_command(command, window, cx);
            }
            // the name is checked when the config loads, so this is an old key binding
            None => self.show_error(format!("no command named {:?}", action.0), cx),
        }
    }

    /// run a picked menu item from the terminal, so it does what its keys do
    pub fn run_menu_action(
        &mut self,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_active_tab(window, cx);
        window.dispatch_action(action, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn font_size_actions_change_only_the_active_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let settings_size = cx.update(|_, cx| Settings::get(cx).terminal.font.size);
        let sizes = |cx: &mut VisualTestContext| -> Vec<f32> {
            ws.update(cx, |ws, cx| {
                ws.focused_pane()
                    .tabs
                    .iter()
                    .map(|tab| tab.view.read(cx).font_size(cx))
                    .collect()
            })
        };

        run_actions(&ws, cx, r#""increase_font_size", "increase_font_size""#);
        assert_eq!(sizes(cx), vec![settings_size, settings_size + 2.]);
        run_actions(&ws, cx, r#""decrease_font_size""#);
        assert_eq!(sizes(cx), vec![settings_size, settings_size + 1.]);
        assert_eq!(
            cx.update(|_, cx| Settings::get(cx).terminal.font.size),
            settings_size
        );

        run_actions(&ws, cx, r#""reset_font_size""#);
        assert_eq!(sizes(cx), vec![settings_size, settings_size]);
    }

    #[gpui::test]
    fn custom_command_opens_a_tab_and_types_into_it(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [{"name": "hello", "actions":
                ["new_tab", {"type": "printf 'from_%s\\n' palette\n"}]}]}"#,
        );
        type_keys(cx, "ctrl-shift-p h e l l o enter");
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 1);
        let terminal = terminal(&ws, cx, 1);
        wait_until(cx, "typed command never ran", |cx| {
            terminal.read_with(cx, |t, _| line_starting_with(t, "from_palette").is_some())
        });
    }

    #[gpui::test]
    fn actions_after_close_tab_use_the_remaining_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let command = Commands::parse(
            r#"{"commands": [{"name": "x", "actions": ["close_tab", "next_tab", {"type": "a"}]}]}"#,
        )
        .unwrap()
        .commands
        .remove(0);
        ws.update_in(cx, |ws, window, cx| ws.run_command(&command, window, cx));
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 1);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn nothing_runs_after_a_step_closed_the_last_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        run_actions(&ws, cx, r#""close_tab", "new_tab""#);
        assert!(quitting(&ws, cx));
        assert!(ws.update(cx, |ws, _| ws.layout.first_pane_id().is_none()));
    }

    #[gpui::test]
    fn palette_actions_report_in_notifications(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("workspace_report");
        // reloading reads the config folder, so it is a temp one
        crate::settings::tests::with_config_home(&dir, || {
            let (ws, cx) = open(cx, 1);
            run_actions(&ws, cx, r#""reload_config", "copy""#);
            assert_eq!(
                notifications(&ws, cx),
                ["config reloaded", "nothing selected to copy"]
            );
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn background_tab_runs_command_and_notifies_when_done(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"notifications": {"timeout": 0}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        let file = std::env::temp_dir().join(format!("kuterm_bg_done_{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let actions = format!(
            r#""new_background_tab", {{"type": "sleep 0.5; touch {}\n"}}, {{"notify_when_done": "bg done"}}"#,
            file.display()
        );
        run_actions(&ws, cx, &actions);

        // the new tab opened behind the active one, which keeps focus
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 0);
        wait_until(cx, "background command never finished", |cx| {
            !notifications(&ws, cx).is_empty()
        });
        assert!(file.exists(), "notified before the command finished");
        assert_eq!(notifications(&ws, cx), ["bg done"]);

        // clicking the notification switches to the tab that finished
        let center = cx.debug_bounds("notification-0").unwrap().center();
        click(cx, center, 1);
        release(cx, center);
        cx.run_until_parked();
        assert_eq!(current(&ws, cx), 1);
        assert!(notifications(&ws, cx).is_empty());
        let _ = std::fs::remove_file(&file);
    }

    #[gpui::test]
    fn copy_and_paste_commands(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        print_line(&ws, cx, "pick these words");
        let start = cell_of(&ws, cx, "pick these words", 5, false);
        let end = cell_of(&ws, cx, "pick these words", 9, true);
        click(cx, start, 1);
        cx.simulate_event(MouseMoveEvent {
            position: end,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        });
        release(cx, end);
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
        run_actions(&ws, cx, r#""copy""#);
        assert_eq!(clipboard(cx).as_deref(), Some("these"));

        // paste sends the clipboard to the shell, read it back as raw bytes
        let r = reader(&ws, cx, false, "paste_command");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("hi".into()));
        run_actions(&ws, cx, r#""paste""#);
        assert_eq!(finish(&ws, cx, r), ["68", "69"]);
    }

    #[gpui::test]
    fn scroll_commands_move_the_view(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let terminal = terminal(&ws, cx, 0);
        send_to(&ws, cx, 0, "seq 1 300; echo DONE\r");
        wait_until(cx, "output never finished", |cx| {
            terminal.read_with(cx, |t, _| line_starting_with(t, "DONE").is_some())
        });
        let top = terminal.read_with(cx, |t, _| t.history_size());
        run_actions(&ws, cx, r#""scroll_top""#);
        assert_eq!(scroll_offset(&terminal, cx), top);
        run_actions(&ws, cx, r#"{"scroll_down": 10}"#);
        assert_eq!(scroll_offset(&terminal, cx), top - 10);
        run_actions(&ws, cx, r#"{"scroll_up": 5}"#);
        assert_eq!(scroll_offset(&terminal, cx), top - 5);
        run_actions(&ws, cx, r#""scroll_bottom""#);
        assert_eq!(scroll_offset(&terminal, cx), 0);
    }

    #[gpui::test]
    fn keymap_sends_text_chords_and_unbinds(cx: &mut TestAppContext) {
        let keys = r#"{"keymap": [
            {"keys": "ctrl-a c", "action": "new_tab"},
            {"keys": "ctrl-shift-k", "action": {"send_text": "sent"}, "when": "terminal"},
            {"keys": "ctrl-shift-w", "action": "unbind"},
            {"keys": "ctrl-shift-z", "action": "new_tab", "when": "vi_mode"},
        ]}"#;
        let full = crate::settings::config_combine(serde_json_lenient::from_str(keys).unwrap());
        let keymap = Keybindings::from_full(&full).unwrap();
        let (ws, cx) = open(cx, 1);
        cx.update(|_, cx| cx.bind_keys(keymap.bindings()));
        // a chord: ctrl-a, then c
        cx.simulate_keystrokes("ctrl-a c");
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 2);
        // unbound, so the tab stays
        cx.simulate_keystrokes("ctrl-shift-w");
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 2);
        // only in vi mode
        cx.simulate_keystrokes("ctrl-shift-z");
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 2);
        run_actions(&ws, cx, r#""vi_mode""#);
        cx.simulate_keystrokes("ctrl-shift-z");
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 3);

        let reading = reader(&ws, cx, false, "keymap_send");
        cx.simulate_keystrokes("ctrl-shift-k");
        pump(cx);
        let got = finish(&ws, cx, reading).concat();
        assert!(got.starts_with("73656e74"), "{got}");
    }

    #[gpui::test]
    fn keymap_run_runs_the_command(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let full = crate::settings::config_combine(
            serde_json_lenient::from_str(
                r#"{"commands": [{"name": "bound", "actions": [{"type": "echo bound"}]}],
                    "keymap": [{"keys": "ctrl-shift-j", "action": {"run": "bound"}}]}"#,
            )
            .unwrap(),
        );
        let commands = Commands::from_full(&full).unwrap();
        let keymap = Keybindings::from_full(&full).unwrap();
        cx.update(|_, cx| {
            cx.set_global(commands);
            cx.bind_keys(keymap.bindings());
        });
        let reading = reader(&ws, cx, false, "keymap_run");
        cx.simulate_keystrokes("ctrl-shift-j");
        pump(cx);
        let got = finish(&ws, cx, reading).concat();
        assert!(got.starts_with("6563686f"), "{got}");
    }

    #[gpui::test]
    fn show_hidden_text_flips_for_every_window(cx: &mut TestAppContext) {
        use crate::ui::terminal_element::ShowHidden;

        let (ws, cx) = open(cx, 1);
        let shown = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| cx.try_global::<ShowHidden>().is_some_and(|shown| shown.0))
        };
        assert!(!shown(cx));
        run_actions(&ws, cx, r#""show_hidden_text""#);
        assert!(shown(cx));
        run_actions(&ws, cx, r#""show_hidden_text""#);
        assert!(!shown(cx));
    }

    #[gpui::test]
    fn output_commands_say_when_no_command_ran(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into()));
        run_actions(&ws, cx, r#""select_last_output", "copy_last_output""#);
        let missing = "no command output, needs shell integration";
        assert_eq!(notifications(&ws, cx), [missing, missing]);
        assert_eq!(clipboard(cx).as_deref(), Some("before"));
    }

    #[gpui::test]
    fn clear_drops_the_history_and_keeps_the_prompt(cx: &mut TestAppContext) {
        with_own_state("clear_history", || {
            let (ws, cx) = open(cx, 1);
            let terminal = terminal(&ws, cx, 0);
            finished_block(&ws, cx, 0, "seq 1 200");
            assert!(terminal.read_with(cx, |t, _| t.history_size()) > 0);
            run_actions(&ws, cx, r#""clear""#);
            assert_eq!(terminal.read_with(cx, |t, _| t.history_size()), 0);
            assert!(!terminal.update(cx, |t, _| {
                t.visible_screen_text(false).0.contains("\n200\n")
            }));
        });
    }

    #[gpui::test]
    fn prompt_jumps_scroll_to_the_prompts_above_and_below(cx: &mut TestAppContext) {
        with_own_state("prompt_jumps", || {
            let (ws, cx) = open(cx, 1);
            let terminal = terminal(&ws, cx, 0);
            // zsh redraws its prompt when the first layout resizes the pty, which drops the
            // prompt's mark, so the prompts jumped between come after that
            finished_block(&ws, cx, 0, "echo warm");
            finished_block(&ws, cx, 0, "seq 1 100");
            finished_block(&ws, cx, 0, "seq 1 101");
            // the last prompt may still be on its way, and new lines move a scrolled view
            let mut last = (usize::MAX, 0);
            wait_until(cx, "the screen never settled", |cx| {
                let now = terminal.update(cx, |t, _| {
                    t.sync();
                    (t.history_size(), t.visible_screen_text(false).0.len())
                });
                std::mem::replace(&mut last, now) == now
            });
            run_actions(&ws, cx, r#""jump_to_prev_prompt""#);
            let first = scroll_offset(&terminal, cx);
            assert!(first > 0, "no prompt above");
            run_actions(&ws, cx, r#""jump_to_prev_prompt""#);
            let second = scroll_offset(&terminal, cx);
            assert!(second > first, "{first} {second}");
            run_actions(&ws, cx, r#""jump_to_next_prompt""#);
            assert_eq!(scroll_offset(&terminal, cx), first);
        });
    }

    #[gpui::test]
    fn focus_moves_down_and_up_between_stacked_panes(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        run_actions(&ws, cx, r#""split_down""#);
        let bottom = ws.update(cx, |ws, _| ws.focused);
        run_actions(&ws, cx, r#""focus_pane_up""#);
        let top = ws.update(cx, |ws, _| ws.focused);
        assert_ne!(top, bottom);
        run_actions(&ws, cx, r#""focus_pane_down""#);
        assert_eq!(ws.update(cx, |ws, _| ws.focused), bottom);
    }

    #[gpui::test]
    fn an_old_binding_to_a_gone_command_reports_it(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        ws.update_in(cx, |ws, window, cx| {
            ws.run_named_command(&RunNamedCommand("gone".into()), window, cx)
        });
        assert_eq!(error_texts(&ws, cx), [r#"no command named "gone""#]);
    }

    #[gpui::test]
    fn raw_text_steps_reach_every_pane_but_never_a_password_prompt(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        run_actions(&ws, cx, r#""split_right""#);
        let right = terminal(&ws, cx, 0);
        // the left pane asks for a password
        let left = ws.update(cx, |ws, cx| {
            let pane = ws.layout.pane_ids()[0];
            ws.terminal_at(pane, 0, cx)
        });
        left.update(cx, |t, _| {
            t.input(
                InputSource::Keyboard,
                b"python3 -c \"import getpass; getpass.getpass('Pw: ')\"\r".as_slice(),
            )
        });
        // getpass turns echo off before it prints its prompt
        wait_until(cx, "the password prompt never showed", |cx| {
            left.update(cx, |t, _| t.visible_screen_text(false).0.contains("Pw:"))
        });
        run_actions(
            &ws,
            cx,
            r#"{"target": "all_panes_in_tab", "action": {"send_text": "echo raw_$((5*5))\r"}}"#,
        );
        let refused = "macro input was not typed: a program asks for a password";
        // the tab also tells it waits for a password, only the refusals count here
        let refusals = |ws: &Entity<Workspace>, cx: &mut VisualTestContext| {
            notifications(ws, cx)
                .iter()
                .filter(|n| *n == refused)
                .count()
        };
        assert_eq!(refusals(&ws, cx), 1);
        wait_until(cx, "the raw text never ran", |cx| {
            right.update(cx, |t, _| t.visible_screen_text(false).0.contains("raw_25"))
        });
        // a step aimed at the prompt's own tab is refused the same way
        ws.update_in(cx, |ws, window, cx| {
            let pane = ws.layout.pane_ids()[0];
            ws.focus_pane(pane, window, cx)
        });
        run_actions(&ws, cx, r#"{"send_text": "x"}"#);
        assert_eq!(refusals(&ws, cx), 2);
        left.update(cx, |t, _| {
            t.input(InputSource::Keyboard, b"\x03".as_slice())
        });
    }
}
