//! the pane tree: splits and their ratios, moving focus and tabs between panes, dropping tabs on
//! pane edges

use super::*;

/// binary tree of panes, the leaves hold the tabs
pub(super) enum LayoutNode {
    /// placeholder while a node is replaced, never rendered
    Empty,
    Leaf(Pane),
    Split {
        axis: SplitAxis,
        /// share of the first child, the second gets the rest
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    pub(super) fn pane(&self, id: usize) -> Option<&Pane> {
        match self {
            Self::Leaf(pane) if pane.id == id => Some(pane),
            Self::Split { first, second, .. } => first.pane(id).or_else(|| second.pane(id)),
            Self::Empty | Self::Leaf(_) => None,
        }
    }

    pub(super) fn pane_mut(&mut self, id: usize) -> Option<&mut Pane> {
        match self {
            Self::Leaf(pane) if pane.id == id => Some(pane),
            Self::Split { first, second, .. } => first.pane_mut(id).or_else(|| second.pane_mut(id)),
            Self::Empty | Self::Leaf(_) => None,
        }
    }

    /// pane id and index of the tab whose view has this id
    pub(super) fn find_tab(&self, id: EntityId) -> Option<(usize, usize)> {
        match self {
            Self::Leaf(pane) => pane
                .tabs
                .iter()
                .position(|tab| tab.view.entity_id() == id)
                .map(|ix| (pane.id, ix)),
            Self::Split { first, second, .. } => first.find_tab(id).or_else(|| second.find_tab(id)),
            Self::Empty => None,
        }
    }

    pub(super) fn contains(&self, id: usize) -> bool {
        match self {
            Self::Leaf(pane) => pane.id == id,
            Self::Split { first, second, .. } => first.contains(id) || second.contains(id),
            Self::Empty => false,
        }
    }

    /// pane at the top right corner of the window, where window buttons go
    pub(super) fn corner_pane_id(&self) -> Option<usize> {
        match self {
            Self::Leaf(pane) => Some(pane.id),
            Self::Split {
                axis: SplitAxis::Horizontal,
                second,
                ..
            } => second.corner_pane_id(),
            Self::Split { first, .. } => first.corner_pane_id(),
            Self::Empty => None,
        }
    }

    pub(super) fn first_pane_id(&self) -> Option<usize> {
        match self {
            Self::Leaf(pane) => Some(pane.id),
            Self::Split { first, .. } => first.first_pane_id(),
            Self::Empty => None,
        }
    }

    /// panes in layout order
    pub(super) fn panes(&self) -> Vec<&Pane> {
        let mut panes = Vec::new();
        self.collect_panes(&mut panes);
        panes
    }

    pub(super) fn collect_panes<'a>(&'a self, panes: &mut Vec<&'a Pane>) {
        match self {
            Self::Leaf(pane) => panes.push(pane),
            Self::Split { first, second, .. } => {
                first.collect_panes(panes);
                second.collect_panes(panes);
            }
            Self::Empty => {}
        }
    }

    /// pane ids in layout order
    pub(super) fn pane_ids(&self) -> Vec<usize> {
        self.panes().iter().map(|pane| pane.id).collect()
    }

    /// split the leaf `target` at `side`, `new_pane` lands on that side
    pub(super) fn split_leaf(&mut self, target: usize, side: DropSide, new_pane: Pane) -> bool {
        match self {
            Self::Leaf(pane) if pane.id == target => {
                let Self::Leaf(pane) = std::mem::replace(self, Self::Empty) else {
                    unreachable!()
                };
                let (first, second) = match side {
                    DropSide::Left | DropSide::Top => (Self::Leaf(new_pane), Self::Leaf(pane)),
                    DropSide::Right | DropSide::Bottom => (Self::Leaf(pane), Self::Leaf(new_pane)),
                };
                *self = Self::Split {
                    axis: match side {
                        DropSide::Left | DropSide::Right => SplitAxis::Horizontal,
                        DropSide::Top | DropSide::Bottom => SplitAxis::Vertical,
                    },
                    ratio: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                };
                true
            }
            Self::Split { first, second, .. } => {
                if first.contains(target) {
                    first.split_leaf(target, side, new_pane)
                } else {
                    second.split_leaf(target, side, new_pane)
                }
            }
            Self::Empty | Self::Leaf(_) => false,
        }
    }

