//! things done to one tab: renaming it in place, pinning, duplicating, and the right click
//! menu listing them

use gpui::{
    AnyElement, App, Context, KeyDownEvent, Window, anchored, deferred, div, prelude::*, px,
};

use super::{
    Workspace,
    tab_bar::{menu_item, popup_menu, rename_input},
    tab_cwd, terminal_size,
};
use crate::{
    settings::{CommandAction, Settings, TabTitleBlock},
    theme::Theme,
};

impl Workspace {
    /// type a title of its own into tab `ix` of `pane_id`, starting from the one shown
    pub(super) fn start_rename(
        &mut self,
        pane_id: usize,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self.tab_title(pane_id, ix, cx);
        let id = self.view_at(pane_id, ix).entity_id();
        self.renaming = Some((id, rename_input("tab title", title, window, cx)));
        self.renaming_name = false;
        cx.notify();
    }

    /// type the name `kuterm msg --pane` finds the active tab by
    pub(super) fn start_rename_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        let Some(tab) = pane.tabs.get(pane.active) else {
            return;
        };
        let (id, name) = (tab.view.entity_id(), tab.name.clone().unwrap_or_default());
        self.renaming = Some((id, rename_input("pane name", name, window, cx)));
        self.renaming_name = true;
        cx.notify();
    }

    /// stop renaming, keeping the typed title when `keep`. an empty one goes back to the
    /// `tab_title` blocks
    pub(super) fn finish_rename(
        &mut self,
        keep: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, input)) = self.renaming.take() else {
            return;
        };
        if keep && let Some((pane, ix)) = self.find_id(id) {
            let text = input.read(cx).content().trim().to_string();
            let name = self.renaming_name;
            let tab = &mut self.pane_mut(pane).expect("pane of found tab").tabs[ix];
            let text = (!text.is_empty()).then_some(text);
            if name {
                tab.name = text;
            } else {
                tab.custom_title = text;
            }
        }
        // typing goes back to the terminal
        self.focus_pane(self.focused, window, cx);
    }

    /// pin tab `ix` of `pane_id` after the other pinned tabs, or unpin it right after them
    pub(super) fn toggle_pin(&mut self, pane_id: usize, ix: usize, cx: &mut Context<Self>) {
        let Some(pane) = self.pane_mut(pane_id) else {
            return;
        };
        let others = pane
            .tabs
            .iter()
            .enumerate()
            .filter(|(other, tab)| *other != ix && tab.pinned)
            .count();
        pane.tabs[ix].pinned = !pane.tabs[ix].pinned;
        self.move_tab(pane_id, ix, others, cx);
        self.refresh_titles();
        cx.notify();
    }

    /// open a tab with the profile and folder of tab `ix` right after it
    pub(super) fn duplicate_tab(
        &mut self,
        pane_id: usize,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.pane(pane_id).and_then(|pane| pane.tabs.get(ix)) else {
            return;
        };
        let mut profile = Settings::get(cx).profile_or_default(&tab.profile).clone();
        if let Some(cwd) = tab_cwd(tab, cx).filter(|cwd| cwd.is_dir()) {
            profile.working_directory = Some(cwd);
        }
        let count = |this: &Self| this.pane(pane_id).map_or(0, |pane| pane.tabs.len());
        let before = count(self);
        self.add_profile_tab(pane_id, &profile, true, window, cx);
        if count(self) > before {
            self.move_tab(pane_id, before, ix + 1, cx);
        }
    }

    /// columns and rows of the active tab while some title shows them, (0, 0) otherwise
    pub(super) fn active_size(&self, cx: &App) -> (usize, usize) {
        let settings = Settings::get(cx);
        let status_bar = &settings.status_bar;
        let shown = settings
            .tab_title
            .iter()
            .chain(&settings.window_title)
            .chain(
                [&status_bar.left, &status_bar.right]
                    .into_iter()
                    .filter(|_| status_bar.enable)
                    .flatten(),
            )
            .any(|block| *block == TabTitleBlock::Size);
        let pane = self.focused_pane();
        match pane.tabs.get(pane.active) {
            Some(tab) if shown => terminal_size(tab.view.read(cx).terminal().read(cx)),
            _ => (0, 0),
        }
    }

    /// close the tab menu, typing goes back to the terminal
    fn close_tab_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_menu = None;
        self.focus_pane(self.focused, window, cx);
        cx.notify();
    }

    /// menu of a right clicked tab, its rows act on that tab
    pub(super) fn render_tab_menu(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let (id, position) = self.tab_menu?;
        let (pane, ix) = self.find_id(id)?;
        let theme = Theme::get(cx);
        let pinned = self.pane(pane)?.tabs[ix].pinned;
        let grouped = self.pane(pane)?.tabs[ix].group.is_some();
        let mut rows: Vec<Option<(&str, CommandAction)>> = vec![
            Some(("rename", CommandAction::RenameTab)),
            Some((if pinned { "unpin" } else { "pin" }, CommandAction::PinTab)),
            Some(("duplicate", CommandAction::DuplicateTab)),
            Some(("copy folder path", CommandAction::CopyFolder)),
            Some(("move to new window", CommandAction::MoveTabToNewWindow)),
            Some(if grouped {
                ("remove from group", CommandAction::RemoveTabFromGroup)
            } else {
                ("add to new group", CommandAction::NewGroup)
            }),
            None,
            Some(("split right", CommandAction::SplitRight)),
            Some(("split down", CommandAction::SplitDown)),
            None,
            Some(("close", CommandAction::CloseTab)),
            Some(("close other tabs", CommandAction::CloseOtherTabs)),
            Some(("close tabs to the right", CommandAction::CloseTabsToRight)),
            Some(("close tabs to the left", CommandAction::CloseTabsToLeft)),
        ];
        if grouped {
            rows.push(Some(("close group", CommandAction::CloseGroup)));
        }
        let rows = rows.into_iter().enumerate().map(|(row, item)| {
            let Some((label, action)) = item else {
                return div().my_1().h(px(1.)).bg(theme.border).into_any_element();
            };
            let selector = format!("tab-menu-{}", label.replace(' ', "-"));
            menu_item(("tab-menu", row), label, theme)
                .debug_selector(move || selector.clone())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.tab_menu = None;
                    // the menu acts on its tab, which actions find as the active one
                    if let Some((pane, ix)) = this.find_id(id) {
                        this.activate_tab(pane, ix, window, cx);
                        this.run_action(&super::RunAction(action.clone()), window, cx);
                    }
                }))
                .into_any_element()
        });
        let menu = popup_menu("tab-menu", theme)
            .children(rows)
            .track_focus(&self.tab_menu_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // every key stays with the menu, escape closes it
                cx.stop_propagation();
                if event.keystroke.key == "escape" {
                    this.close_tab_menu(window, cx);
                }
            }))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.close_tab_menu(window, cx);
            }));
        Some(
            deferred(anchored().position(position).snap_to_window().child(menu)).into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {

    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn double_click_renames_a_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let at = tab_center(cx, 1);
        for count in [1, 2] {
            click(cx, at, count);
            cx.simulate_event(MouseUpEvent {
                position: at,
                modifiers: Modifiers::default(),
                button: MouseButton::Left,
                click_count: count,
            });
        }
        cx.run_until_parked();
        assert!(cx.debug_bounds("rename-tab-1").is_some());
        // the shown title is selected, so typing replaces it
        type_keys(cx, "w o r k enter");
        assert_eq!(custom_title(&ws, cx, 1).as_deref(), Some("work"));
        assert_eq!(
            ws.update(cx, |ws, cx| ws.tab_title(ws.focused, 1, cx)),
            "work"
        );
        assert!(cx.debug_bounds("rename-tab-1").is_none());
        // typing goes back to the terminal
        assert_eq!(current(&ws, cx), 1);

        run_actions(&ws, cx, r#""rename_tab""#);
        type_keys(cx, "x escape");
        assert_eq!(custom_title(&ws, cx, 1).as_deref(), Some("work"));
        // an empty title goes back to the blocks
        run_actions(&ws, cx, r#""rename_tab""#);
        type_keys(cx, "backspace enter");
        assert_eq!(custom_title(&ws, cx, 1), None);
    }

    #[gpui::test]
    fn pinned_tabs_stay_in_front_and_survive_closing_others(cx: &mut TestAppContext) {
        let settings = settings_of(&[r#"{"drag_tabs": true, "show_tab_close_button": true}"#]);
        let (ws, cx) = open_with_settings(cx, 4, "{}", settings);
        let ids = views(&ws, cx);
        run_actions(&ws, cx, r#"{"activate_tab": 3}, "pin_tab""#);
        assert_eq!(views(&ws, cx), [ids[2], ids[0], ids[1], ids[3]]);
        assert_eq!(current(&ws, cx), 0);
        assert!(cx.debug_bounds("close-tab-0").is_none());
        assert!(cx.debug_bounds("close-tab-1").is_some());
        // dragging can't put a tab in front of a pinned one, nor a pinned one after it
        ws.update(cx, |ws, cx| ws.move_tab(ws.focused, 3, 0, cx));
        assert_eq!(views(&ws, cx), [ids[2], ids[3], ids[0], ids[1]]);
        ws.update(cx, |ws, cx| ws.move_tab(ws.focused, 0, 3, cx));
        assert_eq!(views(&ws, cx)[0], ids[2]);
        // a middle click leaves it open
        let at = tab_center(cx, 0);
        cx.simulate_event(MouseDownEvent {
            position: at,
            modifiers: Modifiers::default(),
            button: MouseButton::Middle,
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 4);

        run_actions(&ws, cx, r#"{"activate_tab": 3}, "close_other_tabs""#);
        assert_eq!(views(&ws, cx), [ids[2], ids[0]]);
        run_actions(&ws, cx, r#"{"activate_tab": 1}, "pin_tab""#);
        assert!(ws.update(cx, |ws, _| ws.focused_pane().tabs.iter().all(|t| !t.pinned)));
        assert_eq!(views(&ws, cx), [ids[2], ids[0]]);
    }

    #[gpui::test]
    fn tab_menu_acts_on_the_clicked_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let ids = views(&ws, cx);
        let at = tab_center(cx, 0);
        right_click(cx, at);
        assert!(cx.debug_bounds("tab-menu").is_some());
        let duplicate = cx.debug_bounds("tab-menu-duplicate").unwrap().center();
        click(cx, duplicate, 1);
        release(cx, duplicate);
        cx.run_until_parked();
        assert!(cx.debug_bounds("tab-menu").is_none());
        // the copy opens right after the clicked tab, with its profile
        let now = views(&ws, cx);
        assert_eq!(now.len(), 3);
        assert_eq!((now[0], now[2]), (ids[0], ids[1]));
        assert_eq!(current(&ws, cx), 1);

        let at = tab_center(cx, 2);
        right_click(cx, at);
        let pin = cx.debug_bounds("tab-menu-pin").unwrap().center();
        click(cx, pin, 1);
        release(cx, pin);
        cx.run_until_parked();
        assert_eq!(views(&ws, cx)[0], ids[1]);
        let at = tab_center(cx, 0);
        right_click(cx, at);
        assert!(cx.debug_bounds("tab-menu-unpin").is_some());
    }
}
