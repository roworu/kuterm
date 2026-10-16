//! find in all tabs: one query over the history of every pane of every window, picking a
//! match switches to its tab, scrolls to it and selects it

use std::time::Duration;

use gpui::{AnyWindowHandle, AppContext, Context, Entity, EntityId, Window};

use super::Workspace;
use crate::{
    settings::{Command, CommandAction, Inputs, SearchCase},
    terminal::{FoundLine, SearchQuery, SearchScope, Terminal, search_pattern},
    ui::command_palette::{CommandPalette, FinderQuery},
};

// typing waits this long for the next key before searching
const FIND_DEBOUNCE: Duration = Duration::from_millis(150);
// matches each pane adds to the list, newest first
const FIND_PER_PANE: usize = 50;
const FAILED_PREFIX: &str = "failed:";

/// a match in some tab of some window
pub(super) struct Found {
    window: AnyWindowHandle,
    tab: EntityId,
    line: FoundLine,
}

impl Workspace {
    /// palette searching every tab of every window as the query is typed
    pub(super) fn open_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let palette = cx.new(|cx| {
            CommandPalette::finder("find in all tabs... (failed: for failed commands)", cx)
        });
        self.open_overlay(&palette, window, cx);
        let own = window.window_handle();
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            move |this, palette, query: &FinderQuery, _, cx| {
                this.find_everywhere(own, query.0.clone(), palette.clone(), cx)
            },
        ));
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            |this, _, command: &Command, window, cx| {
                this.close_overlay(window, cx);
                // rows carry their index in the found list as their only action
                if let Some(CommandAction::ActivateTab(ix)) =
                    command.actions.first().map(|step| &step.action)
                {
                    this.reveal_found(*ix, window, cx);
                }
            },
        ));
        self.palette = Some(palette);
    }

    /// search the tabs of every window for `text` after a short pause, then list the matches
    fn find_everywhere(
        &mut self,
        own: AnyWindowHandle,
        text: String,
        palette: Entity<CommandPalette>,
        cx: &mut Context<Self>,
    ) {
        // "failed:" in front looks only in the output of failed commands
        let (scope, text) = match text.strip_prefix(FAILED_PREFIX) {
            Some(text) => (SearchScope::Failed, text.trim_start().to_string()),
            None => (SearchScope::All, text),
        };
        let query = SearchQuery {
            text,
            regex: false,
            whole_word: false,
            case: SearchCase::Smart,
            scope,
        };
        let Ok(pattern) = search_pattern(&query) else {
            self.found.clear();
            palette.update(cx, |palette, cx| palette.set_results(Vec::new(), cx));
            return;
        };
        // this window first, then the others in the order the app keeps them
        let mut tabs = self.tabs_with_titles(cx);
        let mut sources: Vec<(AnyWindowHandle, EntityId, String, Entity<Terminal>)> = tabs
            .drain(..)
            .map(|(id, title, terminal)| (own, id, title, terminal))
            .collect();
        for handle in cx.windows() {
            if handle == own {
                continue;
            }
            let Some(workspace) = handle.downcast::<Workspace>() else {
                continue;
            };
            if let Ok(workspace) = workspace.read(cx) {
                sources.extend(
                    workspace
                        .tabs_with_titles(cx)
                        .into_iter()
                        .map(|(id, title, terminal)| (handle, id, title, terminal)),
                );
            }
        }
        let searches: Vec<_> = sources
            .into_iter()
            .filter_map(|(window, tab, title, terminal)| {
                let task = terminal.update(cx, |terminal, _| {
                    terminal.find_task(&pattern, FIND_PER_PANE, scope)
                })?;
                Some((window, tab, title, task))
            })
            .collect();
        self._find_task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FIND_DEBOUNCE).await;
            let found = cx
                .background_spawn(async move {
                    searches
                        .into_iter()
                        .flat_map(|(window, tab, title, task)| {
                            task()
                                .into_iter()
                                .map(move |line| (window, tab, title.clone(), line))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                let rows = found
                    .iter()
                    .enumerate()
                    .map(|(ix, (_, _, title, line))| {
                        let command = Command {
                            name: line.text.clone(),
                            category: Some(title.clone()),
                            pinned: false,
                            inputs: Inputs::new(),
                            actions: vec![CommandAction::ActivateTab(ix).into()],
                        };
                        (command, Some(line.highlight.clone()))
                    })
                    .collect();
                this.found = found
                    .into_iter()
                    .map(|(window, tab, _, line)| Found { window, tab, line })
                    .collect();
                palette.update(cx, |palette, cx| palette.set_results(rows, cx));
            })
            .ok();
        });
    }

    /// every tab with its number and title, and its terminal, in pane order
    fn tabs_with_titles(&self, cx: &gpui::App) -> Vec<(EntityId, String, Entity<Terminal>)> {
        let mut tabs = Vec::new();
        for pane in self.layout.panes() {
            for (ix, tab) in pane.tabs.iter().enumerate() {
                tabs.push((
                    tab.view.entity_id(),
                    // titles are often the same, like the folder, the number tells them apart
                    format!("{} {}", ix + 1, self.tab_title(pane.id, ix, cx)),
                    tab.view.read(cx).terminal().clone(),
                ));
            }
        }
        tabs
    }

    /// switch to the tab of found match `ix`, in its window, and select the match there
    fn reveal_found(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(found) = self.found.get(ix) else {
            return;
        };
        let (tab, line) = (found.tab, found.line.clone());
        if found.window == window.window_handle() {
            self.select_found_in(tab, &line, window, cx);
            return;
        }
        let Some(other) = found.window.downcast::<Workspace>() else {
            return;
        };
        other
            .update(cx, |workspace, window, cx| {
                window.activate_window();
                workspace.select_found_in(tab, &line, window, cx);
            })
            .ok();
    }

    fn select_found_in(
        &mut self,
        tab: EntityId,
        line: &FoundLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((pane, ix)) = self.find_id(tab) else {
            return;
        };
        self.activate_tab(pane, ix, window, cx);
        self.terminal_at(pane, ix, cx)
            .update(cx, |terminal, _| terminal.select_found(line));
        cx.notify();
    }
}
