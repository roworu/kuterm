//! broadcast input: keys typed into one pane go to every pane joined to the broadcast, each
//! marked so nobody types into the wrong machine by accident

use std::collections::BTreeSet;

use gpui::{Context, Entity};

use super::Workspace;
use crate::{
    settings::Settings,
    terminal::InputSource::Broadcast,
    ui::terminal_view::{TerminalView, TypedInput},
};

/// panes all joined, or none of them when they all were, so the action works as a switch
pub(super) fn toggle_all(
    current: Option<&BTreeSet<usize>>,
    panes: &[usize],
) -> Option<BTreeSet<usize>> {
    let all: BTreeSet<usize> = panes.iter().copied().collect();
    (current != Some(&all)).then_some(all)
}

/// `pane` joined, or left out when it was in, none once no pane is left
pub(super) fn toggle_one(
    current: Option<&BTreeSet<usize>>,
    pane: usize,
) -> Option<BTreeSet<usize>> {
    let mut panes = current.cloned().unwrap_or_default();
    if !panes.remove(&pane) {
        panes.insert(pane);
    }
    (!panes.is_empty()).then_some(panes)
}

impl Workspace {
    /// send input to every pane of the window, or stop
    pub(super) fn toggle_broadcast_all(&mut self, cx: &mut Context<Self>) {
        self.broadcast = toggle_all(self.broadcast.as_ref(), &self.layout.pane_ids());
        cx.notify();
    }

    /// join the focused pane to the broadcast, or take it out
    pub(super) fn toggle_broadcast_pane(&mut self, cx: &mut Context<Self>) {
        self.broadcast = toggle_one(self.broadcast.as_ref(), self.focused);
        cx.notify();
    }

    /// true when typing into `pane` reaches other panes too
    pub(super) fn broadcasts(&self, pane: usize) -> bool {
        self.broadcast
            .as_ref()
            .is_some_and(|panes| panes.contains(&pane) && panes.len() > 1)
    }

    /// input typed into `view` goes to the active tab of every other joined pane, when the
    /// view is the active tab of a joined pane
    pub(super) fn broadcast_input(
        &mut self,
        view: &Entity<TerminalView>,
        input: &TypedInput,
        cx: &mut Context<Self>,
    ) {
        let Some(panes) = &self.broadcast else {
            return;
        };
        let Some((source, ix)) = self.find_view(view) else {
            return;
        };
        if !panes.contains(&source) || self.pane(source).is_none_or(|pane| pane.active != ix) {
            return;
        }
        let option_as_meta = Settings::get(cx).terminal.option_as_meta;
        let targets: Vec<_> = panes
            .iter()
            .filter(|&&pane| pane != source)
            .filter_map(|&pane| {
                let pane = self.pane(pane)?;
                Some(pane.tabs.get(pane.active)?.view.read(cx).terminal().clone())
            })
            .collect();
        for terminal in targets {
            terminal.update(cx, |terminal, _| match input {
                TypedInput::Key(keystroke, kind) => {
                    terminal.try_keystroke(keystroke, *kind, option_as_meta, Broadcast);
                }
                TypedInput::Text(text) => {
                    terminal.input(Broadcast, text.clone().into_bytes());
                }
                TypedInput::Paste(text) => {
                    terminal.paste(Broadcast, text);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{toggle_all, toggle_one};

    #[test]
    fn toggling_all_joins_every_pane_then_none() {
        let all = toggle_all(None, &[1, 2, 3]);
        assert_eq!(all, Some(BTreeSet::from([1, 2, 3])));
        assert_eq!(toggle_all(all.as_ref(), &[1, 2, 3]), None);
        // some joined: all of them join
        let some = BTreeSet::from([2]);
        assert_eq!(
            toggle_all(Some(&some), &[1, 2, 3]),
            Some(BTreeSet::from([1, 2, 3]))
        );
    }

    #[test]
    fn toggling_one_pane_adds_and_removes_it() {
        let one = toggle_one(None, 4);
        assert_eq!(one, Some(BTreeSet::from([4])));
        let two = toggle_one(one.as_ref(), 7);
        assert_eq!(two, Some(BTreeSet::from([4, 7])));
        let back = toggle_one(two.as_ref(), 4);
        assert_eq!(back, Some(BTreeSet::from([7])));
        assert_eq!(toggle_one(back.as_ref(), 7), None);
    }
}
