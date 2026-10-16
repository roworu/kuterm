//! synced scroll: panes joined to the sync scroll together by the same number of lines, like
//! vim's scrollbind, to compare two logs or two hosts line by line

use gpui::{Context, Entity};

use super::Workspace;
use crate::ui::terminal_view::TerminalView;

/// offset from the bottom a pane at `offset` with `history` lines moves to when another pane
/// scrolled `delta` lines, positive up
pub(super) fn follow(offset: usize, history: usize, delta: i32) -> usize {
    (offset as i64 + delta as i64).clamp(0, history as i64) as usize
}

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
        self.scroll_sync.contains(&pane) && self.scroll_sync.len() > 1
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
        let Some((source, ix)) = self.find_view(view) else {
            return;
        };
        if !self.scroll_sync.contains(&source)
            || self.pane(source).is_none_or(|pane| pane.active != ix)
        {
            return;
        }
        let targets: Vec<_> = self
            .scroll_sync
            .iter()
            .filter(|&&pane| pane != source)
            .filter_map(|&pane| {
                let pane = self.pane(pane)?;
                Some(pane.tabs.get(pane.active)?.view.read(cx).terminal().clone())
            })
            .collect();
        for terminal in targets {
            terminal.update(cx, |terminal, cx| {
                let offset = terminal.last_content.display_offset;
                terminal.scroll_to(follow(offset, terminal.history_size(), delta));
                cx.notify();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::follow;

    #[test]
    fn delta_moves_each_pane_clamped_to_its_history() {
        // a pane with a long history follows the whole way
        assert_eq!(follow(0, 500, 50), 50);
        assert_eq!(follow(80, 500, -30), 50);
        // a short one stops at its top and its bottom
        assert_eq!(follow(10, 20, 50), 20);
        assert_eq!(follow(5, 20, -50), 0);
        assert_eq!(follow(0, 0, 50), 0);
    }
}
