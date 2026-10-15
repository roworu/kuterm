//! session restore: tabs and splits saved on quit and opened again on launch. only shells in
//! their folders come back, not the programs that ran in them

use std::path::PathBuf;

use gpui::{Context, ScrollHandle, Window};
use serde::{Deserialize, Serialize};

use super::{LayoutNode, Pane, SplitAxis, Workspace, notifications::NotificationButton, tab_cwd};
use crate::settings::{RestoreSession, Settings, state_dir};

/// one tab: what it was started with and where its shell was
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct SavedTab {
    pub(super) profile: String,
    pub(super) cwd: Option<PathBuf>,
    pub(super) title: String,
    /// title typed with "rename_tab"
    #[serde(default)]
    pub(super) custom_title: Option<String>,
    #[serde(default)]
    pub(super) pinned: bool,
    /// name and color of the group the tab was put into by hand
    #[serde(default)]
    pub(super) group: Option<(String, usize)>,
    /// name `kuterm msg --pane` finds the tab by
    #[serde(default)]
    pub(super) name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SavedNode {
    Pane {
        tabs: Vec<SavedTab>,
        active: usize,
        focused: bool,
    },
    Split {
        vertical: bool,
        ratio: f32,
        first: Box<SavedNode>,
        second: Box<SavedNode>,
    },
}

fn path() -> Option<PathBuf> {
    Some(state_dir()?.join("session.json"))
}

/// the saved session, none when there is none or it can't be read
pub(super) fn load() -> Option<SavedNode> {
    let path = path()?;
    let json = std::fs::read_to_string(&path).ok()?;
    serde_json_lenient::from_str(&json)
        .map_err(|error| eprintln!("invalid session in {}: {error}", path.display()))
        .ok()
}

impl Workspace {
    fn save_node(&self, node: &LayoutNode, cx: &gpui::App) -> Option<SavedNode> {
        match node {
            LayoutNode::Leaf(pane) => {
                // tabs with their own temp files, like a pager on the history, can't come back
                let kept: Vec<usize> = (0..pane.tabs.len())
                    .filter(|&ix| pane.tabs[ix].temp_file.is_none())
                    .collect();
                if kept.is_empty() {
                    return None;
                }
                let tabs = kept
                    .iter()
                    .map(|&ix| {
                        let tab = &pane.tabs[ix];
                        SavedTab {
                            profile: tab.profile.clone(),
                            cwd: tab_cwd(tab, cx),
                            title: tab.title.clone(),
                            custom_title: tab.custom_title.clone(),
                            pinned: tab.pinned,
                            group: self.saved_group(tab),
                            name: tab.name.clone(),
                        }
                    })
                    .collect();
                Some(SavedNode::Pane {
                    tabs,
                    active: kept.iter().position(|&ix| ix == pane.active).unwrap_or(0),
                    focused: pane.id == self.focused,
                })
            }
            LayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => match (self.save_node(first, cx), self.save_node(second, cx)) {
                (Some(first), Some(second)) => Some(SavedNode::Split {
                    vertical: *axis == SplitAxis::Vertical,
                    ratio: *ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (one, other) => one.or(other),
            },
            LayoutNode::Empty => None,
        }
    }

    /// write the tabs and splits to the state folder when sessions are restored
    pub(super) fn save_session(&self, cx: &gpui::App) {
        if Settings::get(cx).restore_session == RestoreSession::Never {
            return;
        }
        let Some(path) = path() else {
            return;
        };
        // quitting by closing the last tab leaves nothing to come back to
        let Some(node) = self.save_node(&self.layout, cx) else {
            std::fs::remove_file(&path).ok();
            return;
        };
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                serde_json_lenient::to_string_pretty(&node).map_err(std::io::Error::other)
            })
            .and_then(|json| std::fs::write(&path, json));
        if let Err(error) = result {
            eprintln!("failed to write {}: {error}", path.display());
        }
    }

    // panes of the layout get their ids here, tabs are opened once the layout is in place
    fn build_node(&mut self, node: &SavedNode, panes: &mut Vec<(usize, SavedNode)>) -> LayoutNode {
        match node {
            SavedNode::Pane { .. } => {
                let id = self.next_pane_id;
                self.next_pane_id += 1;
                panes.push((id, node.clone()));
                LayoutNode::Leaf(Pane {
                    id,
                    tabs: Vec::new(),
                    active: 0,
                    tab_scroll: ScrollHandle::new(),
                })
            }
            SavedNode::Split {
                vertical,
                ratio,
                first,
                second,
            } => LayoutNode::Split {
                axis: if *vertical {
                    SplitAxis::Vertical
                } else {
                    SplitAxis::Horizontal
                },
                ratio: ratio.clamp(super::MIN_SPLIT_RATIO, 1. - super::MIN_SPLIT_RATIO),
                first: Box::new(self.build_node(first, panes)),
                second: Box::new(self.build_node(second, panes)),
            },
        }
    }

    /// open the saved session in place of the tabs open now, false when nothing could be
    /// opened, then the tabs stay
    pub(super) fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(saved) = load() else {
            return false;
        };
        let mut panes = Vec::new();
        let layout = self.build_node(&saved, &mut panes);
        let old_layout = std::mem::replace(&mut self.layout, layout);
        let old_focused = self.focused;
        let mut focused = None;
        for (id, node) in panes {
            let SavedNode::Pane {
                tabs,
                active,
                focused: was_focused,
            } = node
            else {
                continue;
            };
            for tab in &tabs {
                let settings = Settings::get(cx);
                // a profile renamed or removed since falls back to the default one
                let mut profile = settings
                    .profile(&tab.profile)
                    .unwrap_or_else(|| settings.default_profile())
                    .clone();
                if let Some(cwd) = tab.cwd.clone().filter(|cwd| cwd.is_dir()) {
                    profile.working_directory = Some(cwd);
                }
                let before = self.pane(id).map_or(0, |pane| pane.tabs.len());
                self.add_profile_tab(id, &profile, false, window, cx);
                // a tab that failed to spawn is left out
                if let Some(pane) = self.pane_mut(id)
                    && pane.tabs.len() > before
                    && let Some(last) = pane.tabs.last_mut()
                {
                    last.custom_title = tab.custom_title.clone();
                    last.pinned = tab.pinned;
                    last.name = tab.name.clone();
                    if let Some((name, color)) = &tab.group {
                        let ix = before;
                        self.restore_group(id, ix, name, *color);
                    }
                }
            }
            let Some(pane) = self.pane_mut(id) else {
                continue;
            };
            if pane.tabs.is_empty() {
                self.layout.remove_leaf(id);
                continue;
            }
            pane.active = active.min(pane.tabs.len() - 1);
            if was_focused || focused.is_none() {
                focused = Some(id);
            }
        }
        let Some(focused) = focused else {
            self.layout = old_layout;
            self.focused = old_focused;
            return false;
        };
        self.zoomed = None;
        let pane = self.pane(focused).expect("pane was just built");
        let active = pane.active;
        self.activate_tab(focused, active, window, cx);
        true
    }

    /// open a session as settings say: restored right away, asked about, or not at all.
    /// false when no tab was opened
    pub(super) fn open_session(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match Settings::get(cx).restore_session {
            RestoreSession::Always => self.restore_session(window, cx),
            RestoreSession::Ask => {
                if load().is_some() {
                    let buttons = vec![
                        NotificationButton::new("restore", |this, window, cx| {
                            this.restore_session(window, cx);
                        }),
                        NotificationButton::new("skip", |_, _, _| {}),
                    ];
                    self.ask(
                        "restore the tabs of the last session?",
                        None,
                        None,
                        buttons,
                        cx,
                    );
                }
                false
            }
            RestoreSession::Never => false,
        }
    }
}