    /// remove the leaf `id`, a split that loses a direct leaf is replaced by its other side
    pub(super) fn remove_leaf(&mut self, id: usize) -> Option<Pane> {
        match self {
            Self::Leaf(pane) if pane.id == id => {
                let Self::Leaf(pane) = std::mem::replace(self, Self::Empty) else {
                    unreachable!()
                };
                Some(pane)
            }
            Self::Split { first, second, .. } => {
                let (found, other) = if first.contains(id) {
                    (first, second)
                } else {
                    (second, first)
                };
                let pane = found.remove_leaf(id)?;
                // a nested split already collapsed itself, only a direct leaf leaves a hole
                if matches!(**found, Self::Empty) {
                    *self = std::mem::replace(other.as_mut(), Self::Empty);
                }
                Some(pane)
            }
            Self::Empty | Self::Leaf(_) => None,
        }
    }
}

/// place of a pane in the window, as shares of its width and height
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PaneRect {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) w: f32,
    pub(super) h: f32,
}

/// how a split lays out its two children
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum SplitAxis {
    /// children side by side, divider is vertical
    Horizontal,
    /// children stacked, divider is horizontal
    Vertical,
}

/// edge of a pane a dropped tab splits it at
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum DropSide {
    Left,
    Right,
    Top,
    Bottom,
}

/// pane next to `from` on `side`: overlapping it the other way, closest first
pub(super) fn neighbor(rects: &[(usize, PaneRect)], from: usize, side: DropSide) -> Option<usize> {
    // shares of the window, a little slack for float sums of ratios
    const EPSILON: f32 = 1e-4;
    let (_, here) = *rects.iter().find(|(id, _)| *id == from)?;
    rects
        .iter()
        .filter(|(id, _)| *id != from)
        .filter_map(|(id, r)| {
            let (gap, overlap) = match side {
                DropSide::Left => (here.x - (r.x + r.w), overlap(here.y, here.h, r.y, r.h)),
                DropSide::Right => (r.x - (here.x + here.w), overlap(here.y, here.h, r.y, r.h)),
                DropSide::Top => (here.y - (r.y + r.h), overlap(here.x, here.w, r.x, r.w)),
                DropSide::Bottom => (r.y - (here.y + here.h), overlap(here.x, here.w, r.x, r.w)),
            };
            (gap > -EPSILON && overlap > EPSILON).then_some((*id, gap, overlap))
        })
        // nearest, then the one sharing the longest edge
        .min_by(|a, b| {
            (a.1, -a.2)
                .partial_cmp(&(b.1, -b.2))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(id, _, _)| id)
}

pub(super) fn overlap(a: f32, a_len: f32, b: f32, b_len: f32) -> f32 {
    (a + a_len).min(b + b_len) - a.max(b)
}

// share of the window one resize step moves a divider
pub(super) const RESIZE_STEP: f32 = 0.05;

/// side of `bounds` the pointer at `position` is closest to
pub(super) fn drop_side(bounds: Bounds<Pixels>, position: Point<Pixels>) -> DropSide {
    let dx = (position.x - bounds.left()) / bounds.size.width;
    let dy = (position.y - bounds.top()) / bounds.size.height;
    if (dx - 0.5).abs() > (dy - 0.5).abs() {
        if dx < 0.5 {
            DropSide::Left
        } else {
            DropSide::Right
        }
    } else if dy < 0.5 {
        DropSide::Top
    } else {
        DropSide::Bottom
    }
}

/// share of a split the smaller side keeps, so a divider cannot be dragged fully away
pub(super) const MIN_SPLIT_RATIO: f32 = 0.1;

/// node at `path`, false picks the first child and true the second
pub(super) fn layout_at_mut<'a>(
    node: &'a mut LayoutNode,
    path: &[bool],
) -> Option<&'a mut LayoutNode> {
    match path.split_first() {
        None => Some(node),
        Some((&first, rest)) => {
            let LayoutNode::Split {
                first: a,
                second: b,
                ..
            } = node
            else {
                return None;
            };
            layout_at_mut(if first { b } else { a }, rest)
        }
    }
}

/// the default profile for a split next to `tab`, in its folder when splits inherit it
pub(super) fn split_profile(tab: Option<&Tab>, cx: &App) -> Profile {
    let mut profile = Settings::get(cx).default_profile().clone();
    if Settings::get(cx).panes.split_inherits_cwd
        && let Some(cwd) = tab.and_then(|tab| tab_cwd(tab, cx))
    {
        profile.working_directory = Some(cwd);
    }
    profile
}

