//! moving a tab with its running shell to another window: dragged out of the window, or with
//! "move_tab_to_new_window". the terminal entity is app wide, so only its view is made anew

use gpui::{AnyWindowHandle, App, Context, Entity, Pixels, Point, Window, WindowHandle, px};

use super::Workspace;
use crate::{
    settings::{Profile, Settings},
    terminal::Terminal,
};

// where the pointer sits on a torn off tab in its new window, so the tab lands under it
const GRAB_OFFSET: Point<Pixels> = Point {
    x: px(60.),
    y: px(15.),
};

/// a tab on its way to another window
pub struct MovedTab {
    pub(super) terminal: Entity<Terminal>,
    pub(super) profile: Profile,
    pub(super) custom_title: Option<String>,
    pub(super) name: Option<String>,
    /// name and color of the hand made group it was in
    pub(super) group: Option<(String, usize)>,
    pub(super) pinned: bool,
}

impl Workspace {
    /// what tab `ix` of `pane_id` takes along to another window
    fn moving_tab(&self, pane_id: usize, ix: usize, cx: &App) -> Option<MovedTab> {
        let tab = self.pane(pane_id)?.tabs.get(ix)?;
        Some(MovedTab {
            terminal: tab.view.read(cx).terminal().clone(),
            profile: Settings::get(cx).profile_or_default(&tab.profile).clone(),
            custom_title: tab.custom_title.clone(),
            name: tab.name.clone(),
            group: self.saved_group(tab),
            pinned: tab.pinned,
        })
    }

    /// show a tab moved from another window in pane `pane_id`, switching to it
    pub(super) fn adopt_tab(
        &mut self,
        pane_id: usize,
        moved: MovedTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.attach_terminal(pane_id, moved.terminal, &moved.profile, false, window, cx);
        let Some(pane) = self.pane_mut(pane_id) else {
            return;
        };
        let ix = pane.tabs.len() - 1;
        let tab = &mut pane.tabs[ix];
        // its shell started long ago, input needs no waiting
        tab.ready = true;
        tab.custom_title = moved.custom_title;
        tab.name = moved.name;
        tab.pinned = moved.pinned;
        let id = tab.view.entity_id();
        // a pinned tab goes after the pinned ones already there
        let to = if moved.pinned {
            pane.tabs.iter().filter(|tab| tab.pinned).count() - 1
        } else {
            ix
        };
        self.move_tab(pane_id, ix, to, cx);
        if let Some((name, color)) = moved.group {
            self.restore_group(pane_id, to, &name, color);
            self.regroup(pane_id);
        }
        if let Some((pane_id, ix)) = self.find_id(id) {
            self.activate_tab(pane_id, ix, window, cx);
        }
    }

