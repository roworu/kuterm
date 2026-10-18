//! tab groups: named, colored runs of tabs under a chip that collapses them, made by hand or
//! one per git project the tabs are in

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::PathBuf,
};

use gpui::{
    AnyElement, Context, EntityId, FontWeight, MouseButton, MouseDownEvent, Pixels, Point, Window,
    anchored, deferred, div, prelude::*, px, rems,
};

use super::{
    Workspace,
    tab_bar::{DraggedTab, menu_item, popup_menu, rename_field, rename_input},
};
use crate::{
    settings::{CommandAction, Settings},
    theme::Theme,
};

/// what a row of the group menu does to its group
type GroupAction = fn(&mut Workspace, usize, &mut Window, &mut Context<Workspace>);

// group colors are these ansi colors of the theme: red, green, yellow, blue, magenta, cyan
const GROUP_COLORS: [usize; 6] = [1, 2, 3, 4, 5, 6];

// pixels the line under a group's chip and tabs is thick
const GROUP_LINE: f32 = 2.;

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

/// part of a tab another tab is dragged over, like in chrome: its edges put the dragged tab
/// next to it, its middle groups the two
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum TabDrop {
    Before,
    Onto,
    After,
}

impl TabDrop {
    /// the part of a tab `width` wide that `x`, measured from its left edge, is over
    pub(super) fn at(x: f32, width: f32) -> Self {
        if x < width / 4. {
            Self::Before
        } else if x > width * 3. / 4. {
            Self::After
        } else {
            Self::Onto
        }
    }
}

/// group a dropped tab ends up in
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Join {
    Group(usize),
    /// a new group with the tab it was dropped onto
    New,
    Ungrouped,
}

