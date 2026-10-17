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
            // relative, so wheel events coming faster than frames add up, and each pane stops
            // at the ends of its own history
            terminal.update(cx, |terminal, cx| {
                terminal.scroll(delta);
                cx.notify();
            });
        }
    }
}