    /// drop tab `ix` of `pane_id` after it moved away, closing the window when it was the last
    fn forget_moved_tab(
        &mut self,
        pane_id: usize,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, empty)) = self.take_tab(pane_id, ix) else {
            return;
        };
        if empty {
            self.layout.remove_leaf(pane_id);
            if self.layout.first_pane_id().is_none() {
                // the tab lives on in the other window, so nothing quits
                self.quitting = true;
                window.remove_window();
                return;
            }
        }
        let focused = if self.pane(self.focused).is_some() {
            self.focused
        } else {
            self.layout.first_pane_id().expect("a pane is left")
        };
        self.focus_pane(focused, window, cx);
    }

    /// move tab `ix` of `pane_id` into a new window, placed with its tab at `at` in screen
    /// coordinates when known
    pub(super) fn move_tab_to_new_window(
        &mut self,
        pane_id: usize,
        ix: usize,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(moved) = self.moving_tab(pane_id, ix, cx) else {
            return;
        };
        let mut bounds = window.bounds();
        if let Some(at) = at {
            bounds.origin = at - GRAB_OFFSET;
        }
        // opened before the tab leaves, so a window giving away its last tab never quits
        if crate::ui::windows::open_moved_tab_window(moved, bounds, cx).is_some() {
            self.forget_moved_tab(pane_id, ix, window, cx);
        }
    }

    /// move tab `ix` of `pane_id` into the focused pane of another window
    pub(super) fn move_tab_to_window(
        &mut self,
        pane_id: usize,
        ix: usize,
        target: WindowHandle<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(moved) = self.moving_tab(pane_id, ix, cx) else {
            return;
        };
        let adopted = target.update(cx, |workspace, window, cx| {
            workspace.adopt_tab(workspace.focused, moved, window, cx);
            window.activate_window();
        });
        if adopted.is_ok() {
            self.forget_moved_tab(pane_id, ix, window, cx);
        }
    }

    /// a dragged tab was let go at `position`, outside this window: it joins the kuterm window
    /// under the pointer, or opens a window of its own there
    pub(super) fn tab_dropped_outside(
        &mut self,
        pane_id: usize,
        ix: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let others: Vec<WindowHandle<Workspace>> = cx
            .windows()
            .into_iter()
            .filter(|handle| handle.window_id() != window.window_handle().window_id())
            .filter_map(|handle| handle.downcast::<Workspace>())
            .collect();
        let (at, target) = pointer_on_screen(position, &others, window, cx);
        match target {
            Some(target) => self.move_tab_to_window(pane_id, ix, target, window, cx),
            None => self.move_tab_to_new_window(pane_id, ix, at, window, cx),
        }
    }
}

/// where the pointer is in screen coordinates and the window of `others` under it. wayland
/// keeps both from clients, so there it is neither
fn pointer_on_screen(
    position: Point<Pixels>,
    others: &[WindowHandle<Workspace>],
    window: &Window,
    cx: &mut App,
) -> (Option<Point<Pixels>>, Option<WindowHandle<Workspace>>) {
    #[cfg(target_os = "linux")]
    {
        let _ = position;
        if cx.compositor_name() != "X11" {
            return (None, None);
        }
        let xid = |window: &Window| x11::window_id(window);
        let ids: Vec<(WindowHandle<Workspace>, u32)> = others
            .iter()
            .filter_map(|handle| {
                let any: AnyWindowHandle = (*handle).into();
                let id = any.update(cx, |_, window, _| xid(window)).ok()??;
                Some((*handle, id))
            })
            .collect();
        let scale = window.scale_factor();
        match x11::pointer(ids.iter().map(|(_, id)| *id)) {
            Ok((x, y, hit)) => (
                Some(gpui::point(px(x as f32 / scale), px(y as f32 / scale))),
                hit.map(|hit| ids[hit].0),
            ),
            Err(error) => {
                eprintln!("failed to find the pointer: {error}");
                (None, None)
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // window bounds are in screen coordinates here
        let at = window.bounds().origin + position;
        let target = others.iter().copied().find(|handle| {
            let any: AnyWindowHandle = (*handle).into();
            any.update(cx, |_, window, _| window.bounds().contains(&at))
                .unwrap_or(false)
        });
        (Some(at), target)
    }
}

#[cfg(target_os = "linux")]
mod x11 {
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use x11rb::{connection::Connection, protocol::xproto::ConnectionExt as _};

    /// x11 id of a gpui window
    pub(super) fn window_id(window: &Window) -> Option<u32> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
            RawWindowHandle::Xlib(handle) => Some(handle.window as u32),
            _ => None,
        }
    }

    /// pointer position on the root window, in pixels, and the index of the first of `windows`
    /// it is over
    pub(super) fn pointer(
        windows: impl Iterator<Item = u32>,
    ) -> anyhow::Result<(i16, i16, Option<usize>)> {
        // gpui keeps its connection to itself, a short lived one asks the server
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let pointer = conn.query_pointer(root)?.reply()?;
        let (x, y) = (pointer.root_x, pointer.root_y);
        let mut hit = None;
        for (ix, window) in windows.enumerate() {
            // frames of window managers put windows anywhere in their parents, so the pointer
            // is asked about in each window's own coordinates
            let Ok(geometry) = conn.get_geometry(window)?.reply() else {
                continue;
            };
            let local = conn.translate_coordinates(root, window, x, y)?.reply()?;
            let (lx, ly) = (i32::from(local.dst_x), i32::from(local.dst_y));
            if (0..i32::from(geometry.width)).contains(&lx)
                && (0..i32::from(geometry.height)).contains(&ly)
            {
                hit = Some(ix);
                break;
            }
        }
        Ok((x, y, hit))
    }
}

