//! session restore: tabs and splits saved on quit and opened again on launch. only shells in
//! their folders come back, not the programs that ran in them

use std::path::PathBuf;

use gpui::{Context, ScrollHandle, Window};
use serde::{Deserialize, Serialize};

use super::{LayoutNode, Pane, SplitAxis, Workspace, notifications::NotificationButton, tab_cwd};
use crate::settings::{RestoreSession, Settings, load_json, save_json, state_dir};

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
    load_json(&path()?, "session")
}

impl Workspace {
    pub(super) fn save_node(&self, node: &LayoutNode, cx: &gpui::App) -> Option<SavedNode> {
        match node {
            LayoutNode::Leaf(pane) => {
                // tabs with their own temp files, like a pager on the history, can't come back,
                // incognito ones must not
                let kept: Vec<usize> = (0..pane.tabs.len())
                    .filter(|&ix| {
                        let tab = &pane.tabs[ix];
                        tab.temp_file.is_none() && !tab.view.read(cx).terminal().read(cx).incognito
                    })
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
        save_json(&path, &node);
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
                let mut profile = Settings::get(cx).profile_or_default(&tab.profile).clone();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn incognito_tabs_are_left_out_of_the_session(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        terminal(&ws, cx, 1).update(cx, |terminal, _| terminal.incognito = true);
        let saved = ws.update(cx, |ws, cx| ws.save_node(&ws.layout, cx));
        match saved {
            Some(session::SavedNode::Pane { tabs, .. }) => assert_eq!(tabs.len(), 1),
            other => panic!("not one pane: {other:?}"),
        }
        terminal(&ws, cx, 0).update(cx, |terminal, _| terminal.incognito = true);
        assert!(
            ws.update(cx, |ws, cx| ws.save_node(&ws.layout, cx))
                .is_none()
        );
    }

    #[gpui::test]
    fn session_is_saved_and_restored_with_splits_and_folders(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("session");
        crate::settings::tests::with_config_home(&dir, || {
            let settings = Settings::parse(r#"{"restore_session": "always"}"#).unwrap();
            let (ws, cx) = open_with_settings(cx, 2, "{}", settings.clone());
            let tmp = std::env::temp_dir().canonicalize().unwrap();
            send_to(&ws, cx, 1, &format!("cd {}\r", tmp.display()));
            let shell_pid = terminal(&ws, cx, 1).read_with(cx, |t, _| t.shell_pid);
            wait_until(cx, "cd never ran", |_| {
                crate::terminal::process_info(shell_pid).and_then(|p| p.cwd) == Some(tmp.clone())
            });
            run_actions(&ws, cx, r#""split_down""#);
            ws.update(cx, |ws, _| {
                let tab = &mut ws.pane_mut(ws.layout.pane_ids()[0]).unwrap().tabs[0];
                tab.custom_title = Some("work".into());
                tab.pinned = true;
            });
            ws.update(cx, |ws, cx| ws.save_session(cx));
            assert!(dir.join("state/kuterm/session.json").exists());
            // only a first window restores
            cx.update(|window, _| window.remove_window());
            cx.run_until_parked();

            let (restored, cx) = cx.add_window_view(Workspace::new);
            cx.run_until_parked();
            let (panes, cwd) = restored.update(cx, |ws, cx| {
                let panes = ws.layout.pane_ids().len();
                let pane = ws.focused_pane();
                let pid = pane.tabs[pane.active]
                    .view
                    .read(cx)
                    .terminal()
                    .read(cx)
                    .shell_pid;
                (panes, pid)
            });
            assert_eq!(panes, 2);
            let kept = restored.update(cx, |ws, _| {
                let tab = &ws.pane(ws.layout.pane_ids()[0]).unwrap().tabs[0];
                (tab.custom_title.clone(), tab.pinned)
            });
            assert_eq!(kept, (Some("work".into()), true));
            // the moved tab was focused and its shell was in the temp folder
            wait_until(cx, "restored shell is elsewhere", |_| {
                crate::terminal::process_info(cwd).and_then(|p| p.cwd) == Some(tmp.clone())
            });

            // closing everything leaves no session
            restored.update(cx, |ws, cx| {
                ws.layout = LayoutNode::Empty;
                ws.save_session(cx);
            });
            assert!(!dir.join("state/kuterm/session.json").exists());
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn a_split_with_only_incognito_tabs_on_one_side_saves_the_other_side(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        run_actions(&ws, cx, r#""split_right""#);
        // the moved tab in the new pane is the only one there
        terminal(&ws, cx, 0).update(cx, |terminal, _| terminal.incognito = true);
        let saved = ws.update(cx, |ws, cx| ws.save_node(&ws.layout, cx));
        match saved {
            Some(SavedNode::Pane { tabs, focused, .. }) => {
                assert_eq!(tabs.len(), 1);
                // focus was on the side left out
                assert!(!focused);
            }
            other => panic!("not the left pane alone: {other:?}"),
        }
    }

    #[gpui::test]
    fn ask_mode_restores_the_session_only_when_told(cx: &mut TestAppContext) {
        with_own_state("session_ask", || {
            let ask = Settings::parse(r#"{"restore_session": "ask"}"#).unwrap();
            // nothing was saved yet, so nothing is asked
            let (ws, cx) = open_with_settings(cx, 3, "{}", ask);
            assert!(notifications(&ws, cx).is_empty());
            ws.update(cx, |ws, cx| ws.save_session(cx));
            cx.update(|window, _| window.remove_window());
            cx.run_until_parked();

            // a fresh tab opens meanwhile, the question waits for an answer
            let (restored, cx) = cx.add_window_view(Workspace::new);
            cx.run_until_parked();
            assert_eq!(views(&restored, cx).len(), 1);
            assert_eq!(
                notifications(&restored, cx),
                ["restore the tabs of the last session?"]
            );
            let restore = cx.debug_bounds("notification-0-button-0").unwrap().center();
            click(cx, restore, 1);
            release(cx, restore);
            cx.run_until_parked();
            assert_eq!(views(&restored, cx).len(), 3);
            assert!(notifications(&restored, cx).is_empty());
        });
    }
}
