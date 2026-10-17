//! tab groups: named, colored runs of tabs under a chip that collapses them, made by hand or
//! one per git project the tabs are in

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use gpui::{
    AnyElement, Context, EntityId, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, Pixels,
    Point, Window, anchored, deferred, div, prelude::*, rems,
};

use super::{
    Workspace,
    tab_bar::{DraggedTab, menu_item, popup_menu},
};
use crate::{
    settings::{CommandAction, Settings},
    theme::Theme,
    ui::text_input::TextInput,
};

/// what a row of the group menu does to its group
type GroupAction = fn(&mut Workspace, usize, &mut Window, &mut Context<Workspace>);

// group colors are these ansi colors of the theme: red, green, yellow, blue, magenta, cyan
const GROUP_COLORS: [usize; 6] = [1, 2, 3, 4, 5, 6];

/// named, colored run of tabs in a pane
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Group {
    pub(super) id: usize,
    pub(super) name: String,
    /// index into `GROUP_COLORS`
    pub(super) color: usize,
    pub(super) collapsed: bool,
    /// git root the group stands for when it was made by `tab_groups.by_project`
    pub(super) auto_root: Option<PathBuf>,
}

/// order of tabs, given as their groups, that keeps each group's tabs together where its
/// first tab is, every other tab staying in its order
pub(super) fn together(groups: &[Option<usize>]) -> Vec<usize> {
    let mut order = Vec::with_capacity(groups.len());
    for (ix, group) in groups.iter().enumerate() {
        if order.contains(&ix) {
            continue;
        }
        match group {
            Some(group) => {
                order.extend((ix..groups.len()).filter(|&other| groups[other] == Some(*group)))
            }
            None => order.push(ix),
        }
    }
    order
}

/// the git repository `path` is in, found by walking up to a folder with `.git`
pub(super) fn project_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// color of a group named `name`, the same name always gets the same one
pub(super) fn color_for(name: &str) -> usize {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish() as usize % GROUP_COLORS.len()
}

impl Group {
    /// the theme color of the group
    pub(super) fn hsla(&self, theme: &Theme) -> gpui::Hsla {
        theme.ansi[GROUP_COLORS[self.color % GROUP_COLORS.len()]]
    }
}

impl Workspace {
    fn group(&self, id: usize) -> Option<&Group> {
        self.groups.iter().find(|group| group.id == id)
    }

    fn group_mut(&mut self, id: usize) -> Option<&mut Group> {
        self.groups.iter_mut().find(|group| group.id == id)
    }

    fn add_group(&mut self, name: String, color: usize, auto_root: Option<PathBuf>) -> usize {
        let id = self.next_group_id;
        self.next_group_id += 1;
        self.groups.push(Group {
            id,
            name,
            color,
            collapsed: false,
            auto_root,
        });
        id
    }

    /// group of the active tab of the focused pane
    fn active_group(&self) -> Option<usize> {
        let pane = self.focused_pane();
        pane.tabs.get(pane.active)?.group
    }

    /// put the tabs of each group of `pane_id` next to each other and forget empty groups
    pub(super) fn regroup(&mut self, pane_id: usize) {
        if let Some(pane) = self.pane_mut(pane_id) {
            let groups: Vec<Option<usize>> = pane.tabs.iter().map(|tab| tab.group).collect();
            let order = together(&groups);
            if order.iter().enumerate().any(|(at, &ix)| at != ix) {
                let active = order.iter().position(|&ix| ix == pane.active).unwrap_or(0);
                let mut tabs: Vec<Option<super::Tab>> = std::mem::take(&mut pane.tabs)
                    .into_iter()
                    .map(Some)
                    .collect();
                pane.tabs = order.iter().filter_map(|&ix| tabs[ix].take()).collect();
                pane.active = active;
            }
        }
        let used: Vec<usize> = self
            .layout
            .panes()
            .iter()
            .flat_map(|pane| pane.tabs.iter().filter_map(|tab| tab.group))
            .collect();
        self.groups.retain(|group| used.contains(&group.id));
        self.refresh_titles();
    }

    /// a new group with the active tab in it
    pub(super) fn new_group(&mut self, cx: &mut Context<Self>) {
        let name = format!("group {}", self.next_group_id + 1);
        let color = self.groups.len() % GROUP_COLORS.len();
        let id = self.add_group(name, color, None);
        let focused = self.focused;
        let pane = self.pane_mut(focused).expect("focused pane");
        let active = pane.active;
        pane.tabs[active].group = Some(id);
        self.regroup(focused);
        cx.notify();
    }