#[cfg(test)]
mod tests {

    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn tabs_move_between_windows_with_their_shells(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let moved = terminal(&ws, cx, 1);
        ws.update(cx, |ws, _| {
            let tab = &mut ws.pane_mut(ws.focused).unwrap().tabs[1];
            tab.custom_title = Some("server".into());
        });
        run_actions(&ws, cx, r#"{"activate_tab": 2}, "move_tab_to_new_window""#);
        let windows = workspaces(cx);
        assert_eq!(windows.len(), 2);
        let first_id = cx.update(|window, _| window.window_handle().window_id());
        let first = windows.iter().copied().find(|w| w.window_id() == first_id);
        let first = first.expect("first window is gone");
        let second = windows.into_iter().find(|w| *w != first).unwrap();
        // the same terminal, so the shell and its history came along
        assert_eq!(terminals_of(first, cx).len(), 1);
        assert_eq!(terminals_of(second, cx), [moved.entity_id()]);
        let title = cx.cx.update(|cx| {
            second
                .read_with(cx, |ws, _| ws.focused_pane().tabs[0].custom_title.clone())
                .unwrap()
        });
        assert_eq!(title.as_deref(), Some("server"));
        let pid = moved.read_with(cx, |t, _| t.shell_pid);
        assert!(
            crate::terminal::process_info(pid).is_some(),
            "shell died on the move"
        );

        // moved back, the window left empty closes and nothing quits
        cx.cx.update(|cx| {
            second
                .update(cx, |ws, window, cx| {
                    ws.move_tab_to_window(ws.focused, 0, first, window, cx)
                })
                .unwrap()
        });
        cx.run_until_parked();
        assert_eq!(workspaces(cx), [first]);
        assert_eq!(terminals_of(first, cx)[1], moved.entity_id());
        assert_eq!(current(&ws, cx), 1);
        assert!(!ws.update(cx, |ws, _| ws.quitting));
    }

    #[gpui::test]
    fn a_pinned_tab_joins_another_window_in_front_of_its_unpinned_tabs(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        let pinned = terminal(&ws, cx, 0);
        ws.update(cx, |ws, _| {
            ws.pane_mut(ws.focused).unwrap().tabs[0].pinned = true
        });
        run_actions(&ws, cx, r#"{"activate_tab": 3}, "move_tab_to_new_window""#);
        let first_id = cx.update(|window, _| window.window_handle().window_id());
        let second = workspaces(cx)
            .into_iter()
            .find(|w| w.window_id() != first_id)
            .expect("no second window");
        let unpinned = terminals_of(second, cx)[0];
        ws.update_in(cx, |ws, window, cx| {
            ws.move_tab_to_window(ws.focused, 0, second, window, cx)
        });
        cx.run_until_parked();
        assert_eq!(terminals_of(second, cx), [pinned.entity_id(), unpinned]);
        let (active, still_pinned) = cx.cx.update(|cx| {
            second
                .read_with(cx, |ws, _| {
                    let pane = ws.focused_pane();
                    (pane.active, pane.tabs[0].pinned)
                })
                .unwrap()
        });
        assert_eq!(active, 0);
        assert!(still_pinned);
    }
}