/// where a tab dropped on tab `target` of a row with `groups` lands, as the index `move_tab`
/// takes (`from` is its index when it comes from the same row), and the group it joins.
/// the left edge of a grouped tab is inside its group, even the first one's, which sits right
/// after the chip; the right edge is only inside when the next tab is in the group too, so
/// dropping past the last tab of a group leaves it. none when nothing changes
pub(super) fn plan_drop(
    groups: &[Option<usize>],
    from: Option<usize>,
    target: usize,
    zone: TabDrop,
) -> Option<(usize, Join)> {
    let join = match zone {
        TabDrop::Onto if from == Some(target) => return None,
        TabDrop::Onto => groups[target].map_or(Join::New, Join::Group),
        TabDrop::Before => groups[target].map_or(Join::Ungrouped, Join::Group),
        TabDrop::After => {
            let next = (target + 1..groups.len()).find(|&ix| Some(ix) != from);
            match (groups[target], next.and_then(|ix| groups[ix])) {
                (Some(group), Some(next)) if group == next => Join::Group(group),
                _ => Join::Ungrouped,
            }
        }
    };
    let gap = if zone == TabDrop::Before {
        target
    } else {
        target + 1
    };
    // the tab leaves its place first, so a gap after it is one to the left
    let to = match from {
        Some(from) if gap > from => gap - 1,
        _ => gap,
    };
    Some((to, join))
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

/// color of a group named `name`, the same name always gets the same one
pub(super) fn color_for(name: &str) -> usize {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish() as usize % GROUP_COLORS.len()
}

/// the group colored line along the bottom of the tab bar, from its left to its right edge
pub(super) fn group_line(color: gpui::Hsla) -> gpui::Div {
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .h(px(GROUP_LINE))
        .bg(color)
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

    /// drop the tab `src_ix` of `src_pane` on tab `target` of `dst_pane`, like chrome: onto a
    /// tab groups them, between tabs it joins the group around the gap or leaves its own
    pub(super) fn drop_tab_on_tab(
        &mut self,
        (src_pane, src_ix): (usize, usize),
        (dst_pane, target): (usize, usize),
        zone: TabDrop,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dst) = self.pane(dst_pane) else {
            return;
        };
        let groups: Vec<Option<usize>> = dst.tabs.iter().map(|tab| tab.group).collect();
        let from = (src_pane == dst_pane).then_some(src_ix);
        let Some((to, join)) = plan_drop(&groups, from, target, zone) else {
            return;
        };
        let group = match join {
            Join::Group(group) => Some(group),
            Join::Ungrouped => None,
            Join::New => {
                // unnamed like chrome's, its chip is a dot until it is renamed
                let color = self.groups.len() % GROUP_COLORS.len();
                let id = self.add_group(String::new(), color, None);
                if let Some(tab) = self
                    .pane_mut(dst_pane)
                    .and_then(|pane| pane.tabs.get_mut(target))
                {
                    tab.group = Some(id);
                }
                Some(id)
            }
        };
        // the group is set before the move, so regrouping keeps the tab where it lands
        let Some(tab) = self
            .pane_mut(src_pane)
            .and_then(|pane| pane.tabs.get_mut(src_ix))
        else {
            return;
        };
        tab.group = group;
        self.move_tab_to_pane(src_pane, src_ix, dst_pane, to, window, cx);
        for pane in [src_pane, dst_pane] {
            self.regroup(pane);
        }
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
                crate::ui::dialogs::CloseTarget::Tabs(ids),
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
        self.renaming_group = Some((group, rename_input("group name", name, window, cx)));
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
        let label = match (group.collapsed, group.name.is_empty()) {
            (true, true) => format!("({count})"),
            (true, false) => format!("{} ({count})", group.name),
            (false, _) => group.name.clone(),
        };
        let named = renaming.is_some() || !label.is_empty();
        let content = match renaming {
            Some(input) => {
                rename_field(input, Workspace::finish_group_rename, cx).into_any_element()
            }
            // an unnamed group shows a dot in its color, like chrome
            None if label.is_empty() => div()
                .size(rems(0.625))
                .rounded_full()
                .bg(color)
                .into_any_element(),
            None => div().child(label).into_any_element(),
        };
        let chip = div()
            .id(("group-chip", id))
            .debug_selector(move || format!("group-chip-{id}"))
            .flex()
            .flex_none()
            .items_center()
            .my_1()
            .when(named, |chip| {
                chip.px_2()
                    .rounded_md()
                    .bg(color)
                    .text_color(theme.terminal_background)
                    .font_weight(FontWeight::SEMIBOLD)
            })
            .when(!named, |chip| chip.px_1())
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
            }));
        // chrome's line starts under the chip and runs on under the group's tabs, a collapsed
        // group has none
        Some(
            div()
                .relative()
                .flex()
                .flex_none()
                .h_full()
                .px_1()
                .when(!group.collapsed, |wrap| {
                    wrap.child(group_line(color).left(px(4.)).rounded_l_sm())
                })
                .child(chip)
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

    /// color the next group made gets
    pub(super) fn next_group_color(&self, cx: &Context<Self>) -> gpui::Hsla {
        Theme::get(cx).ansi[GROUP_COLORS[self.groups.len() % GROUP_COLORS.len()]]
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

    /// group name and color of a tab kept by hand, for a tab moving to another window
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
    use super::*;
    use super::{Join, TabDrop, color_for, plan_drop, together};
    use crate::ui::workspace::test_support::*;

    #[test]
    fn tab_edges_reorder_and_its_middle_groups() {
        assert_eq!(TabDrop::at(10., 200.), TabDrop::Before);
        assert_eq!(TabDrop::at(100., 200.), TabDrop::Onto);
        assert_eq!(TabDrop::at(190., 200.), TabDrop::After);
    }

    #[test]
    fn drops_join_the_group_around_the_gap() {
        // - a a -
        let row = [None, Some(1), Some(1), None];
        // onto an ungrouped tab makes a new group, onto a grouped one joins it, next to it
        assert_eq!(
            plan_drop(&row, Some(3), 0, TabDrop::Onto),
            Some((1, Join::New))
        );
        assert_eq!(
            plan_drop(&row, Some(0), 1, TabDrop::Onto),
            Some((1, Join::Group(1)))
        );
        // the left edge of the first tab of a group is right after its chip, so inside
        assert_eq!(
            plan_drop(&row, Some(3), 1, TabDrop::Before),
            Some((1, Join::Group(1)))
        );
        // between two tabs of the group is inside, past its last tab is outside
        assert_eq!(
            plan_drop(&row, Some(0), 1, TabDrop::After),
            Some((1, Join::Group(1)))
        );
        assert_eq!(
            plan_drop(&row, Some(0), 2, TabDrop::After),
            Some((2, Join::Ungrouped))
        );
        assert_eq!(
            plan_drop(&row, Some(1), 0, TabDrop::Before),
            Some((0, Join::Ungrouped))
        );
        // the dragged tab itself is skipped when looking past the target, so the last tab of a
        // group dropped on its own spot is past the group and leaves it
        assert_eq!(
            plan_drop(&row, Some(2), 1, TabDrop::After),
            Some((2, Join::Ungrouped))
        );
        // from another pane nothing leaves first, and a tab onto itself does nothing
        assert_eq!(
            plan_drop(&row, None, 2, TabDrop::After),
            Some((3, Join::Ungrouped))
        );
        assert_eq!(plan_drop(&row, Some(2), 2, TabDrop::Onto), None);
    }

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
    fn group_colors_are_stable() {
        assert_eq!(color_for("kuterm"), color_for("kuterm"));
    }

    #[gpui::test]
    fn dragging_tabs_onto_each_other_groups_them_like_chrome(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 4);
        let old = views(&ws, cx);
        let groups = |ws: &Entity<Workspace>, cx: &mut VisualTestContext| {
            ws.update(cx, |ws, _| {
                ws.pane(0)
                    .unwrap()
                    .tabs
                    .iter()
                    .map(|tab| tab.group)
                    .collect::<Vec<_>>()
            })
        };
        // the fourth tab dropped onto the first: a new unnamed group of the two, at the front
        ws.update_in(cx, |ws, window, cx| {
            ws.drop_tab_on_tab((0, 3), (0, 0), groups::TabDrop::Onto, window, cx)
        });
        let group = groups(&ws, cx)[0].expect("a group");
        assert_eq!(groups(&ws, cx), [Some(group), Some(group), None, None]);
        assert_eq!(views(&ws, cx), [old[0], old[3], old[1], old[2]]);
        assert_eq!(ws.update(cx, |ws, _| ws.groups[0].name.clone()), "");
        // the third tab dropped on the left edge of the group's second tab joins the group there
        ws.update_in(cx, |ws, window, cx| {
            ws.drop_tab_on_tab((0, 3), (0, 1), groups::TabDrop::Before, window, cx)
        });
        assert_eq!(
            groups(&ws, cx),
            [Some(group), Some(group), Some(group), None]
        );
        assert_eq!(views(&ws, cx), [old[0], old[2], old[3], old[1]]);
        // dropped past the group's last tab it leaves the group
        ws.update_in(cx, |ws, window, cx| {
            ws.drop_tab_on_tab((0, 0), (0, 2), groups::TabDrop::After, window, cx)
        });
        assert_eq!(groups(&ws, cx), [Some(group), Some(group), None, None]);
        assert_eq!(views(&ws, cx), [old[2], old[3], old[0], old[1]]);
    }

    #[gpui::test]
    fn a_tab_joining_from_afar_moves_next_to_its_group(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 4);
        let old = views(&ws, cx);
        let groups = |ws: &Entity<Workspace>, cx: &mut VisualTestContext| {
            ws.update(cx, |ws, _| {
                let tabs = &ws.pane(0).unwrap().tabs;
                tabs.iter().map(|tab| tab.group).collect::<Vec<_>>()
            })
        };
        // the first two tabs make a group
        activate(&ws, cx, 0);
        run_actions(&ws, cx, r#""new_group""#);
        let group = groups(&ws, cx)[0].expect("a group");
        activate(&ws, cx, 1);
        run_actions(&ws, cx, r#""add_tab_to_group""#);
        assert_eq!(groups(&ws, cx), [Some(group), Some(group), None, None]);
        // the last tab dropped on the group's chip joins it right after its tabs, and the
        // active tab stays active wherever it moved
        activate(&ws, cx, 2);
        ws.update(cx, |ws, cx| ws.join_group(0, 3, group, cx));
        assert_eq!(views(&ws, cx), [old[0], old[1], old[3], old[2]]);
        assert_eq!(
            groups(&ws, cx),
            [Some(group), Some(group), Some(group), None]
        );
        assert_eq!(ws.update(cx, |ws, _| ws.pane(0).unwrap().active), 3);

        // the first tab of the group leaves it to the right of the group's run
        activate(&ws, cx, 0);
        run_actions(&ws, cx, r#""remove_tab_from_group""#);
        assert_eq!(views(&ws, cx), [old[1], old[3], old[0], old[2]]);
        assert_eq!(groups(&ws, cx), [Some(group), Some(group), None, None]);
        assert_eq!(current(&ws, cx), 2);
    }

    #[gpui::test]
    fn an_unnamed_chip_takes_dropped_tabs_and_counts_them_when_collapsed(cx: &mut TestAppContext) {
        let settings = settings_of(&[r#"{"drag_tabs": true}"#]);
        let (ws, cx) = open_with_settings(cx, 4, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        // the fourth tab onto the first makes an unnamed group of the two
        ws.update_in(cx, |ws, window, cx| {
            ws.drop_tab_on_tab((0, 3), (0, 0), groups::TabDrop::Onto, window, cx)
        });
        cx.run_until_parked();
        let group = ws.update(cx, |ws, _| ws.groups[0].id);
        let chip: &'static str = format!("group-chip-{group}").leak();
        let dot = cx.debug_bounds(chip).expect("no chip");
        // an ungrouped tab dragged onto the chip joins the group
        let last = cx.debug_bounds("tab-title-2").unwrap().center();
        drag_to(cx, last, dot.center());
        let count = |ws: &Entity<Workspace>, cx: &mut VisualTestContext| {
            ws.update(cx, |ws, _| {
                let tabs = &ws.pane(0).unwrap().tabs;
                tabs.iter().filter(|tab| tab.group == Some(group)).count()
            })
        };
        assert_eq!(count(&ws, cx), 3);
        // collapsed, the dot gives way to the number of its tabs
        let at = cx.debug_bounds(chip).unwrap().center();
        click(cx, at, 1);
        release(cx, at);
        cx.run_until_parked();
        assert!(ws.update(cx, |ws, _| ws.groups[0].collapsed));
        let collapsed = cx.debug_bounds(chip).unwrap();
        assert!(
            collapsed.size.width > dot.size.width * 2.,
            "{dot:?} {collapsed:?}"
        );

        // its menu goes away with a click anywhere else
        right_click(cx, collapsed.center());
        assert!(ws.update(cx, |ws, _| ws.group_menu.is_some()));
        let terminal = cx.debug_bounds("pane-terminal-0").unwrap().center();
        click(cx, terminal, 1);
        release(cx, terminal);
        cx.run_until_parked();
        assert!(ws.update(cx, |ws, _| ws.group_menu.is_none()));
        assert!(ws.update(cx, |ws, _| ws.groups[0].collapsed));
    }
}
