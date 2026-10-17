//! the command palette and the tab picker

use super::*;

// how many recently run commands the palette remembers
pub(super) const RECENT_COMMANDS_MAX: usize = 10;

impl Workspace {
    pub(super) fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette.is_some() {
            self.close_overlay(window, cx);
            return;
        }
        if !Settings::get(cx).command_palette.enable {
            return;
        }
        let mut commands = Commands::get(cx).commands.clone();
        let serving = self.serving_commands(cx);
        commands.extend(serving.iter().map(|(command, _)| command.clone()));
        // recency is ignored when disabled, so the palette only sees recent names when it is on
        let recent = if Settings::get(cx).command_palette.show_recent {
            self.recent_commands.clone()
        } else {
            Vec::new()
        };
        let palette = cx.new(|cx| CommandPalette::new(commands, recent, cx));
        self.open_overlay(&palette, window, cx);
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            move |this, _, command: &Command, window, cx| {
                this.close_overlay(window, cx);
                // ports come and go, so these are not remembered as recent commands
                if let Some((_, id)) = serving.iter().find(|(serving, _)| serving == command) {
                    this.activate_id(*id, window, cx);
                    return;
                }
                this.record_recent(command.label());
                this.pick_command(command.clone(), window, cx);
            },
        ));
        self.palette = Some(palette);
    }

    /// palette listing the open tabs, picking one switches to it
    pub(super) fn open_tab_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused;
        let tabs = (0..self.focused_pane().tabs.len())
            .map(|ix| {
                Command::new(
                    self.tab_title(pane, ix, cx),
                    Some((ix + 1).to_string()),
                    vec![CommandAction::ActivateTab(ix + 1).into()],
                )
            })
            .collect();
        let palette = cx.new(|cx| CommandPalette::tab_picker(tabs, cx));
        self.open_overlay(&palette, window, cx);
        // tabs change all the time, so picks are not remembered as recent commands
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            |this, _, command: &Command, window, cx| {
                this.close_overlay(window, cx);
                this.run_command(command, window, cx);
            },
        ));
        self.palette = Some(palette);
    }

    /// a "go to tab serving :port" command per port a tab listens on, while ports are measured
    pub(super) fn serving_commands(&self, cx: &App) -> Vec<(Command, EntityId)> {
        if !shows_samples(Settings::get(cx)) {
            return Vec::new();
        }
        let mut commands = Vec::new();
        for pane in self.layout.panes() {
            for tab in &pane.tabs {
                let pid = tab.view.read(cx).terminal().read(cx).shell_pid;
                let ports = ProcessSampler::shared()
                    .sample(pid)
                    .unwrap_or_default()
                    .ports;
                for port in ports {
                    let command = Command::new(
                        format!("go to tab serving :{port}"),
                        Some("tabs".into()),
                        Vec::new(),
                    );
                    commands.push((command, tab.view.entity_id()));
                }
            }
        }
        commands
    }

    /// palette of `rows` in their order, each `text` and then the title of tab `ix` of pane
    /// `pane`, picking one switches to it
    pub(super) fn open_tab_rows(
        &mut self,
        rows: Vec<(String, usize, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panes = self.layout.pane_ids();
        let rows: Vec<(Command, EntityId)> = rows
            .into_iter()
            .map(|(text, pane, ix)| {
                // the number tells the rows apart, so a pick finds its tab again
                let number = match panes.iter().position(|id| *id == pane) {
                    Some(at) if panes.len() > 1 => format!("{}.{}", at + 1, ix + 1),
                    _ => (ix + 1).to_string(),
                };
                let name = format!("{text}  {}", self.tab_title(pane, ix, cx));
                let id = self.view_at(pane, ix).entity_id();
                (Command::new(name, Some(number), Vec::new()), id)
            })
            .collect();
        let commands = rows.iter().map(|(command, _)| command.clone()).collect();
        let palette = cx.new(|cx| CommandPalette::tab_picker(commands, cx));
        self.open_overlay(&palette, window, cx);
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            move |this, _, command: &Command, window, cx| {
                this.close_overlay(window, cx);
                if let Some((_, id)) = rows.iter().find(|(row, _)| row == command) {
                    this.activate_id(*id, window, cx);
                }
            },
        ));
        self.palette = Some(palette);
    }

    /// remember a command run from the palette, most recent first, oldest dropped
    pub(super) fn record_recent(&mut self, label: String) {
        self.recent_commands.retain(|recent| *recent != label);
        self.recent_commands.insert(0, label);
        self.recent_commands.truncate(RECENT_COMMANDS_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn ctrl_shift_p_toggles_palette_and_escape_closes_it(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        type_keys(cx, "ctrl-shift-p");
        assert!(palette_open(&ws, cx));
        assert!(palette_focused(&ws, cx));
        assert!(cx.debug_bounds("command-palette").is_some());
        assert_eq!(
            palette_names(&ws, cx).len(),
            Commands::default().commands.len()
        );

        type_keys(cx, "escape");
        assert!(!palette_open(&ws, cx));
        // focus went back to the active tab
        assert_eq!(current(&ws, cx), 1);

        type_keys(cx, "ctrl-shift-p");
        type_keys(cx, "ctrl-shift-p");
        assert!(!palette_open(&ws, cx));
        assert_eq!(current(&ws, cx), 1);
    }

    #[gpui::test]
    fn disabled_palette_never_opens(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"command_palette": {"enable": false}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        type_keys(cx, "ctrl-shift-p");
        assert!(!palette_open(&ws, cx));
        assert!(cx.debug_bounds("command-palette").is_none());
    }

    #[gpui::test]
    fn typing_filters_and_arrows_pick_a_command(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [
                {"name": "first", "actions": ["new_tab"]},
                {"name": "second", "actions": ["new_tab", "new_tab"]},
                {"name": "sneaky", "actions": ["new_tab", "new_tab", "new_tab"]},
            ]}"#,
        );
        type_keys(cx, "ctrl-shift-p s");
        assert_eq!(palette_names(&ws, cx), ["first", "second", "sneaky"]);
        type_keys(cx, "e");
        assert_eq!(palette_names(&ws, cx), ["second", "sneaky"]);
        type_keys(cx, "x");
        assert!(palette_names(&ws, cx).is_empty());
        assert!(cx.debug_bounds("command-0").is_none());
        // enter with nothing matched keeps the palette open
        type_keys(cx, "enter");
        assert!(palette_open(&ws, cx));
        type_keys(cx, "backspace down up enter");
        assert!(!palette_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 3);
    }

    #[gpui::test]
    fn arrows_wrap_between_first_and_last_command(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [
                {"name": "first", "actions": ["new_tab"]},
                {"name": "second", "actions": ["new_tab", "new_tab"]},
            ]}"#,
        );
        // down from the last command wraps to the first one
        type_keys(cx, "ctrl-shift-p down down enter");
        assert!(!palette_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 2);
        // up from the first command wraps to the last one, "first" is listed first as recent
        type_keys(cx, "ctrl-shift-p up enter");
        assert!(!palette_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 4);
    }

    #[gpui::test]
    fn pick_tab_lists_tabs_and_switches_to_the_picked_one(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        run_actions(&ws, cx, r#""pick_tab""#);
        assert!(palette_open(&ws, cx));
        assert!(palette_focused(&ws, cx));
        let names = palette_names(&ws, cx);
        assert_eq!(names.len(), 3);
        for (ix, name) in names.iter().enumerate() {
            assert!(name.starts_with(&format!("{}: ", ix + 1)), "{name}");
        }
        // tabs have no pins to click
        assert!(cx.debug_bounds("pin-0").is_none());
        type_keys(cx, "down enter");
        assert!(!palette_open(&ws, cx));
        assert_eq!(current(&ws, cx), 1);
        // picked tabs are not remembered as recent commands
        assert!(ws.update(cx, |ws, _| ws.recent_commands.is_empty()));
    }

    #[gpui::test]
    fn palette_edits_the_query_like_a_text_input(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [{"name": "alpha beta", "actions": ["new_tab"]}]}"#,
        );
        type_keys(cx, "ctrl-shift-p a l p h a space b e t a");
        assert_eq!(palette_query(&ws, cx), "alpha beta");

        // alt+backspace deletes the word before the caret
        type_keys(cx, "alt-backspace");
        assert_eq!(palette_query(&ws, cx), "alpha ");

        // select all and copy puts the query on the clipboard
        type_keys(cx, "ctrl-a ctrl-c");
        assert_eq!(clipboard(cx).as_deref(), Some("alpha "));

        // paste replaces the selection
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("beta".into()));
        type_keys(cx, "ctrl-v");
        assert_eq!(palette_query(&ws, cx), "beta");
    }

    #[gpui::test]
    fn click_runs_a_command(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [
                {"name": "one", "actions": ["about"]},
                {"name": "two", "actions": ["new_tab"]},
            ]}"#,
        );
        type_keys(cx, "ctrl-shift-p");
        let item = cx.debug_bounds("command-1").unwrap().center();
        click(cx, item, 1);
        release(cx, item);
        cx.run_until_parked();
        assert!(!palette_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 2);
    }

    #[gpui::test]
    fn recently_run_commands_are_listed_first(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        recent_test_commands(cx);
        type_keys(cx, "ctrl-shift-p t h i r d enter");
        type_keys(cx, "ctrl-shift-p");
        assert_eq!(palette_names(&ws, cx), ["third", "first", "second"]);
        type_keys(cx, "f i r s t enter");
        type_keys(cx, "ctrl-shift-p");
        assert_eq!(palette_names(&ws, cx), ["first", "third", "second"]);
    }

    #[gpui::test]
    fn disabled_recent_sorts_by_category_and_name(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"command_palette": {"show_recent": false}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        recent_test_commands(cx);
        type_keys(cx, "ctrl-shift-p t h i r d enter");
        type_keys(cx, "ctrl-shift-p");
        assert_eq!(palette_names(&ws, cx), ["first", "second", "third"]);
    }

    #[gpui::test]
    fn commands_are_grouped_by_category_then_name(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [
                {"name": "paste", "category": "edit", "actions": ["new_tab"]},
                {"name": "zeta", "actions": ["new_tab"]},
                {"name": "create new", "category": "tabs", "actions": ["new_tab"]},
                {"name": "copy", "category": "edit", "actions": ["new_tab"]},
            ]}"#,
        );
        type_keys(cx, "ctrl-shift-p");
        // categorized commands first, each group sorted by name, uncategorized last
        assert_eq!(
            palette_names(&ws, cx),
            ["edit: copy", "edit: paste", "tabs: create new", "zeta"]
        );
    }

    #[gpui::test]
    fn pinned_commands_are_listed_first(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [
                {"name": "alpha", "actions": ["new_tab"]},
                {"name": "beta", "pinned": true, "actions": ["new_tab"]},
                {"name": "gamma", "actions": ["new_tab"]},
            ]}"#,
        );
        // run gamma so it is recent, the pinned command still wins
        type_keys(cx, "ctrl-shift-p g a m m a enter");
        type_keys(cx, "ctrl-shift-p");
        assert_eq!(palette_names(&ws, cx), ["beta", "gamma", "alpha"]);
    }

    #[gpui::test]
    fn clicking_the_pin_unpins_and_saves(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("workspace_unpin");
        crate::settings::tests::with_config_home(&dir, || {
            let (ws, cx) = open(cx, 1);
            set_commands(
                cx,
                r#"{"commands": [
                    {"name": "alpha", "actions": ["new_tab"]},
                    {"name": "beta", "pinned": true, "actions": ["new_tab"]},
                ]}"#,
            );
            type_keys(cx, "ctrl-shift-p");
            assert_eq!(palette_names(&ws, cx), ["beta", "alpha"]);

            let pin = cx.debug_bounds("pin-0").unwrap().center();
            click(cx, pin, 1);
            release(cx, pin);
            cx.run_until_parked();

            // the row was not run, the palette stays open and beta moved below alpha
            assert_eq!(views(&ws, cx).len(), 1);
            assert_eq!(palette_names(&ws, cx), ["alpha", "beta"]);
            assert_eq!(Pins::load().overrides.get("beta"), Some(&false));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn selection_scrolls_into_view(cx: &mut TestAppContext) {
        let (_ws, cx) = open(cx, 1);
        let commands = (0..40)
            .map(|n| format!(r#"{{"name": "cmd {n:02}", "actions": ["new_tab"]}}"#))
            .collect::<Vec<_>>()
            .join(",");
        set_commands(cx, &format!(r#"{{"commands": [{commands}]}}"#));
        type_keys(cx, "ctrl-shift-p");
        for _ in 0..35 {
            type_keys(cx, "down");
        }
        let list = cx.debug_bounds("command-list").unwrap();
        let item = cx.debug_bounds("command-35").unwrap();
        assert!(
            item.top() >= list.top() - gpui::px(1.)
                && item.bottom() <= list.bottom() + gpui::px(1.),
            "selected item {item:?} is outside the list {list:?}"
        );
    }

    #[gpui::test]
    fn palette_opens_below_the_menu_and_tab_bars(cx: &mut TestAppContext) {
        let settings =
            Settings::parse(r#"{"hide_bar_for_one_tab": false, "window": {"menu_bar": "always"}}"#)
                .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        ws.update_in(cx, |ws, window, cx| {
            ws.toggle_command_palette(&ToggleCommandPalette, window, cx)
        });
        cx.run_until_parked();
        // the empty part of the tab bar is as high as the bar
        let tab = cx.debug_bounds("drag-area").expect("no tab bar");
        let palette = cx.debug_bounds("command-palette").expect("no palette");
        assert!(
            palette.top() - tab.bottom() >= tab.size.height / 2.,
            "palette at {palette:?} touches the tab bar at {tab:?}"
        );
    }
}