impl Workspace {
    /// update the split edge a dragged tab would drop at
    pub(super) fn update_drop_zone(
        &mut self,
        pane_id: usize,
        position: Point<Pixels>,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        // some pane always contains the pointer, so a pane only ever clears its own zone
        if bounds.contains(&position) {
            let zone = Some((pane_id, drop_side(bounds, position)));
            if self.drop_zone != zone {
                self.drop_zone = zone;
                cx.notify();
            }
        } else if self.drop_zone.is_some_and(|(id, _)| id == pane_id) {
            self.drop_zone = None;
            cx.notify();
        }
    }

    /// drop the tab dragged from `src_pane` onto pane `pane_id`, splitting it
    pub(super) fn drop_tab_on_pane(
        &mut self,
        pane_id: usize,
        src_pane: usize,
        src_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((zone_pane, side)) = self.drop_zone.take() else {
            return;
        };
        if zone_pane != pane_id {
            return;
        }
        self.split_with_tab(src_pane, src_ix, pane_id, side, window, cx);
    }

    /// move the tab at `src_ix` of `src_pane` into a new pane split off `target` at `side`
    pub(super) fn split_with_tab(
        &mut self,
        src_pane: usize,
        src_ix: usize,
        target: usize,
        side: DropSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((tab, empty)) = self.take_tab(src_pane, src_ix) else {
            return;
        };
        self.zoomed = None;
        if empty && src_pane == target {
            // the pane stays, so it needs a shell to show in place of the moved tab
            let profile = split_profile(Some(&tab), cx);
            if !self.add_profile_tab(src_pane, &profile, false, window, cx) {
                let pane = self.pane_mut(src_pane).expect("pane from take_tab");
                pane.tabs.push(tab);
                pane.active = 0;
                return;
            }
        } else if empty {
            self.layout.remove_leaf(src_pane);
            if self.focused == src_pane {
                self.focused = self.layout.first_pane_id().unwrap_or(target);
            }
        }
        let new_id = self.split_off(target, side, vec![tab]);
        self.focus_pane(new_id, window, cx);
    }

    /// a new pane holding `tabs` split off `target` at `side`, returns its id
    pub(super) fn split_off(&mut self, target: usize, side: DropSide, tabs: Vec<Tab>) -> usize {
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        let pane = Pane {
            id,
            tabs,
            active: 0,
            tab_scroll: ScrollHandle::new(),
        };
        self.layout.split_leaf(target, side, pane);
        id
    }

    /// move the tab at `src_ix` of `src_pane` to `dst_ix` of `dst_pane`
    pub(super) fn move_tab_to_pane(
        &mut self,
        src_pane: usize,
        src_ix: usize,
        dst_pane: usize,
        dst_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if src_pane == dst_pane {
            self.move_tab(src_pane, src_ix, dst_ix, cx);
            return;
        }
        let Some((tab, empty)) = self.take_tab(src_pane, src_ix) else {
            return;
        };
        if empty {
            self.layout.remove_leaf(src_pane);
            if self.focused == src_pane {
                self.focused = self.layout.first_pane_id().unwrap_or(dst_pane);
            }
        }
        let Some(dst) = self.pane_mut(dst_pane) else {
            return;
        };
        let pinned = dst.tabs.iter().filter(|tab| tab.pinned).count();
        let ix = if tab.pinned {
            dst_ix.min(pinned)
        } else {
            dst_ix.clamp(pinned, dst.tabs.len())
        };
        dst.tabs.insert(ix, tab);
        dst.active = ix;
        self.focus_pane(dst_pane, window, cx);
    }

    /// focus the pane next to the focused one on `side`
    pub(super) fn focus_pane_toward(
        &mut self,
        side: DropSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut rects = Vec::new();
        let whole = PaneRect {
            x: 0.,
            y: 0.,
            w: 1.,
            h: 1.,
        };
        self.layout.rects(whole, &mut rects);
        if let Some(id) = neighbor(&rects, self.focused, side) {
            self.focus_pane(id, window, cx);
        }
    }

    /// move the divider next to the focused pane toward `side`
    pub(super) fn resize_pane(&mut self, side: DropSide, cx: &mut Context<Self>) {
        let (axis, delta) = match side {
            DropSide::Left => (SplitAxis::Horizontal, -RESIZE_STEP),
            DropSide::Right => (SplitAxis::Horizontal, RESIZE_STEP),
            DropSide::Top => (SplitAxis::Vertical, -RESIZE_STEP),
            DropSide::Bottom => (SplitAxis::Vertical, RESIZE_STEP),
        };
        if self.layout.move_divider(self.focused, axis, delta) {
            cx.notify();
        }
    }

