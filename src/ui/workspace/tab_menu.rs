//! things done to one tab: renaming it in place, pinning, duplicating, and the right click
//! menu listing them

use gpui::{AnyElement, App, Context, Focusable, Window, anchored, deferred, div, prelude::*, px};

use super::{
    Workspace,
    tab_bar::{menu_item, popup_menu},
    tab_cwd, terminal_size,
};
use crate::{
    settings::{CommandAction, Settings, TabTitleBlock},
    theme::Theme,
    ui::text_input::TextInput,
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
        let input = cx.new(|cx| {
            let mut input = TextInput::new("tab title", cx);
            input.set_content(title, cx);
            input
        });
        input.focus_handle(cx).focus(window, cx);
        self.renaming = Some((id, input));
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
        let input = cx.new(|cx| {
            let mut input = TextInput::new("pane name", cx);
            input.set_content(name, cx);
            input
        });
        input.focus_handle(cx).focus(window, cx);
        self.renaming = Some((id, input));
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
        let settings = Settings::get(cx);
        // a profile renamed or removed since falls back to the default one
        let mut profile = settings
            .profile(&tab.profile)
            .unwrap_or_else(|| settings.default_profile())
            .clone();
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
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.tab_menu = None;
                cx.notify();
            }));
        Some(
            deferred(anchored().position(position).snap_to_window().child(menu)).into_any_element(),
        )
    }
}