    /// put the active tab into the group of the tab before it, or after it, or a new one
    pub(super) fn add_tab_to_group(&mut self, cx: &mut Context<Self>) {
        let pane = self.focused_pane();
        let neighbour = pane
            .active
            .checked_sub(1)
            .and_then(|ix| pane.tabs[ix].group)
            .or_else(|| pane.tabs.get(pane.active + 1).and_then(|tab| tab.group));
        let Some(group) = neighbour else {
            self.new_group(cx);
            return;
        };
        self.join_group(self.focused, self.focused_pane().active, group, cx);
    }

    fn join_group(&mut self, pane_id: usize, ix: usize, group: usize, cx: &mut Context<Self>) {
        let Some(tab) = self
            .pane_mut(pane_id)
            .and_then(|pane| pane.tabs.get_mut(ix))
        else {
            return;
        };
        tab.group = Some(group);
        self.regroup(pane_id);
        cx.notify();
    }

    /// take the active tab out of its group
    pub(super) fn remove_tab_from_group(&mut self, cx: &mut Context<Self>) {
        let focused = self.focused;
        let pane = self.pane_mut(focused).expect("focused pane");
        let active = pane.active;
        let Some(group) = pane.tabs[active].group.take() else {
            return;
        };
        // it leaves the group's run, right after it
        let last = pane.tabs.iter().rposition(|tab| tab.group == Some(group));
        if let Some(last) = last.filter(|&last| last > active) {
            self.move_tab(focused, active, last, cx);
        }
        self.regroup(focused);
        cx.notify();
    }

    /// close every tab of group `group`, asking first like closing them one by one
    pub(super) fn close_group(
        &mut self,
        group: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.confirm_close.is_some() {
            return;
        }
        let warn = Settings::get(cx).close_running_tab_warn;
        let mut ids: Vec<EntityId> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for pane in self.layout.panes() {
            for (ix, tab) in pane.tabs.iter().enumerate() {
                if tab.group != Some(group) {
                    continue;
                }
                ids.push(tab.view.entity_id());
                names.extend(
                    warn.then(|| self.running_program(pane.id, ix, cx))
                        .flatten(),
                );
            }
        }
        // one question for the whole group, a dialog per tab would drop the rest
        if names.is_empty() {
            self.close_tabs(&ids, window, cx);
        } else {
            self.open_confirm(
                super::side_close_message(&names),
                crate::ui::command_palette::CloseTarget::Tabs(ids),
                window,
                cx,
            );
        }
    }