    /// set the ratio of the split at `path`
    pub(super) fn resize_split(&mut self, path: &[bool], ratio: f32, cx: &mut Context<Self>) {
        let Some(LayoutNode::Split { ratio: split, .. }) = layout_at_mut(&mut self.layout, path)
        else {
            return;
        };
        *split = ratio.clamp(MIN_SPLIT_RATIO, 1.0 - MIN_SPLIT_RATIO);
        cx.notify();
    }

    /// split `pane_id` at `side` and show a fresh shell in the new pane, focused. none when
    /// the shell failed to spawn, then nothing is split
    pub(super) fn split_new_tab(
        &mut self,
        pane_id: usize,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(usize, usize)> {
        let side = match side {
            Side::Left => DropSide::Left,
            Side::Right => DropSide::Right,
            Side::Up => DropSide::Top,
            Side::Down => DropSide::Bottom,
        };
        let active = self
            .pane(pane_id)
            .and_then(|pane| pane.tabs.get(pane.active));
        let profile = split_profile(active, cx);
        // spawned first, a pane without tabs breaks every tab action
        let window_id = window.window_handle().window_id().as_u64();
        let builder = match spawn_profile(&profile, TerminalBounds::default(), window_id, cx) {
            Ok(builder) => builder,
            Err(error) => {
                eprintln!("failed to spawn terminal: {error:#}");
                return None;
            }
        };
        self.zoomed = None;
        let id = self.split_off(pane_id, side, Vec::new());
        self.add_builder_tab(id, builder, &profile, true, window, cx);
        self.focus_pane(id, window, cx);
        Some((id, 0))
    }

    /// focus a pane the mouse moved over, only while a terminal has focus, so open overlays
    /// like the palette keep it
    pub(super) fn hover_pane(
        &mut self,
        pane_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = self.focused_pane();
        let in_terminal = pane
            .tabs
            .get(pane.active)
            .is_some_and(|tab| tab.view.focus_handle(cx).is_focused(window));
        if pane_id != self.focused && in_terminal {
            self.focus_pane(pane_id, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn dragging_a_tab_onto_the_pane_edge_splits_it(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        cx.run_until_parked();
        let start = cx.debug_bounds("tab-title-0").unwrap().center();
        let terminal = cx.debug_bounds("pane-terminal-0").unwrap();
        let end = point(
            terminal.origin.x + terminal.size.width * 0.75,
            terminal.origin.y + terminal.size.height * 0.5,
        );
        drag_to(cx, start, end);
        assert_eq!(ws.update(cx, |ws, _| ws.layout.pane_ids()).len(), 2);
        let left = cx.debug_bounds("pane-terminal-0").unwrap();
        let right = cx.debug_bounds("pane-terminal-1").unwrap();
        assert!(
            (left.size.width - right.size.width).abs() < gpui::px(2.),
            "panes {left:?} {right:?}"
        );
        assert!(
            left.size.width > gpui::px(400.) && left.size.width < gpui::px(500.),
            "left pane did not take half the width: {left:?}"
        );
        assert!(
            left.size.height > gpui::px(400.),
            "panes lost their height: {left:?}"
        );
    }

    #[gpui::test]
    fn dragging_a_tab_onto_the_bottom_edge_stacks_the_panes(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        cx.run_until_parked();
        let start = cx.debug_bounds("tab-title-0").unwrap().center();
        let terminal = cx.debug_bounds("pane-terminal-0").unwrap();
        let end = point(
            terminal.origin.x + terminal.size.width * 0.5,
            terminal.origin.y + terminal.size.height * 0.75,
        );
        drag_to(cx, start, end);
        assert_eq!(ws.update(cx, |ws, _| ws.layout.pane_ids()).len(), 2);
        let top = cx.debug_bounds("pane-terminal-0").unwrap();
        let bottom = cx.debug_bounds("pane-terminal-1").unwrap();
        assert!(bottom.origin.y > top.origin.y, "{top:?} {bottom:?}");
        assert!(
            (top.size.width - bottom.size.width).abs() < gpui::px(2.),
            "{top:?} {bottom:?}"
        );
        assert!(
            top.size.height > gpui::px(200.) && top.size.height < gpui::px(300.),
            "top pane did not take half the height: {top:?}"
        );
    }

    #[test]
    fn drop_side_picks_the_nearest_edge() {
        let bounds = Bounds {
            origin: point(gpui::px(0.), gpui::px(0.)),
            size: gpui::size(gpui::px(100.), gpui::px(100.)),
        };
        assert_eq!(
            drop_side(bounds, point(gpui::px(10.), gpui::px(50.))),
            DropSide::Left
        );
        assert_eq!(
            drop_side(bounds, point(gpui::px(90.), gpui::px(50.))),
            DropSide::Right
        );
        assert_eq!(
            drop_side(bounds, point(gpui::px(50.), gpui::px(10.))),
            DropSide::Top
        );
        assert_eq!(
            drop_side(bounds, point(gpui::px(50.), gpui::px(90.))),
            DropSide::Bottom
        );
    }

    #[gpui::test]
    fn splitting_moves_the_tab_into_a_new_pane(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let old = views(&ws, cx);
        let new_pane = split_pane(&ws, cx, DropSide::Right);
        let axis = ws.update(cx, |ws, _| match &ws.layout {
            LayoutNode::Split { axis, .. } => Some(*axis),
            _ => None,
        });
        assert_eq!(axis, Some(SplitAxis::Horizontal));
        assert_ne!(new_pane, 0);
        // the moved tab is alone in the new pane, the other one stays behind
        assert_eq!(views(&ws, cx), vec![old[0]]);
        assert_eq!(
            ws.update(cx, |ws, _| ws.pane(0).unwrap().tabs[0].view.entity_id()),
            old[1]
        );
    }

    #[gpui::test]
    fn drag_over_a_pane_picks_the_nearest_edge(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let bounds = Bounds {
            origin: point(gpui::px(0.), gpui::px(0.)),
            size: gpui::size(gpui::px(200.), gpui::px(100.)),
        };
        ws.update(cx, |ws, cx| {
            ws.update_drop_zone(0, point(gpui::px(190.), gpui::px(50.)), bounds, cx)
        });
        assert_eq!(
            ws.update(cx, |ws, _| ws.drop_zone),
            Some((0, DropSide::Right))
        );
        // a position outside clears the pane's zone again
        ws.update(cx, |ws, cx| {
            ws.update_drop_zone(0, point(gpui::px(300.), gpui::px(50.)), bounds, cx)
        });
        assert_eq!(ws.update(cx, |ws, _| ws.drop_zone), None);
    }

    #[gpui::test]
    fn moving_the_last_tab_between_panes_collapses_the_old_one(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let new_pane = split_pane(&ws, cx, DropSide::Right);
        ws.update_in(cx, |ws, window, cx| {
            ws.move_tab_to_pane(0, 0, new_pane, 1, window, cx);
        });
        cx.run_until_parked();
        // the emptied pane is gone, both tabs live in the new one
        assert_eq!(ws.update(cx, |ws, _| ws.layout.pane_ids()), vec![new_pane]);
        assert_eq!(views(&ws, cx).len(), 2);
    }

    #[gpui::test]
    fn closing_the_last_tab_of_a_pane_removes_the_pane(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let new_pane = split_pane(&ws, cx, DropSide::Right);
        ws.update_in(cx, |ws, window, cx| {
            ws.close_tab_at(new_pane, 0, window, cx)
        });
        cx.run_until_parked();
        assert_eq!(ws.update(cx, |ws, _| ws.layout.pane_ids()), vec![0]);
        assert_eq!(ws.update(cx, |ws, _| ws.focused), 0);
    }

    #[gpui::test]
    fn closing_a_leaf_in_a_nested_split_keeps_the_other_leaf(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        // left | (top / bottom)
        let right = split_pane(&ws, cx, DropSide::Right);
        let bottom = split_pane(&ws, cx, DropSide::Bottom);
        assert_eq!(
            ws.update(cx, |ws, _| ws.layout.pane_ids()),
            vec![0, right, bottom]
        );
        // closing the bottom pane must not take the top one with it
        ws.update_in(cx, |ws, window, cx| ws.close_tab_at(bottom, 0, window, cx));
        cx.run_until_parked();
        assert_eq!(ws.update(cx, |ws, _| ws.layout.pane_ids()), vec![0, right]);
    }

    #[gpui::test]
    fn closing_a_nested_pane_focuses_the_sibling(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        // left | (top / bottom)
        let top = split_pane(&ws, cx, DropSide::Right);
        let bottom = split_pane(&ws, cx, DropSide::Bottom);
        // a program runs in the top pane
        ws.update_in(cx, |ws, window, cx| ws.focus_pane(top, window, cx));
        cx.run_until_parked();
        start_program(&ws, cx);
        // closing the focused bottom pane must leave focus on the top one
        ws.update_in(cx, |ws, window, cx| ws.focus_pane(bottom, window, cx));
        cx.run_until_parked();
        close_active(&ws, cx);
        assert_eq!(ws.update(cx, |ws, _| ws.focused), top);
        assert_eq!(current(&ws, cx), 0);
        // closing the surviving pane's program must still ask
        close_active(&ws, cx);
        assert!(cx.debug_bounds("confirm-close").is_some());
    }

    #[gpui::test]
    fn split_commands_split_the_focused_pane(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let old = views(&ws, cx);
        run_actions(&ws, cx, "\"split_right\"");
        assert_eq!(ws.update(cx, |ws, _| ws.layout.pane_ids()).len(), 2);
        // the active tab moved into the new pane, which now has focus
        let new = ws.update(cx, |ws, _| ws.focused);
        assert_ne!(new, 0);
        assert_eq!(
            ws.update(cx, |ws, _| ws.pane(new).unwrap().tabs[0].view.entity_id()),
            old[1]
        );
        run_actions(&ws, cx, "\"split_up\"");
        assert_eq!(ws.update(cx, |ws, _| ws.layout.pane_ids()).len(), 3);
    }

    #[gpui::test]
    fn resizing_a_split_clamps_the_ratio(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        split_pane(&ws, cx, DropSide::Top);
        ws.update(cx, |ws, cx| ws.resize_split(&[], 5.0, cx));
        let ratio = ws.update(cx, |ws, _| match &ws.layout {
            LayoutNode::Split { ratio, .. } => *ratio,
            _ => panic!("expected a split"),
        });
        assert_eq!(ratio, 1.0 - MIN_SPLIT_RATIO);
    }

    #[gpui::test]
    fn hovering_a_pane_focuses_it(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let new_pane = split_pane(&ws, cx, DropSide::Right);
        let other = ws.update(cx, |ws, _| {
            ws.layout
                .panes()
                .into_iter()
                .map(|pane| pane.id)
                .find(|id| *id != new_pane)
                .unwrap()
        });
        ws.update_in(cx, |ws, window, cx| ws.hover_pane(other, window, cx));
        cx.run_until_parked();
        assert_eq!(ws.update(cx, |ws, _| ws.focused), other);
        assert_eq!(current(&ws, cx), 0);
    }

    #[test]
    fn neighbors_follow_the_layout() {
        let layout = three_panes();
        let mut rects = Vec::new();
        let whole = PaneRect {
            x: 0.,
            y: 0.,
            w: 1.,
            h: 1.,
        };
        layout.rects(whole, &mut rects);
        assert_eq!(neighbor(&rects, 0, DropSide::Right), Some(1));
        assert_eq!(neighbor(&rects, 2, DropSide::Left), Some(0));
        assert_eq!(neighbor(&rects, 1, DropSide::Bottom), Some(2));
        assert_eq!(neighbor(&rects, 2, DropSide::Top), Some(1));
        assert_eq!(neighbor(&rects, 0, DropSide::Left), None);
        assert_eq!(neighbor(&rects, 1, DropSide::Right), None);
    }

    #[test]
    fn equalize_and_move_divider_change_ratios() {
        // three side by side: 0 | (1 | 2)
        let mut layout = split(
            SplitAxis::Horizontal,
            0.5,
            leaf(0),
            split(SplitAxis::Horizontal, 0.5, leaf(1), leaf(2)),
        );
        layout.equalize();
        let LayoutNode::Split { ratio, second, .. } = &layout else {
            unreachable!()
        };
        assert!((ratio - 1. / 3.).abs() < 1e-6, "{ratio}");
        let LayoutNode::Split { ratio, .. } = second.as_ref() else {
            unreachable!()
        };
        assert!((ratio - 0.5).abs() < 1e-6);

        let mut layout = three_panes();
        // pane 2 is in a vertical split inside the horizontal one
        assert!(layout.move_divider(2, SplitAxis::Horizontal, -0.1));
        assert!(layout.move_divider(2, SplitAxis::Vertical, 0.1));
        let LayoutNode::Split { ratio, second, .. } = &layout else {
            unreachable!()
        };
        assert!((ratio - 0.4).abs() < 1e-6);
        let LayoutNode::Split { ratio, .. } = second.as_ref() else {
            unreachable!()
        };
        assert!((ratio - 0.6).abs() < 1e-6);
        // pane 0 has no vertical split around it
        assert!(!layout.move_divider(0, SplitAxis::Vertical, 0.1));
        // dividers never go past the minimum share
        for _ in 0..30 {
            layout.move_divider(0, SplitAxis::Horizontal, -0.1);
        }
        let LayoutNode::Split { ratio, .. } = &layout else {
            unreachable!()
        };
        assert_eq!(*ratio, MIN_SPLIT_RATIO);
    }

    #[gpui::test]
    fn pane_actions_focus_zoom_and_close(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"panes": {"dim_inactive": 0.5}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        run_actions(&ws, cx, r#""split_right""#);
        let right = ws.update(cx, |ws, _| ws.focused);
        let left = ws.update(cx, |ws, _| *ws.layout.pane_ids().first().unwrap());
        assert_ne!(left, right);
        // the pane without focus is dimmed
        let dim = cx
            .debug_bounds(&*format!("pane-dim-{left}").leak())
            .unwrap();
        assert!(dim.size.width > gpui::px(300.), "{dim:?}");
        assert!(
            cx.debug_bounds(&*format!("pane-dim-{right}").leak())
                .is_none()
        );

        run_actions(&ws, cx, r#""focus_pane_left""#);
        assert_eq!(ws.update(cx, |ws, _| ws.focused), left);
        run_actions(&ws, cx, r#""focus_pane_up""#);
        assert_eq!(ws.update(cx, |ws, _| ws.focused), left);
        run_actions(&ws, cx, r#""focus_pane_right""#);
        assert_eq!(ws.update(cx, |ws, _| ws.focused), right);

        let width = |cx: &mut VisualTestContext, id: usize| {
            cx.debug_bounds(&*format!("pane-terminal-{id}").leak())
                .map(|bounds| bounds.size.width)
        };
        let before = width(cx, right).unwrap();
        run_actions(&ws, cx, r#""resize_pane_left""#);
        assert!(width(cx, right).unwrap() > before);
        run_actions(&ws, cx, r#""equalize_panes""#);
        assert!((width(cx, right).unwrap() - before).abs() < gpui::px(2.));

        run_actions(&ws, cx, r#""zoom_pane""#);
        assert!(width(cx, left).is_none(), "other pane still shown");
        assert!(width(cx, right).unwrap() > gpui::px(800.));
        run_actions(&ws, cx, r#""zoom_pane""#);
        assert!(width(cx, left).is_some());

        run_actions(&ws, cx, r#""close_pane""#);
        assert_eq!(ws.update(cx, |ws, _| ws.layout.pane_ids()), vec![left]);
        assert_eq!(ws.update(cx, |ws, _| ws.focused), left);
    }

    #[gpui::test]
    fn split_starts_its_shell_in_the_folder_of_the_split_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let dir = std::env::temp_dir().canonicalize().unwrap();
        send_to(&ws, cx, 0, &format!("cd {}\r", dir.display()));
        let shell_pid = terminal(&ws, cx, 0).read_with(cx, |t, _| t.shell_pid);
        wait_until(cx, "cd never ran", |_| {
            crate::terminal::process_info(shell_pid).and_then(|p| p.cwd) == Some(dir.clone())
        });
        run_actions(&ws, cx, r#""split_right""#);
        // the new shell stays in the left pane, the moved tab went right
        let left = ws.update(cx, |ws, _| *ws.layout.pane_ids().first().unwrap());
        let new_pid = ws.update(cx, |ws, cx| {
            ws.pane(left).unwrap().tabs[0]
                .view
                .read(cx)
                .terminal()
                .read(cx)
                .shell_pid
        });
        wait_until(cx, "new shell is elsewhere", |_| {
            crate::terminal::process_info(new_pid).and_then(|p| p.cwd) == Some(dir.clone())
        });
    }

    fn ratio_at(ws: &Entity<Workspace>, cx: &mut VisualTestContext, path: &[bool]) -> f32 {
        let path = path.to_vec();
        ws.update(cx, |ws, _| match layout_at_mut(&mut ws.layout, &path) {
            Some(LayoutNode::Split { ratio, .. }) => *ratio,
            _ => panic!("no split at {path:?}"),
        })
    }

    #[gpui::test]
    fn up_and_down_move_the_divider_of_stacked_panes(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        split_pane(&ws, cx, DropSide::Bottom);
        let before = ratio_at(&ws, cx, &[]);
        // the bottom pane has focus, up grows it by moving the divider up
        run_actions(&ws, cx, r#""resize_pane_up""#);
        assert!((ratio_at(&ws, cx, &[]) - (before - RESIZE_STEP)).abs() < 1e-6);
        run_actions(&ws, cx, r#""resize_pane_down", "resize_pane_down""#);
        assert!((ratio_at(&ws, cx, &[]) - (before + RESIZE_STEP)).abs() < 1e-6);
    }

    #[gpui::test]
    fn dragging_a_nested_divider_resizes_only_its_split(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        // left | (top / bottom)
        split_pane(&ws, cx, DropSide::Right);
        split_pane(&ws, cx, DropSide::Bottom);
        ws.update(cx, |ws, cx| ws.resize_split(&[true], 0.3, cx));
        assert!((ratio_at(&ws, cx, &[true]) - 0.3).abs() < 1e-6);
        assert_eq!(ratio_at(&ws, cx, &[]), 0.5);
        // a path ending on a pane, like one from before the layout changed, does nothing
        ws.update(cx, |ws, cx| ws.resize_split(&[false], 0.9, cx));
        ws.update(cx, |ws, cx| ws.resize_split(&[true, true, true], 0.9, cx));
        assert_eq!(ratio_at(&ws, cx, &[]), 0.5);
        assert!((ratio_at(&ws, cx, &[true]) - 0.3).abs() < 1e-6);
    }

    #[gpui::test]
    fn a_shell_that_fails_to_spawn_splits_nothing(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let settings = Settings::parse(
            r#"{"profiles": [{"name": "broken", "command": {"program": "/nonexistent/kuterm-shell"}}]}"#,
        )
        .unwrap();
        cx.update(|_, cx| cx.set_global(settings));
        let split = ws.update_in(cx, |ws, window, cx| {
            ws.split_new_tab(ws.focused, Side::Right, window, cx)
        });
        assert_eq!(split, None);
        assert_eq!(ws.update(cx, |ws, _| ws.layout.panes().len()), 1);
        // the only tab split off its own pane stays when no shell can replace it
        run_actions(&ws, cx, r#""split_right""#);
        assert_eq!(ws.update(cx, |ws, _| ws.layout.panes().len()), 1);
        assert_eq!(views(&ws, cx).len(), 1);
        run_actions(&ws, cx, r#""next_tab""#);
    }

    #[gpui::test]
    fn the_only_tab_of_a_pane_split_off_another_pane_takes_the_pane_along(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let right = split_pane(&ws, cx, DropSide::Right);
        let moved = views(&ws, cx)[0];
        // the focused right pane's tab dropped on the bottom edge of the left pane
        ws.update_in(cx, |ws, window, cx| {
            ws.split_with_tab(right, 0, 0, DropSide::Bottom, window, cx)
        });
        cx.run_until_parked();
        let (ids, focused) = ws.update(cx, |ws, _| (ws.layout.pane_ids(), ws.focused));
        assert!(!ids.contains(&right), "the emptied pane stayed: {ids:?}");
        assert_eq!(ids, [0, focused]);
        assert_eq!(views(&ws, cx), [moved]);
        assert!(matches!(
            ws.update(cx, |ws, _| match &ws.layout {
                LayoutNode::Split { axis, .. } => Some(*axis),
                _ => None,
            }),
            Some(SplitAxis::Vertical)
        ));
    }

    #[gpui::test]
    fn a_pinned_tab_moved_to_another_pane_stays_among_its_pinned_tabs(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        let right = split_pane(&ws, cx, DropSide::Right);
        let moved = views(&ws, cx)[0];
        ws.update(cx, |ws, _| {
            ws.pane_mut(0).unwrap().tabs[0].pinned = true;
            ws.pane_mut(right).unwrap().tabs[0].pinned = true;
        });
        // dropped past the unpinned tab, it goes right after the pinned one
        ws.update_in(cx, |ws, window, cx| {
            ws.move_tab_to_pane(right, 0, 0, 2, window, cx)
        });
        cx.run_until_parked();
        assert_eq!(ws.update(cx, |ws, _| ws.layout.pane_ids()), [0]);
        assert_eq!(ws.update(cx, |ws, _| ws.focused), 0);
        assert_eq!(views(&ws, cx)[1], moved);
        assert_eq!(current(&ws, cx), 1);
    }
}
