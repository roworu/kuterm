//! closing tabs, panes and windows, and quitting, asking first while programs still run

use super::*;

// how many program names the quit dialog lists before cutting the list short
pub(super) const QUIT_DIALOG_MAX_NAMES: usize = 3;

/// program names joined for a dialog, cut short after a few
pub(super) fn name_list(names: &[String]) -> String {
    let mut list = names[..names.len().min(QUIT_DIALOG_MAX_NAMES)].join(", ");
    if names.len() > QUIT_DIALOG_MAX_NAMES {
        list.push_str(", …");
    }
    list
}

/// dialog text naming the programs still running in tabs about to close
pub(super) fn side_close_message(names: &[String]) -> String {
    let list = name_list(names);
    match names.len() {
        1 => format!("a tab to close is still running a program: {list}"),
        count => format!("{count} tabs to close are still running programs: {list}"),
    }
}

/// quit dialog text naming the programs still running, the list is cut after a few names
pub(super) fn quit_message(names: &[String]) -> String {
    let list = name_list(names);
    match names.len() {
        0 => "quit kuterm?".to_owned(),
        1 => format!("1 tab is still running a program: {list}"),
        count => format!("{count} tabs are still running programs: {list}"),
    }
}

impl Workspace {
    /// close every tab of the focused pane, asking once when programs still run in them
    pub(super) fn request_close_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.focused_pane().tabs.len();
        self.request_close_range(self.focused, (0..count).collect(), window, cx);
    }

    /// name of the program running in the tab at `ix` of pane `pane_id`, none when only the shell is there
    pub(super) fn running_program(&self, pane_id: usize, ix: usize, cx: &App) -> Option<String> {
        let shell_pid = self.terminal_at(pane_id, ix, cx).read(cx).shell_pid;
        // a program stopped with ctrl+z is lost with the tab just like a running one
        let names: Vec<String> = foreground_process(shell_pid)
            .filter(|process| process.pid != shell_pid)
            .map(|process| process.name)
            .into_iter()
            .chain(
                stopped_jobs(shell_pid)
                    .into_iter()
                    .map(|job| format!("{} (stopped)", job.name)),
            )
            .collect();
        (!names.is_empty()).then(|| names.join(", "))
    }

    /// close the tab at `ix` of `pane_id`, asking first when a program still runs in it
    pub(super) fn request_close_tab(
        &mut self,
        pane_id: usize,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // actions run before the dialog sees keys, so a second close must not replace the dialog
        if self.confirm_close.is_some() {
            return;
        }
        let warn = Settings::get(cx).close_running_tab_warn;
        let Some(program) = warn
            .then(|| self.running_program(pane_id, ix, cx))
            .flatten()
        else {
            self.close_tab_at(pane_id, ix, window, cx);
            return;
        };
        let message = format!(
            "\"{program}\" is still running in tab {} \"{}\"",
            ix + 1,
            self.tab_title(pane_id, ix, cx)
        );
        let id = self.view_at(pane_id, ix).entity_id();
        self.open_confirm(message, CloseTarget::Tabs(vec![id]), window, cx);
    }

    /// close every tab on one side of `ix` in `pane_id`, asking once when programs still run in them
    pub(super) fn request_close_tabs_side(
        &mut self,
        pane_id: usize,
        ix: usize,
        right: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tabs = &self.pane(pane_id).expect("pane from caller").tabs;
        let range = if right { ix + 1..tabs.len() } else { 0..ix };
        let unpinned = range.filter(|&ix| !tabs[ix].pinned).collect();
        self.request_close_range(pane_id, unpinned, window, cx);
    }

    /// close every tab of `pane_id` but `ix` and pinned ones, asking once when programs still
    /// run in them
    pub(super) fn request_close_others(
        &mut self,
        pane_id: usize,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tabs = &self.pane(pane_id).expect("pane from caller").tabs;
        let others = (0..tabs.len())
            .filter(|&other| other != ix && !tabs[other].pinned)
            .collect();
        self.request_close_range(pane_id, others, window, cx);
    }

    /// close the tabs at `indices` of `pane_id`, asking once when programs still run in them
    pub(super) fn request_close_range(
        &mut self,
        pane_id: usize,
        indices: Vec<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.confirm_close.is_some() || indices.is_empty() {
            return;
        }
        let ids: Vec<EntityId> = indices
            .iter()
            .map(|&ix| self.view_at(pane_id, ix).entity_id())
            .collect();
        let names: Vec<String> = if Settings::get(cx).close_running_tab_warn {
            indices
                .into_iter()
                .filter_map(|ix| self.running_program(pane_id, ix, cx))
                .collect()
        } else {
            Vec::new()
        };
        if names.is_empty() {
            self.close_tabs(&ids, window, cx);
        } else {
            self.open_confirm(
                side_close_message(&names),
                CloseTarget::Tabs(ids),
                window,
                cx,
            );
        }
    }

    /// close the tabs with these views, skipping any that are already gone
    pub(super) fn close_tabs(
        &mut self,
        ids: &[EntityId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for id in ids {
            if let Some((pane_id, ix)) = self.find_id(*id) {
                self.close_tab_at(pane_id, ix, window, cx);
            }
        }
    }

    /// quit, asking first when programs still run in some tabs
    pub fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_close.is_some() {
            return;
        }
        if !self.ask_before_quit(CloseTarget::Quit, window, cx) {
            self.quit(cx);
        }
    }

    /// the window is about to close, true lets it
    pub(super) fn should_close_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.quitting {
            return true;
        }
        // a second close while asked keeps the dialog up
        self.confirm_close.is_none() && !self.ask_before_quit(CloseTarget::Window, window, cx)
    }

    /// open the quit dialog when `confirm_quit` asks for it, true when it opened
    pub(super) fn ask_before_quit(
        &mut self,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let confirm = Settings::get(cx).confirm_quit;
        if confirm == ConfirmQuit::Never {
            return false;
        }
        let names: Vec<String> = self
            .layout
            .panes()
            .into_iter()
            .flat_map(|pane| (0..pane.tabs.len()).map(move |ix| (pane.id, ix)))
            .filter_map(|(pane_id, ix)| self.running_program(pane_id, ix, cx))
            .collect();
        if names.is_empty() && confirm != ConfirmQuit::Always {
            return false;
        }
        self.open_confirm(quit_message(&names), target, window, cx);
        true
    }

    pub(super) fn open_confirm(
        &mut self,
        message: String,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let confirm = cx.new(|cx| ConfirmClose::new(message, target.clone(), cx));
        self.open_overlay(&confirm, window, cx);
        self._overlay_subscriptions.push(cx.subscribe_in(
            &confirm,
            window,
            move |this, _, _: &CloseConfirmed, window, cx| {
                this.close_overlay(window, cx);
                match &target {
                    // tabs may have moved or closed while the dialog was open
                    CloseTarget::Tabs(ids) => this.close_tabs(ids, window, cx),
                    CloseTarget::Window => this.close_window(window, cx),
                    CloseTarget::Quit => this.quit(cx),
                }
            },
        ));
        self.confirm_close = Some(confirm);
    }

    // other windows stay, only the last one quits
    fn close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cx.windows().len() > 1 {
            self.quitting = true;
            window.remove_window();
        } else {
            self.quit(cx);
        }
    }

    pub(super) fn quit(&mut self, cx: &mut Context<Self>) {
        self.quitting = true;
        cx.quit();
    }

    pub(super) fn close_tab_at(
        &mut self,
        pane_id: usize,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_view = |this: &Self| {
            let pane = this.pane(pane_id)?;
            Some(pane.tabs.get(pane.active)?.view.entity_id())
        };
        let old_active = active_view(self);
        let Some((closed_tab, empty)) = self.take_tab(pane_id, ix) else {
            return;
        };
        let closed = closed_tab.view.entity_id();
        // the shell may exit while its tab is asked about, leaving nothing to confirm.
        // a batch keeps its dialog until every tab it asks about is gone
        let target = self
            .confirm_close
            .as_ref()
            .map(|c| c.read(cx).target.clone());
        if let Some(target) = target
            && target.contains(closed)
            && !self.any_target_present(&target)
        {
            self.close_overlay(window, cx);
        }
        if empty {
            let before = self.layout.pane_ids();
            let at = before.iter().position(|id| *id == pane_id);
            self.layout.remove_leaf(pane_id);
            if self.layout.first_pane_id().is_none() {
                // other windows stay, only the last one quits
                if cx.windows().len() > 1 {
                    self.quitting = true;
                    window.remove_window();
                } else {
                    self.quit(cx);
                }
                return;
            }
            if self.focused == pane_id {
                // focus the pane that takes the removed one's place, not the first pane
                let after = self.layout.pane_ids();
                self.focus_pane(after[at.unwrap_or(0).min(after.len() - 1)], window, cx);
                self.focus_overlay(window, cx);
            } else {
                self.refresh_titles();
                cx.notify();
            }
            return;
        }
        let active = self.pane(pane_id).expect("pane from take_tab").active;
        // a dialog that still asks about a remaining tab keeps focus and stays up, the tab
        // in front is the same even when one left of it closed
        if self.confirm_close.is_some() && active_view(self) == old_active {
            self.refresh_titles();
            cx.notify();
        } else if self.focused == pane_id {
            self.activate_tab(pane_id, active, window, cx);
            self.focus_overlay(window, cx);
        } else {
            self.refresh_titles();
            cx.notify();
        }
    }

    /// true when some tab still in the layout is part of `target`
    pub(super) fn any_target_present(&self, target: &CloseTarget) -> bool {
        self.layout
            .panes()
            .into_iter()
            .flat_map(|pane| &pane.tabs)
            .any(|tab| target.contains(tab.view.entity_id()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::test_support::*;

    // what these tests are about, so they never follow a change of the bundled defaults
    const ASKING: &str = r#"{"close_running_tab_warn": true, "confirm_quit": "running"}"#;

    /// a workspace that asks before closing a running tab and before quitting with one
    fn open(cx: &mut TestAppContext, tabs: usize) -> (Entity<Workspace>, &mut VisualTestContext) {
        open_with_settings(cx, tabs, "{}", settings_of(&[ASKING]))
    }

    #[gpui::test]
    fn running_tab_asks_before_closing(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        assert_eq!(views(&ws, cx).len(), 2);
        assert!(cx.debug_bounds("confirm-close").is_some());
        assert!(ws.update_in(cx, |ws, window, cx| {
            ws.confirm_close
                .as_ref()
                .is_some_and(|confirm| confirm.focus_handle(cx).is_focused(window))
        }));

        // escape keeps the tab and gives focus back to it
        type_keys(cx, "escape");
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 1);

        close_active(&ws, cx);
        type_keys(cx, "enter");
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 1);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn arrows_pick_the_button_enter_presses(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        // "close" starts selected, left moves to "cancel"
        type_keys(cx, "left enter");
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 1);

        close_active(&ws, cx);
        type_keys(cx, "left right enter");
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[gpui::test]
    fn dialog_closes_when_its_tab_exits(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        // ctrl-c stops sleep, then the shell exits and its tab closes by itself
        terminal(&ws, cx, 1).update(cx, |terminal, _| terminal.type_text(b"\x03exit\n"));
        wait_until(cx, "tab never exited", |cx| views(&ws, cx).len() == 1);
        cx.run_until_parked();
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn close_tabs_to_the_right_keeps_the_active_and_its_left_neighbors(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 4);
        let old = views(&ws, cx);
        assert_eq!(activate(&ws, cx, 1), 1);
        run_actions(&ws, cx, r#""close_tabs_to_right""#);
        assert_eq!(views(&ws, cx), vec![old[0], old[1]]);
        assert_eq!(current(&ws, cx), 1);
        assert!(cx.debug_bounds("confirm-close").is_none());
    }

    #[gpui::test]
    fn close_tabs_to_the_left_keeps_the_active_and_its_right_neighbors(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 4);
        let old = views(&ws, cx);
        assert_eq!(activate(&ws, cx, 2), 2);
        run_actions(&ws, cx, r#""close_tabs_to_left""#);
        assert_eq!(views(&ws, cx), vec![old[2], old[3]]);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn closing_an_empty_tab_side_does_nothing(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let old = views(&ws, cx);
        assert_eq!(activate(&ws, cx, 1), 1);
        run_actions(&ws, cx, r#""close_tabs_to_right""#);
        assert_eq!(views(&ws, cx), old);
        assert_eq!(current(&ws, cx), 1);
        assert_eq!(activate(&ws, cx, 0), 0);
        run_actions(&ws, cx, r#""close_tabs_to_left""#);
        assert_eq!(views(&ws, cx), old);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn close_tabs_side_asks_once_for_running_programs(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        let old = views(&ws, cx);
        start_program(&ws, cx);
        activate(&ws, cx, 0);
        run_actions(&ws, cx, r#""close_tabs_to_right""#);
        assert_eq!(views(&ws, cx), old);
        let (target, message) = dialog(&ws, cx).expect("busy side closed without asking");
        assert_eq!(target, CloseTarget::Tabs(vec![old[1], old[2]]));
        assert_eq!(message, "a tab to close is still running a program: sleep");
        type_keys(cx, "enter");
        assert_eq!(views(&ws, cx), vec![old[0]]);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn close_tabs_side_dialog_survives_one_tab_exiting(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        let old = views(&ws, cx);
        start_program(&ws, cx);
        activate(&ws, cx, 0);
        // ask about the two tabs to the right, then end the shell of the first
        run_actions(&ws, cx, r#""close_tabs_to_right""#);
        assert_eq!(views(&ws, cx), old);
        terminal(&ws, cx, 1).update(cx, |terminal, _| terminal.type_text(b"exit\n"));
        wait_until(cx, "tab never exited", |cx| views(&ws, cx).len() == 2);
        cx.run_until_parked();
        // one target is gone, the other still runs, so the dialog stays up
        assert!(cx.debug_bounds("confirm-close").is_some());
        type_keys(cx, "enter");
        assert_eq!(views(&ws, cx), vec![old[0]]);
        assert_eq!(current(&ws, cx), 0);
    }

    #[test]
    fn side_close_message_lists_a_few_programs() {
        let names = |n: &[&str]| n.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(
            side_close_message(&names(&["vim"])),
            "a tab to close is still running a program: vim"
        );
        assert_eq!(
            side_close_message(&names(&["vim", "cargo"])),
            "2 tabs to close are still running programs: vim, cargo"
        );
    }

    #[gpui::test]
    fn close_button_in_dialog_closes_running_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        run_actions(&ws, cx, r#""close_tab""#);
        let button = cx.debug_bounds("confirm-close-ok").unwrap().center();
        click(cx, button, 1);
        release(cx, button);
        cx.run_until_parked();
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[gpui::test]
    fn disabled_warning_closes_running_tab_at_once(cx: &mut TestAppContext) {
        let settings = settings_of(&[ASKING, r#"{"close_running_tab_warn": false}"#]);
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        start_program(&ws, cx);
        close_active(&ws, cx);
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[gpui::test]
    fn idle_tab_closes_without_asking(cx: &mut TestAppContext) {
        let settings = settings_of(&[ASKING, r#"{"close_running_tab_warn": true}"#]);
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        close_active(&ws, cx);
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[test]
    fn quit_message_lists_a_few_programs() {
        let names = |n: &[&str]| n.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(
            quit_message(&names(&["vim"])),
            "1 tab is still running a program: vim"
        );
        assert_eq!(
            quit_message(&names(&["vim", "cargo"])),
            "2 tabs are still running programs: vim, cargo"
        );
        assert_eq!(
            quit_message(&names(&["a", "b", "c", "d"])),
            "4 tabs are still running programs: a, b, c, …"
        );
        assert_eq!(quit_message(&[]), "quit kuterm?");
    }

    #[gpui::test]
    fn quit_action_asks_with_running_program(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        run_actions(&ws, cx, r#""quit""#);
        let (target, message) = dialog(&ws, cx).expect("quit asked nothing");
        assert_eq!(target, CloseTarget::Quit);
        assert_eq!(message, "1 tab is still running a program: sleep");
        assert!(!quitting(&ws, cx));

        type_keys(cx, "escape");
        assert!(dialog(&ws, cx).is_none());
        assert!(!quitting(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 2);

        run_actions(&ws, cx, r#""quit""#);
        type_keys(cx, "enter");
        assert!(quitting(&ws, cx));
    }

    #[gpui::test]
    fn idle_quit_and_disabled_warning_quit_at_once(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        run_actions(&ws, cx, r#""quit""#);
        assert!(dialog(&ws, cx).is_none());
        assert!(quitting(&ws, cx));

        let settings = settings_of(&[ASKING, r#"{"confirm_quit": "never"}"#]);
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        start_program(&ws, cx);
        run_actions(&ws, cx, r#""quit""#);
        assert!(dialog(&ws, cx).is_none());
        assert!(quitting(&ws, cx));
    }

    #[gpui::test]
    fn confirm_quit_always_asks_when_idle(cx: &mut TestAppContext) {
        let settings = settings_of(&[ASKING, r#"{"confirm_quit": "always"}"#]);
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        run_actions(&ws, cx, r#""quit""#);
        let (target, message) = dialog(&ws, cx).expect("quit asked nothing");
        assert_eq!(target, CloseTarget::Quit);
        assert_eq!(message, "quit kuterm?");
        type_keys(cx, "escape");
        assert!(!quitting(&ws, cx));
        assert!(!cx.simulate_close(), "idle window closed without asking");
        type_keys(cx, "enter");
        assert!(quitting(&ws, cx));
    }

    #[gpui::test]
    fn confirm_quit_is_separate_from_close_tab_warning(cx: &mut TestAppContext) {
        let settings = settings_of(&[
            ASKING,
            r#"{"close_running_tab_warn": false, "confirm_quit": "running"}"#,
        ]);
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        start_program(&ws, cx);
        assert!(!cx.simulate_close(), "busy window closed without asking");
        assert_eq!(dialog(&ws, cx).map(|d| d.0), Some(CloseTarget::Window));
    }

    #[gpui::test]
    fn idle_window_closes_at_once(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        assert!(cx.simulate_close(), "idle window kept open");
        assert!(dialog(&ws, cx).is_none());
    }

    #[gpui::test]
    fn window_close_asks_with_running_program(cx: &mut TestAppContext) {
        let (ws2, cx2) = open(cx, 1);
        start_program(&ws2, cx2);
        assert!(!cx2.simulate_close(), "busy window closed without asking");
        assert_eq!(dialog(&ws2, cx2).map(|d| d.0), Some(CloseTarget::Window));
        // closing again while asked keeps the dialog
        assert!(!cx2.simulate_close());
        type_keys(cx2, "escape");
        assert!(dialog(&ws2, cx2).is_none());
        assert!(!quitting(&ws2, cx2));

        assert!(!cx2.simulate_close());
        type_keys(cx2, "enter");
        assert!(quitting(&ws2, cx2));
        assert!(
            cx2.simulate_close(),
            "confirmed quit still blocks the close"
        );
    }

    #[gpui::test]
    fn confirmed_window_close_keeps_other_windows(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        run_actions(&ws, cx, r#"{"activate_tab": 2}, "move_tab_to_new_window""#);
        assert_eq!(workspaces(cx).len(), 2);
        start_program(&ws, cx);
        assert!(!cx.simulate_close(), "busy window closed without asking");
        type_keys(cx, "enter");
        cx.run_until_parked();
        let left = workspaces(cx);
        assert_eq!(left.len(), 1, "the other window closed too");
        let other_quits = cx
            .cx
            .update(|cx| left[0].read_with(cx, |ws, _| ws.quitting))
            .unwrap();
        assert!(!other_quits);
    }

    #[gpui::test]
    fn disabled_warning_closes_busy_window(cx: &mut TestAppContext) {
        let settings = settings_of(&[ASKING, r#"{"confirm_quit": "never"}"#]);
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        start_program(&ws, cx);
        assert!(cx.simulate_close());
        assert!(dialog(&ws, cx).is_none());
    }

    #[gpui::test]
    fn middle_click_asks_about_the_clicked_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        activate(&ws, cx, 1);
        start_program(&ws, cx);
        activate(&ws, cx, 2);
        let old = views(&ws, cx);
        let title = cx.debug_bounds("tab-title-1").unwrap().center();
        cx.simulate_event(MouseDownEvent {
            position: title,
            modifiers: Modifiers::default(),
            button: MouseButton::Middle,
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        let (target, message) = dialog(&ws, cx).expect("busy tab closed without asking");
        assert_eq!(target, CloseTarget::Tabs(vec![old[1]]));
        assert!(
            message.starts_with("\"sleep\" is still running in tab 2 \""),
            "{message}"
        );
        type_keys(cx, "enter");
        assert_eq!(views(&ws, cx), vec![old[0], old[2]]);
        // the active tab stays active, it just moved left
        assert_eq!(current(&ws, cx), 1);
    }

    #[gpui::test]
    fn modified_keys_do_not_answer_the_dialog(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        for keys in [
            "ctrl-y",
            "ctrl-n",
            "alt-y",
            "ctrl-enter",
            "shift-enter",
            "ctrl-escape",
        ] {
            type_keys(cx, keys);
            assert!(dialog(&ws, cx).is_some(), "{keys} answered the dialog");
        }
        // arrows work with modifiers held
        type_keys(cx, "shift-left enter");
        assert!(dialog(&ws, cx).is_none());
        assert_eq!(views(&ws, cx).len(), 2);

        close_active(&ws, cx);
        type_keys(cx, "shift-n");
        assert!(dialog(&ws, cx).is_none());
        assert_eq!(views(&ws, cx).len(), 2);
        close_active(&ws, cx);
        type_keys(cx, "shift-y");
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[gpui::test]
    fn plain_y_and_n_answer_the_dialog(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        type_keys(cx, "n");
        assert!(dialog(&ws, cx).is_none());
        assert_eq!(views(&ws, cx).len(), 2);
        close_active(&ws, cx);
        type_keys(cx, "y");
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[gpui::test]
    fn close_tab_key_does_not_stack_dialogs(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        type_keys(cx, "ctrl-shift-w");
        let first = ws.update(cx, |ws, _| ws.confirm_close.as_ref().map(|c| c.entity_id()));
        assert!(first.is_some());
        type_keys(cx, "ctrl-shift-w");
        let second = ws.update(cx, |ws, _| ws.confirm_close.as_ref().map(|c| c.entity_id()));
        assert_eq!(first, second, "a second dialog replaced the first");
        type_keys(cx, "escape");
        assert!(dialog(&ws, cx).is_none());
        assert_eq!(views(&ws, cx).len(), 2);
    }

    #[gpui::test]
    fn palette_key_replaces_the_dialog(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        type_keys(cx, "ctrl-shift-p");
        assert!(dialog(&ws, cx).is_none());
        assert!(palette_focused(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 2);
    }

    #[gpui::test]
    fn new_tab_key_closes_the_dialog(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        type_keys(cx, "ctrl-shift-t");
        assert!(dialog(&ws, cx).is_none());
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 3);
        assert_eq!(current(&ws, cx), 2);
    }

    #[gpui::test]
    fn tab_switch_key_closes_the_dialog(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        type_keys(cx, "alt-1");
        assert!(dialog(&ws, cx).is_none());
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn disabled_warning_closes_a_busy_side_at_once(cx: &mut TestAppContext) {
        let settings = settings_of(&[ASKING, r#"{"close_running_tab_warn": false}"#]);
        let (ws, cx) = open_with_settings(cx, 3, "{}", settings);
        let old = views(&ws, cx);
        start_program(&ws, cx);
        activate(&ws, cx, 0);
        run_actions(&ws, cx, r#""close_tabs_to_right""#);
        assert!(dialog(&ws, cx).is_none());
        assert_eq!(views(&ws, cx), vec![old[0]]);
    }

    #[gpui::test]
    fn quit_while_a_close_dialog_asks_keeps_that_dialog(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let old = views(&ws, cx);
        start_program(&ws, cx);
        close_active(&ws, cx);
        run_actions(&ws, cx, r#""quit""#);
        let (target, _) = dialog(&ws, cx).expect("the close dialog went away");
        assert_eq!(target, CloseTarget::Tabs(vec![old[1]]));
        assert!(!quitting(&ws, cx));
    }

    #[gpui::test]
    fn quit_dialog_stays_when_a_tab_left_of_the_active_one_exits(cx: &mut TestAppContext) {
        let settings = settings_of(&[ASKING, r#"{"confirm_quit": "always"}"#]);
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        run_actions(&ws, cx, r#""quit""#);
        assert_eq!(dialog(&ws, cx).map(|d| d.0), Some(CloseTarget::Quit));
        terminal(&ws, cx, 0).update(cx, |terminal, _| terminal.type_text(b"exit\n"));
        wait_until(cx, "tab never exited", |cx| views(&ws, cx).len() == 1);
        cx.run_until_parked();
        assert_eq!(dialog(&ws, cx).map(|d| d.0), Some(CloseTarget::Quit));
    }

    #[gpui::test]
    fn quit_dialog_stays_when_a_tab_exits(cx: &mut TestAppContext) {
        let settings = settings_of(&[ASKING, r#"{"confirm_quit": "always"}"#]);
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        activate(&ws, cx, 0);
        run_actions(&ws, cx, r#""quit""#);
        assert_eq!(dialog(&ws, cx).map(|d| d.0), Some(CloseTarget::Quit));
        terminal(&ws, cx, 1).update(cx, |terminal, _| terminal.type_text(b"exit\n"));
        wait_until(cx, "tab never exited", |cx| views(&ws, cx).len() == 1);
        cx.run_until_parked();
        // the window is still asked about
        assert_eq!(dialog(&ws, cx).map(|d| d.0), Some(CloseTarget::Quit));
        type_keys(cx, "enter");
        assert!(quitting(&ws, cx));
    }

    #[gpui::test]
    fn closing_tabs_of_a_pane_out_of_focus_keeps_the_focus(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        let first = ws.update(cx, |ws, _| ws.focused);
        let second = split_pane(&ws, cx, DropSide::Right);
        assert_ne!(first, second);
        let focused_view = active_view(&ws, cx).entity_id();
        let tabs_of = |pane: usize, cx: &mut VisualTestContext| {
            ws.update(cx, |ws, _| ws.pane(pane).map(|pane| pane.tabs.len()))
        };
        assert_eq!(tabs_of(first, cx), Some(2));
        let close_first_tab = |cx: &mut VisualTestContext| {
            ws.update_in(cx, |ws, window, cx| ws.close_tab_at(first, 0, window, cx));
            cx.run_until_parked();
        };
        close_first_tab(cx);
        assert_eq!(tabs_of(first, cx), Some(1));
        assert_eq!(ws.update(cx, |ws, _| ws.focused), second);
        assert_eq!(active_view(&ws, cx).entity_id(), focused_view);
        current(&ws, cx);
        // its last tab takes the pane with it, the focused pane stays focused
        close_first_tab(cx);
        assert_eq!(tabs_of(first, cx), None);
        assert_eq!(ws.update(cx, |ws, _| ws.layout.pane_ids()), [second]);
        assert_eq!(active_view(&ws, cx).entity_id(), focused_view);
        current(&ws, cx);
        assert!(!quitting(&ws, cx));
    }

    #[gpui::test]
    fn last_tab_of_one_window_closes_only_that_window(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        run_actions(&ws, cx, r#""move_tab_to_new_window""#);
        let first_id = cx.update(|window, _| window.window_handle().window_id());
        let windows = workspaces(cx);
        assert_eq!(windows.len(), 2);
        let second = windows
            .into_iter()
            .find(|w| w.window_id() != first_id)
            .unwrap();
        cx.cx.update(|cx| {
            second
                .update(cx, |ws, window, cx| {
                    ws.close_tab_at(ws.focused, 0, window, cx)
                })
                .unwrap()
        });
        cx.run_until_parked();
        let left: Vec<_> = workspaces(cx).iter().map(|w| w.window_id()).collect();
        assert_eq!(left, [first_id]);
        assert!(!quitting(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 1);
    }
}