    /// close the group of the active tab, like every tab of its project
    pub(super) fn close_active_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(group) = self.active_group() {
            self.close_group(group, window, cx);
        }
    }

    /// show or hide the tabs of group `group` behind its chip
    fn toggle_collapsed(&mut self, group: usize, cx: &mut Context<Self>) {
        if let Some(group) = self.group_mut(group) {
            group.collapsed = !group.collapsed;
        }
        cx.notify();
    }

    fn ungroup(&mut self, group: usize, cx: &mut Context<Self>) {
        for pane in self.layout.pane_ids() {
            for tab in self
                .pane_mut(pane)
                .map_or(&mut [][..], |pane| &mut pane.tabs[..])
            {
                if tab.group == Some(group) {
                    tab.group = None;
                }
            }
        }
        self.groups.retain(|other| other.id != group);
        cx.notify();
    }

    fn next_color(&mut self, group: usize, cx: &mut Context<Self>) {
        if let Some(group) = self.group_mut(group) {
            group.color = (group.color + 1) % GROUP_COLORS.len();
        }
        cx.notify();
    }

    fn start_group_rename(&mut self, group: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.group(group).map(|group| group.name.clone()) else {
            return;
        };
        let input = cx.new(|cx| {
            let mut input = TextInput::new("group name", cx);
            input.set_content(name, cx);
            input
        });
        input.focus_handle(cx).focus(window, cx);
        self.renaming_group = Some((group, input));
        cx.notify();
    }

    fn finish_group_rename(&mut self, keep: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((group, input)) = self.renaming_group.take() else {
            return;
        };
        let name = input.read(cx).content().trim().to_string();
        if keep
            && !name.is_empty()
            && let Some(group) = self.group_mut(group)
        {
            group.name = name;
            // a name of its own makes it a group kept by hand
            group.auto_root = None;
        }
        self.focus_pane(self.focused, window, cx);
    }

    /// move tabs into the group of the git project they are in, as found on the last title
    /// refresh, when `tab_groups.by_project` is on. groups made by hand are left alone
    pub(super) fn group_by_project(&mut self, cx: &mut Context<Self>) -> bool {
        if !Settings::get(cx).tab_groups.by_project {
            return false;
        }
        let mut moves = Vec::new();
        for pane in self.layout.panes() {
            for (ix, tab) in pane.tabs.iter().enumerate() {
                let group = tab.group.and_then(|id| self.group(id));
                if group.is_some_and(|group| group.auto_root.is_none()) {
                    continue;
                }
                let current = group.and_then(|group| group.auto_root.clone());
                if current != tab.project {
                    moves.push((pane.id, ix, tab.project.clone()));
                }
            }
        }
        let changed = !moves.is_empty();
        let mut panes = Vec::new();
        for (pane_id, ix, project) in moves {
            let group = project.map(|root| {
                match self
                    .groups
                    .iter()
                    .find(|group| group.auto_root.as_ref() == Some(&root))
                {
                    Some(group) => group.id,
                    None => {
                        let name = root.file_name().map_or_else(
                            || root.display().to_string(),
                            |name| name.to_string_lossy().into_owned(),
                        );
                        let color = color_for(&name);
                        self.add_group(name, color, Some(root))
                    }
                }
            });
            if let Some(tab) = self
                .pane_mut(pane_id)
                .and_then(|pane| pane.tabs.get_mut(ix))
            {
                tab.group = group;
            }
            panes.push(pane_id);
        }
        panes.dedup();
        for pane in panes {
            self.regroup(pane);
        }
        changed
    }

    /// true when tab `ix` of `pane_id` is hidden behind the chip of its collapsed group
    pub(super) fn hidden_in_group(&self, pane_id: usize, ix: usize) -> bool {
        let Some(pane) = self.pane(pane_id) else {
            return false;
        };
        // the active tab always shows
        ix != pane.active
            && pane.tabs[ix]
                .group
                .and_then(|id| self.group(id))
                .is_some_and(|group| group.collapsed)
    }

    /// chip before the first tab of its group, none for other tabs
    pub(super) fn render_group_chip(
        &self,
        pane_id: usize,
        ix: usize,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let pane = self.pane(pane_id)?;
        let id = pane.tabs[ix].group?;
        if ix > 0 && pane.tabs[ix - 1].group == Some(id) {
            return None;
        }
        let group = self.group(id)?;
        let theme = Theme::get(cx);
        let color = group.hsla(theme);
        let renaming = self
            .renaming_group
            .as_ref()
            .filter(|(renamed, _)| *renamed == id)
            .map(|(_, input)| input.clone());
        let count = pane.tabs.iter().filter(|tab| tab.group == Some(id)).count();
        let label = if group.collapsed {
            format!("{} ({count})", group.name)
        } else {
            group.name.clone()
        };
        let content = match renaming {
            Some(input) => div()
                .min_w(rems(6.))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "enter" => this.finish_group_rename(true, window, cx),
                        "escape" => this.finish_group_rename(false, window, cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                }))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(
                    cx.listener(|this, _, window, cx| this.finish_group_rename(true, window, cx)),
                )
                .child(input)
                .into_any_element(),
            None => div().child(label).into_any_element(),
        };
        Some(
            div()
                .id(("group-chip", id))
                .debug_selector(move || format!("group-chip-{id}"))
                .flex()
                .flex_none()
                .items_center()
                .my_1()
                .mx_1()
                .px_2()
                .rounded_md()
                .bg(color)
                .text_color(theme.terminal_background)
                .whitespace_nowrap()
                .child(content)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_collapsed(id, cx)))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.group_menu = Some((id, event.position));
                        cx.notify();
                    }),
                )
                // a tab dropped on the chip joins the group
                .drag_over::<DraggedTab>(move |style, _, _, _| style.border_2().border_color(color))
                .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                    this.join_group(dragged.pane, dragged.ix, id, cx);
                }))
                .into_any_element(),
        )
    }

    /// menu of a right clicked group chip
    pub(super) fn render_group_menu(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let (id, position): (usize, Point<Pixels>) = self.group_menu?;
        let group = self.group(id)?;
        let theme = Theme::get(cx);
        let close = if group.auto_root.is_some() {
            "close all tabs of this project"
        } else {
            "close group"
        };
        let rows: [(&str, GroupAction); 5] = [
            ("rename", |this, id, window, cx| {
                this.start_group_rename(id, window, cx)
            }),
            (
                if group.collapsed {
                    "expand"
                } else {
                    "collapse"
                },
                |this, id, _, cx| this.toggle_collapsed(id, cx),
            ),
            ("change color", |this, id, _, cx| this.next_color(id, cx)),
            ("ungroup", |this, id, _, cx| this.ungroup(id, cx)),
            (close, |this, id, window, cx| {
                this.close_group(id, window, cx)
            }),
        ];
        let rows = rows.into_iter().enumerate().map(|(row, (label, run))| {
            let selector = format!("group-menu-{}", label.replace(' ', "-"));
            menu_item(("group-menu", row), label, theme)
                .debug_selector(move || selector.clone())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.group_menu = None;
                    run(this, id, window, cx);
                    cx.notify();
                }))
        });
        let menu = popup_menu("group-menu", theme)
            .children(rows)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.group_menu = None;
                cx.notify();
            }));
        Some(
            deferred(anchored().position(position).snap_to_window().child(menu)).into_any_element(),
        )
    }

    /// line under the tabs of a group in its color
    pub(super) fn group_color_of(
        &self,
        pane_id: usize,
        ix: usize,
        cx: &Context<Self>,
    ) -> Option<gpui::Hsla> {
        let id = self.pane(pane_id)?.tabs.get(ix)?.group?;
        Some(self.group(id)?.hsla(Theme::get(cx)))
    }

    /// group name and color of a tab kept by hand, for session restore
    pub(super) fn saved_group(&self, tab: &super::Tab) -> Option<(String, usize)> {
        let group = self.group(tab.group?)?;
        group
            .auto_root
            .is_none()
            .then(|| (group.name.clone(), group.color))
    }

    /// put a restored tab into the group named `name`, made when there is none yet
    pub(super) fn restore_group(&mut self, pane_id: usize, ix: usize, name: &str, color: usize) {
        let id = match self
            .groups
            .iter()
            .find(|group| group.auto_root.is_none() && group.name == name)
        {
            Some(group) => group.id,
            None => self.add_group(name.to_string(), color, None),
        };
        if let Some(tab) = self
            .pane_mut(pane_id)
            .and_then(|pane| pane.tabs.get_mut(ix))
        {
            tab.group = Some(id);
        }
    }

    /// the actions of the group commands
    pub(super) fn run_group_action(
        &mut self,
        action: &CommandAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            CommandAction::NewGroup => self.new_group(cx),
            CommandAction::AddTabToGroup => self.add_tab_to_group(cx),
            CommandAction::RemoveTabFromGroup => self.remove_tab_from_group(cx),
            CommandAction::CloseGroup => self.close_active_group(window, cx),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{color_for, project_root, together};

    #[test]
    fn groups_stay_together_where_their_first_tab_is() {
        // a b a - b : the second a joins the first, b's tabs follow
        let order = together(&[Some(1), Some(2), Some(1), None, Some(2)]);
        assert_eq!(order, [0, 2, 1, 4, 3]);
        assert_eq!(together(&[None, None]), [0, 1]);
        assert_eq!(together(&[None, Some(3), None, Some(3)]), [0, 1, 3, 2]);
        assert_eq!(together(&[]), Vec::<usize>::new());
    }

    #[test]
    fn projects_are_found_by_their_git_folder() {
        let root = std::env::temp_dir().join(format!("kuterm-groups-{}", std::process::id()));
        let deep = root.join("src/a/b");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert_eq!(project_root(&deep), Some(root.clone()));
        assert_eq!(project_root(&root), Some(root.clone()));
        std::fs::remove_dir_all(&root).ok();
        // the temp folder itself is in no repository
        assert_eq!(
            project_root(&std::env::temp_dir().join("no-such-project")),
            None
        );
        assert_eq!(color_for("kuterm"), color_for("kuterm"));
    }
}
