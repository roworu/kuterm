//! synced scroll: panes joined to the sync scroll together by the same number of lines, like
//! vim's scrollbind, to compare two logs or two hosts line by line

use gpui::{Context, Entity};

use super::Workspace;
use crate::ui::terminal_view::TerminalView;

impl Workspace {
    /// join the focused pane to the panes scrolled together, or take it out
    pub(super) fn toggle_scroll_sync(&mut self, cx: &mut Context<Self>) {
        if !self.scroll_sync.remove(&self.focused) {
            self.scroll_sync.insert(self.focused);
        }
        cx.notify();
    }

    /// true when scrolling `pane` scrolls others too
    pub(super) fn scroll_synced(&self, pane: usize) -> bool {
        // closed panes stay in the set until the next scroll, only the live ones count
        self.scroll_sync.contains(&pane)
            && self
                .scroll_sync
                .iter()
                .filter(|&&id| self.pane(id).is_some())
                .count()
                > 1
    }

    /// `view` scrolled `delta` lines, the active tabs of the other joined panes follow
    pub(super) fn sync_scroll(
        &mut self,
        view: &Entity<TerminalView>,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        // closed panes leave on their own
        let panes = self.layout.pane_ids();
        self.scroll_sync.retain(|pane| panes.contains(pane));
        for terminal in self.joined_terminals(&self.scroll_sync, view, cx) {
            // relative, so wheel events coming faster than frames add up, and each pane stops
            // at the ends of its own history
            terminal.update(cx, |terminal, cx| {
                terminal.scroll(delta);
                cx.notify();
            });
        }
    }
}

#[cfg(test)]
mod tests {

    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn a_pane_left_alone_is_not_marked_synced(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let first = ws.update(cx, |ws, cx| {
            ws.toggle_scroll_sync(cx);
            ws.focused
        });
        split_pane(&ws, cx, DropSide::Right);
        ws.update(cx, |ws, cx| ws.toggle_scroll_sync(cx));
        assert!(ws.update(cx, |ws, _| ws.scroll_synced(first)));
        run_actions(&ws, cx, r#""close_pane""#);
        assert!(!ws.update(cx, |ws, _| ws.scroll_synced(first)));
    }

    #[gpui::test]
    fn synced_panes_follow_each_delta_clamped_to_their_history(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        // a second pane with the second tab, both joined
        run_actions(&ws, cx, r#""split_right""#);
        let panes = ws.read_with(cx, |ws, _| ws.layout.pane_ids());
        ws.update(cx, |ws, _| ws.scroll_sync = panes.iter().copied().collect());
        let views: Vec<_> = ws.read_with(cx, |ws, _| {
            ws.layout
                .panes()
                .iter()
                .map(|pane| pane.tabs[pane.active].view.clone())
                .collect()
        });
        let terminals: Vec<_> = views
            .iter()
            .map(|view| view.read_with(cx, |view, _| view.terminal().clone()))
            .collect();
        // the first pane has a long history, the second a short one
        let (first, second) = (terminals[0].clone(), terminals[1].clone());
        first.update(cx, |t, _| t.type_text(b"seq 1 300\r"));
        wait_until(cx, "history of the first pane", |cx| {
            first.read_with(cx, |t, _| t.history_size() > 200)
        });
        let short = second.read_with(cx, |t, _| t.history_size());
        // wheel events faster than frames: three deltas before anything is drawn
        ws.update(cx, |ws, cx| {
            for _ in 0..3 {
                ws.sync_scroll(&views[1], 10, cx);
            }
        });
        for terminal in &terminals {
            terminal.update(cx, |t, _| t.sync());
        }
        assert_eq!(
            first.read_with(cx, |t, _| t.last_content.display_offset),
            30
        );
        assert_eq!(
            second.read_with(cx, |t, _| t.last_content.display_offset),
            30.min(short)
        );
        ws.update(cx, |ws, cx| ws.sync_scroll(&views[1], -100, cx));
        first.update(cx, |t, _| t.sync());
        assert_eq!(first.read_with(cx, |t, _| t.last_content.display_offset), 0);
    }
}
