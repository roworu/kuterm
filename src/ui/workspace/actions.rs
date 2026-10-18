//! handlers of the gpui actions keys and menus send the workspace

use super::*;

impl Workspace {
    pub(super) fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.add_tab(window, cx);
    }

    pub(super) fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.focused_pane().active;
        let pane = self.focused;
        self.request_close_tab(pane, ix, window, cx);
    }

    pub(super) fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        // wraps from the last tab back to the first
        let count = self.focused_pane().tabs.len();
        let ix = (self.focused_pane().active + 1) % count;
        let pane = self.focused;
        self.activate_tab(pane, ix, window, cx);
    }

    pub(super) fn activate_tab_action(
        &mut self,
        action: &ActivateTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.focused_pane().tabs.len();
        let pane = self.focused;
        if action.0 < count {
            self.activate_tab(pane, action.0, window, cx);
        }
    }

    pub(super) fn toggle_fullscreen(
        &mut self,
        _: &ToggleFullscreen,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.toggle_fullscreen();
    }

    pub(super) fn quit_action(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
        self.request_quit(window, cx);
    }

    pub(super) fn minimize(&mut self, _: &Minimize, window: &mut Window, _: &mut Context<Self>) {
        window.minimize_window();
    }

    pub(super) fn zoom(&mut self, _: &Zoom, window: &mut Window, _: &mut Context<Self>) {
        window.zoom_window();
    }

    // none step goes back to the size from settings
    pub(super) fn change_font_size(
        &self,
        pane: usize,
        ix: usize,
        step: Option<f32>,
        cx: &mut Context<Self>,
    ) {
        self.view_at(pane, ix).update(cx, |view, cx| {
            let size = step.map(|step| view.font_size(cx) + step);
            view.set_font_size(size, cx);
        });
    }

    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        let Some(path) = Settings::path() else {
            self.show_notification("no config folder to open settings from", None, cx);
            return;
        };
        // macos has no app set for .jsonc files, so it is opened as plain text
        if cfg!(target_os = "macos") {
            cx.background_spawn(async move {
                std::process::Command::new("open")
                    .arg("-t")
                    .arg(path)
                    .status()
                    .ok();
            })
            .detach();
        } else {
            cx.open_with_system(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn f11_toggles_fullscreen(cx: &mut TestAppContext) {
        let (_ws, cx) = open(cx, 1);
        assert!(!cx.update(|window, _| window.is_fullscreen()));
        type_keys(cx, "f11");
        assert!(cx.update(|window, _| window.is_fullscreen()));
        type_keys(cx, "f11");
        assert!(!cx.update(|window, _| window.is_fullscreen()));
    }

    #[gpui::test]
    fn the_quit_action_asks_while_a_program_runs(cx: &mut TestAppContext) {
        let (ws, cx) = open_with_settings(
            cx,
            1,
            "{}",
            settings_of(&[r#"{"confirm_quit": "running"}"#]),
        );
        start_program(&ws, cx);
        cx.dispatch_action(Quit);
        cx.run_until_parked();
        assert!(dialog(&ws, cx).is_some(), "quit without asking");
        assert!(!quitting(&ws, cx));

        let (ws, cx) = open_with_settings(
            cx,
            1,
            "{}",
            settings_of(&[r#"{"confirm_quit": "running"}"#]),
        );
        cx.dispatch_action(Quit);
        cx.run_until_parked();
        assert!(quitting(&ws, cx));
    }
}
