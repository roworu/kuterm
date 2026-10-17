//! what floats over the terminals: only one overlay is open at a time, and focus goes back to
//! the active tab once it closes

use super::*;

impl Workspace {
    // closes whatever floats over the terminal first, only one overlay is open at a time
    pub(super) fn open_overlay<V: ManagedView>(
        &mut self,
        view: &Entity<V>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(window, cx);
        self._overlay_subscriptions =
            vec![
                cx.subscribe_in(view, window, |this, _, _: &DismissEvent, window, cx| {
                    this.close_overlay(window, cx)
                }),
            ];
        view.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.about = None;
        self.confirm_close = None;
        self.confirm_paste = None;
        self.command_input = None;
        self._overlay_subscriptions.clear();
        self.focus_active_tab(window, cx);
        cx.notify();
    }

    /// give the keyboard back to the overlay still drawn over the tabs, if one is
    pub(super) fn focus_overlay(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = [
            self.palette.as_ref().map(|view| view.focus_handle(cx)),
            self.about.as_ref().map(|view| view.focus_handle(cx)),
            self.confirm_close
                .as_ref()
                .map(|view| view.focus_handle(cx)),
            self.confirm_paste
                .as_ref()
                .map(|view| view.focus_handle(cx)),
            self.command_input
                .as_ref()
                .map(|view| view.focus_handle(cx)),
        ]
        .into_iter()
        .flatten()
        .next();
        if let Some(handle) = handle {
            handle.focus(window, cx);
        }
    }

    pub(super) fn focus_active_tab(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.pane(self.focused)
            && let Some(tab) = pane.tabs.get(pane.active)
        {
            tab.view.focus_handle(cx).focus(window, cx);
        }
    }

    /// ask before pasting `text` into the tab of `view`
    pub(super) fn open_confirm_paste(
        &mut self,
        view: Entity<TerminalView>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dialog = cx.new(|cx| ConfirmPaste::new(&text, cx));
        self.open_overlay(&dialog, window, cx);
        self._overlay_subscriptions.push(cx.subscribe_in(
            &dialog,
            window,
            move |this, _, _: &PasteConfirmed, window, cx| {
                this.close_overlay(window, cx);
                view.update(cx, |view, cx| view.force_paste(&text, cx));
            },
        ));
        self.confirm_paste = Some(dialog);
    }

    pub(super) fn open_about(
        &mut self,
        _: &OpenAbout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let about = cx.new(About::new);
        self.open_overlay(&about, window, cx);
        self.about = Some(about);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn about_command_shows_page_until_a_key(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let r = reader(&ws, cx, false, "about");
        type_keys(cx, "ctrl-shift-p a b o u t enter");
        assert!(!palette_open(&ws, cx));
        assert!(ws.update(cx, |ws, _| ws.about.is_some()));
        assert!(cx.debug_bounds("about").is_some());
        type_keys(cx, "q");
        assert!(ws.update(cx, |ws, _| ws.about.is_none()));
        assert!(cx.debug_bounds("about").is_none());
        assert_eq!(current(&ws, cx), 0);
        // keys typed into the palette and the page never reach the shell
        assert_eq!(finish(&ws, cx, r), Vec::<String>::new());
    }

    #[gpui::test]
    fn unsafe_paste_asks_and_pastes_only_when_confirmed(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"notifications": {"timeout": 0}, "terminal": {"paste_protection": true}}"#,
        )
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.write_to_clipboard(ClipboardItem::new_string("echo one\necho two\n".into()));
        without_bracketed_paste(&ws, cx);
        run_actions(&ws, cx, r#""paste""#);
        assert!(ws.update(cx, |ws, _| ws.confirm_paste.is_some()));
        let preview = ws.update(cx, |ws, cx| {
            ws.confirm_paste
                .as_ref()
                .unwrap()
                .read(cx)
                .preview()
                .to_vec()
        });
        assert_eq!(preview, ["echo one", "echo two"]);
        // escape cancels, nothing is pasted
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(ws.update(cx, |ws, _| ws.confirm_paste.is_none()));
        assert!(notifications(&ws, cx).is_empty());

        without_bracketed_paste(&ws, cx);
        run_actions(&ws, cx, r#""paste""#);
        cx.simulate_keystrokes("y");
        cx.run_until_parked();
        assert!(ws.update(cx, |ws, _| ws.confirm_paste.is_none()));
        assert_eq!(notifications(&ws, cx), ["pasted from clipboard"]);
    }

    #[gpui::test]
    fn safe_or_unprotected_paste_does_not_ask(cx: &mut TestAppContext) {
        for (protection, text) in [(true, "echo one"), (false, "echo one\necho two\n")] {
            let settings = Settings::parse(&format!(
                r#"{{"notifications": {{"timeout": 0}}, "terminal": {{"paste_protection": {protection}}}}}"#
            ))
            .unwrap();
            let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
            cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
            without_bracketed_paste(&ws, cx);
            run_actions(&ws, cx, r#""paste""#);
            assert!(
                ws.update(cx, |ws, _| ws.confirm_paste.is_none()),
                "{text:?}"
            );
            assert_eq!(notifications(&ws, cx), ["pasted from clipboard"]);
        }
    }

    #[gpui::test]
    fn the_palette_keeps_the_keyboard_when_the_tab_under_it_closes(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        type_keys(cx, "ctrl-shift-p");
        assert!(palette_focused(&ws, cx));
        // like its shell exiting: the next tab comes to the front behind the palette
        ws.update_in(cx, |ws, window, cx| {
            ws.close_tab_at(ws.focused, 1, window, cx)
        });
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 1);
        assert!(palette_open(&ws, cx));
        assert!(palette_focused(&ws, cx), "the palette lost the keyboard");
    }
}
