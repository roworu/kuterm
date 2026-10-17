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
        let settings = Settings::get(cx);
        Some(MovedTab {
            terminal: tab.view.read(cx).terminal().clone(),
            // a profile renamed or removed since falls back to the default one
            profile: settings
                .profile(&tab.profile)
                .unwrap_or_else(|| settings.default_profile())
                .clone(),
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
