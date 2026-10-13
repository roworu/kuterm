//! root view: owns tabs and handles new tab / close tab.

mod app_menu;
mod config_watch;
mod end_truncated;
mod notifications;
mod session;
mod tab_bar;
mod tab_icon;
mod tab_title;
mod window;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{
    FutureExt, StreamExt,
    channel::mpsc::{UnboundedSender, unbounded},
};
use gpui::{
    Action, App, Bounds, ClipboardItem, Context, DismissEvent, Entity, EntityId, Focusable,
    ManagedView, Pixels, Point, ScrollHandle, Size, Subscription, Task, Window, WindowAppearance,
    actions, prelude::*,
};

pub use app_menu::{app_menus, dock_menu, open_urls};
pub use config_watch::watch_config;
use notifications::Notification;
use tab_title::TitleInputs;
pub use window::{background_appearance, startup_bounds, window_decorations};

use crate::{
    cli::Cli,
    settings::{
        Command, CommandAction, Commands, Config, ConfigFiles, ConfirmQuit, Profile,
        RestoreSession, Settings, Shell, TabIconSettings, TabIcons, TabTitleBlock,
    },
    terminal::{
        Event, Terminal, TerminalBounds, TerminalBuilder, foreground_process,
        with_shell_integration,
    },
    theme::{self, Theme},
    ui::{
        command_palette::{
            About, CloseConfirmed, CloseTarget, CommandPalette, ConfirmClose, ConfirmPaste,
            PasteConfirmed,
        },
        terminal_view::{ClipboardEvent, TerminalView, copied_text},
    },
};

actions!(
    workspace,
    [
        NewTab,
        CloseTab,
        NextTab,
        ToggleCommandPalette,
        ToggleFullscreen,
        OpenAbout,
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom
    ]
);

/// activate tab at this 0 based index
#[derive(Clone, PartialEq, Action)]
#[action(namespace = workspace, no_json)]
pub struct ActivateTab(pub usize);

/// run one command palette action, for menu items and keys with no action of their own
#[derive(Clone, PartialEq, Action)]
#[action(namespace = workspace, no_json)]
pub struct RunAction(pub CommandAction);

// programs, folders and command output change without events, so titles are polled
const TITLE_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
// output also refreshes titles, so short commands like "sudo dnf check-update" show up
// between polls. busy output asks many times a second, so its refreshes are spaced by this
const TITLE_REFRESH_MIN_GAP: Duration = Duration::from_millis(200);
// how many recently run commands the palette remembers
const RECENT_COMMANDS_MAX: usize = 10;
// how many program names the quit dialog lists before cutting the list short
const QUIT_DIALOG_MAX_NAMES: usize = 3;
// a fresh shell prints startup output before its prompt, so typed input waits for a short
// quiet gap. writing sooner lets the tty echo the input before the shell reads it
const SHELL_READY_SETTLE: Duration = Duration::from_millis(50);

// tab title blocks, window title blocks, icon settings, program icons, active tab, per tab inputs
type TitleSnapshot = (
    Vec<TabTitleBlock>,
    Vec<TabTitleBlock>,
    TabIconSettings,
    TabIcons,
    usize,
    Vec<(EntityId, TitleInputs)>,
);

struct Tab {
    view: Entity<TerminalView>,
    /// name of the profile it was opened with
    profile: String,
    /// built from `tab_title` blocks, empty until the first refresh
    title: String,
    icon: String,
    profile_icon: Option<String>,
    /// true once the shell is done starting, so writing to it now is not echoed by the tty
    ready: bool,
    /// fires after a quiet gap and releases input queued for a still starting shell,
    /// replacing it on newer output drops the old task and restarts the gap
    ready_task: Task<()>,
    /// input a command queued before the shell was ready
    pending_input: Vec<u8>,
    /// the bell rang while the tab was in the background, cleared when it is shown
    bell: bool,
    /// file only this tab uses, like the history its pager shows, removed with the tab
    temp_file: Option<PathBuf>,
    _subscriptions: [Subscription; 2],
}

impl Drop for Tab {
    fn drop(&mut self) {
        if let Some(path) = &self.temp_file {
            std::fs::remove_file(path).ok();
        }
    }
}

/// path of the remote control socket while kuterm listens on it
pub struct RemoteSocket(pub PathBuf);

impl gpui::Global for RemoteSocket {}

/// pane grouping its own tabs and active terminal
struct Pane {
    id: usize,
    tabs: Vec<Tab>,
    active: usize,
    /// tab row scroll state, also records where each tab was laid out
    tab_scroll: ScrollHandle,
}

/// how a split lays out its two children
#[derive(Clone, Copy, PartialEq, Debug)]
enum SplitAxis {
    /// children side by side, divider is vertical
    Horizontal,
    /// children stacked, divider is horizontal
    Vertical,
}

/// edge of a pane a dropped tab splits it at
#[derive(Clone, Copy, PartialEq, Debug)]
enum DropSide {
    Left,
    Right,
    Top,
    Bottom,
}

/// binary tree of panes, the leaves hold the tabs
enum LayoutNode {
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
    fn pane(&self, id: usize) -> Option<&Pane> {
        match self {
            Self::Leaf(pane) if pane.id == id => Some(pane),
            Self::Split { first, second, .. } => first.pane(id).or_else(|| second.pane(id)),
            Self::Empty | Self::Leaf(_) => None,
        }
    }

    fn pane_mut(&mut self, id: usize) -> Option<&mut Pane> {
        match self {
            Self::Leaf(pane) if pane.id == id => Some(pane),
            Self::Split { first, second, .. } => first.pane_mut(id).or_else(|| second.pane_mut(id)),
            Self::Empty | Self::Leaf(_) => None,
        }
    }

    /// pane id and index of the tab whose view has this id
    fn find_tab(&self, id: EntityId) -> Option<(usize, usize)> {
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

    fn contains(&self, id: usize) -> bool {
        match self {
            Self::Leaf(pane) => pane.id == id,
            Self::Split { first, second, .. } => first.contains(id) || second.contains(id),
            Self::Empty => false,
        }
    }

    /// pane at the top right corner of the window, where window buttons go
    fn corner_pane_id(&self) -> Option<usize> {
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

    fn first_pane_id(&self) -> Option<usize> {
        match self {
            Self::Leaf(pane) => Some(pane.id),
            Self::Split { first, .. } => first.first_pane_id(),
            Self::Empty => None,
        }
    }

    /// panes in layout order
    fn panes(&self) -> Vec<&Pane> {
        let mut panes = Vec::new();
        self.collect_panes(&mut panes);
        panes
    }

    fn collect_panes<'a>(&'a self, panes: &mut Vec<&'a Pane>) {
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
    fn pane_ids(&self) -> Vec<usize> {
        self.panes().iter().map(|pane| pane.id).collect()
    }

    /// split the leaf `target` at `side`, `new_pane` lands on that side
    fn split_leaf(&mut self, target: usize, side: DropSide, new_pane: Pane) -> bool {
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
    fn remove_leaf(&mut self, id: usize) -> Option<Pane> {
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
struct PaneRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl LayoutNode {
    /// every pane with its place, for moving focus between panes by direction
    fn rects(&self, rect: PaneRect, out: &mut Vec<(usize, PaneRect)>) {
        match self {
            Self::Leaf(pane) => out.push((pane.id, rect)),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (a, b) = match axis {
                    SplitAxis::Horizontal => (
                        PaneRect {
                            w: rect.w * ratio,
                            ..rect
                        },
                        PaneRect {
                            x: rect.x + rect.w * ratio,
                            w: rect.w * (1. - ratio),
                            ..rect
                        },
                    ),
                    SplitAxis::Vertical => (
                        PaneRect {
                            h: rect.h * ratio,
                            ..rect
                        },
                        PaneRect {
                            y: rect.y + rect.h * ratio,
                            h: rect.h * (1. - ratio),
                            ..rect
                        },
                    ),
                };
                first.rects(a, out);
                second.rects(b, out);
            }
            Self::Empty => {}
        }
    }

    /// panes side by side along `axis`, nested splits the other way count as one
    fn panes_along(&self, axis: SplitAxis) -> usize {
        match self {
            Self::Split {
                axis: own,
                first,
                second,
                ..
            } if *own == axis => first.panes_along(axis) + second.panes_along(axis),
            Self::Split { first, second, .. } => {
                first.panes_along(axis).max(second.panes_along(axis))
            }
            Self::Leaf(_) | Self::Empty => 1,
        }
    }

    /// size splits so panes in a row or column get the same share
    fn equalize(&mut self) {
        if let Self::Split {
            axis,
            ratio,
            first,
            second,
        } = self
        {
            let (a, b) = (first.panes_along(*axis), second.panes_along(*axis));
            *ratio = a as f32 / (a + b) as f32;
            first.equalize();
            second.equalize();
        }
    }

    /// move the divider of the innermost `axis` split around pane `id` by `delta`,
    /// false when no such split holds it
    fn move_divider(&mut self, id: usize, axis: SplitAxis, delta: f32) -> bool {
        let Self::Split {
            axis: own,
            ratio,
            first,
            second,
        } = self
        else {
            return false;
        };
        let inner = if first.contains(id) { first } else { second };
        if !inner.contains(id) {
            return false;
        }
        if inner.move_divider(id, axis, delta) {
            return true;
        }
        if *own != axis {
            return false;
        }
        *ratio = (*ratio + delta).clamp(MIN_SPLIT_RATIO, 1.0 - MIN_SPLIT_RATIO);
        true
    }
}

/// pane next to `from` on `side`: overlapping it the other way, closest first
fn neighbor(rects: &[(usize, PaneRect)], from: usize, side: DropSide) -> Option<usize> {
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

fn overlap(a: f32, a_len: f32, b: f32, b_len: f32) -> f32 {
    (a + a_len).min(b + b_len) - a.max(b)
}

// share of the window one resize step moves a divider
const RESIZE_STEP: f32 = 0.05;

/// side of `bounds` the pointer at `position` is closest to
fn drop_side(bounds: Bounds<Pixels>, position: Point<Pixels>) -> DropSide {
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
const MIN_SPLIT_RATIO: f32 = 0.1;

/// node at `path`, false picks the first child and true the second
fn layout_at_mut<'a>(node: &'a mut LayoutNode, path: &[bool]) -> Option<&'a mut LayoutNode> {
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

pub struct Workspace {
    layout: LayoutNode,
    /// pane with focus, its active tab is the active tab of the window
    focused: usize,
    /// pane shown alone over the others, none shows every pane
    zoomed: Option<usize>,
    next_pane_id: usize,
    /// pane edge a dragged tab would split at, none while no tab drags over a pane
    drop_zone: Option<(usize, DropSide)>,
    /// built from `window_title` blocks for the active tab
    window_title: String,
    /// pane and place the profile menu was opened at, none while it is closed
    profile_menu: Option<(usize, Point<Pixels>)>,
    /// open command palette, none while it is closed
    palette: Option<Entity<CommandPalette>>,
    /// open about page, none while it is closed
    about: Option<Entity<About>>,
    /// open close tab dialog, none while it is closed
    confirm_close: Option<Entity<ConfirmClose>>,
    /// open paste dialog, none while it is closed
    confirm_paste: Option<Entity<ConfirmPaste>>,
    _overlay_subscriptions: Vec<Subscription>,
    /// labels of commands run from the palette, most recent first
    recent_commands: Vec<String>,
    /// shown notifications, oldest first
    notifications: Vec<Notification>,
    next_notification_id: usize,
    /// true asks for titles right away, false lets output driven refreshes wait for the gap
    refresh_titles: UnboundedSender<bool>,
    _title_task: Task<()>,
    /// set once quitting is decided, so closing the window asks nothing more
    quitting: bool,
    /// writes the window size once resizing stops
    _size_save: Task<()>,
    /// appearance the themes were last picked for
    appearance: WindowAppearance,
}

/// spawn `profile` in a pty, with shell integration and the remote socket added
fn spawn_profile(
    profile: &Profile,
    bounds: TerminalBounds,
    window_id: u64,
    cx: &App,
) -> anyhow::Result<TerminalBuilder> {
    let settings = Settings::get(cx);
    let mut profile = if settings.shell_integration.inject {
        with_shell_integration(profile)
    } else {
        profile.clone()
    };
    // so `kuterm msg` run in the tab reaches this kuterm
    if let Some(socket) = cx.try_global::<RemoteSocket>() {
        profile
            .env
            .entry("KUTERM_SOCKET".into())
            .or_insert_with(|| socket.0.to_string_lossy().into_owned());
    }
    TerminalBuilder::new(&settings.terminal, &profile, bounds, window_id)
}

impl Workspace {
    /// create workspace with one terminal tab
    #[cfg(test)]
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_start(None, None, window, cx)
    }

    /// spawn the shell of a new window's first tab before the window exists, so it starts
    /// while gpu setup runs. none when the window brings the last session back instead
    pub fn spawn_first_tab(window_size: Size<Pixels>, cx: &App) -> Option<TerminalBuilder> {
        let settings = Settings::get(cx);
        if cx.windows().is_empty() && settings.restore_session == RestoreSession::Always {
            return None;
        }
        let bounds = window::startup_grid(window_size, cx);
        // the window id is not known yet. it only fills WINDOWID, and gpui's id is no x11 id
        spawn_profile(settings.default_profile(), bounds, 0, cx)
            .inspect_err(|error| eprintln!("failed to spawn terminal: {error:#}"))
            .ok()
    }

    /// create a workspace, its first tab in `folder` when given, else last session or a new
    /// tab, running the `first` shell when one was spawned already
    pub fn with_start(
        folder: Option<PathBuf>,
        first: Option<TerminalBuilder>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (refresh_titles, mut refresh_rx) = unbounded();
        let title_task = cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok((blocks, window_blocks, icon_settings, icons, active, inputs)) =
                    this.update(cx, |this, cx| this.title_inputs(cx))
                else {
                    break;
                };
                let shared = Arc::new((blocks, window_blocks, icon_settings, icons));
                // /proc reads and exec blocks may block, keep them off the main thread.
                // every tab builds on its own, so a slow exec block holds back only its tab
                let builds = inputs
                    .into_iter()
                    .enumerate()
                    .map(|(ix, (id, inputs))| {
                        let shared = shared.clone();
                        cx.background_executor().spawn(async move {
                            let (blocks, window_blocks, icon_settings, icons) = &*shared;
                            // read once, the window title, tab title and icon all need it
                            let process = foreground_process(inputs.shell_pid);
                            let window_title = (ix == active).then(|| {
                                tab_title::build_title(window_blocks, &inputs, process.as_ref())
                            });
                            let title = tab_title::build_title(blocks, &inputs, process.as_ref());
                            let icon = tab_icon::tab_icon(
                                icon_settings,
                                icons,
                                inputs.profile_icon.as_deref(),
                                process,
                            );
                            (id, title, icon, window_title)
                        })
                    })
                    .collect::<Vec<_>>();
                let built_at = Instant::now();
                let mut window_title = String::new();
                let titles = futures::future::join_all(builds)
                    .await
                    .into_iter()
                    .map(|(id, title, icon, active_title)| {
                        if let Some(active_title) = active_title {
                            window_title = active_title;
                        }
                        (id, title, icon)
                    })
                    .collect();
                let Ok(()) = this.update_in(cx, |this, window, cx| {
                    this.set_titles(titles, window_title, window, cx)
                }) else {
                    break;
                };
                let mut timer = cx
                    .background_executor()
                    .timer(TITLE_REFRESH_INTERVAL)
                    .fuse();
                let urgent = futures::select_biased! {
                    urgent = refresh_rx.next() => urgent.unwrap_or(true),
                    _ = timer => true,
                };
                // output asks many times a second, so its refreshes are spaced out,
                // while new, moved or closed tabs get their titles right away
                if !urgent {
                    let mut gap = cx
                        .background_executor()
                        .timer(TITLE_REFRESH_MIN_GAP.saturating_sub(built_at.elapsed()))
                        .fuse();
                    loop {
                        futures::select_biased! {
                            _ = gap => break,
                            urgent = refresh_rx.next() => if urgent != Some(false) {
                                break;
                            },
                        }
                    }
                }
                // many requests while building count as one
                while refresh_rx.try_recv().is_ok() {}
            }
        });
        let mut this = Self {
            layout: LayoutNode::Leaf(Pane {
                id: 0,
                tabs: Vec::new(),
                active: 0,
                tab_scroll: ScrollHandle::new(),
            }),
            focused: 0,
            zoomed: None,
            next_pane_id: 1,
            drop_zone: None,
            window_title: String::new(),
            profile_menu: None,
            palette: None,
            about: None,
            confirm_close: None,
            confirm_paste: None,
            _overlay_subscriptions: Vec::new(),
            recent_commands: Vec::new(),
            notifications: Vec::new(),
            next_notification_id: 0,
            refresh_titles,
            _title_task: title_task,
            quitting: false,
            _size_save: Task::ready(()),
            appearance: theme::appearance(window, cx),
        };
        // the window manager's close button goes through here, false keeps the window open
        let workspace = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            workspace
                .update(cx, |this, cx| this.should_close_window(window, cx))
                .unwrap_or(true)
        });
        // gpui also calls this on frame changes, which change no colors
        cx.observe_window_appearance(window, |this: &mut Self, window, cx| {
            let appearance = theme::appearance(window, cx);
            if this.appearance != appearance {
                this.reload_themes(window, cx);
            }
        })
        .detach();
        cx.observe_window_bounds(window, |this: &mut Self, window, cx| {
            this.window_resized(window, cx);
        })
        .detach();
        window::apply_startup_mode(window, cx);
        if let Some(folder) = folder {
            this.open_folder(folder, window, cx);
        } else {
            // only the first window brings the last session back, a window being built
            // is not listed yet
            let alone = cx.windows().is_empty();
            if !(alone && this.open_session(window, cx)) {
                match first {
                    Some(builder) => {
                        let profile = Settings::get(cx).default_profile().clone();
                        this.add_builder_tab(this.focused, builder, &profile, true, window, cx);
                    }
                    None => this.add_tab(window, cx),
                }
            }
        }
        // config problems from launch, printed before any window could show them
        if cx.has_global::<ConfigFiles>() {
            for problem in std::mem::take(&mut cx.global_mut::<ConfigFiles>().problems) {
                this.show_error(problem, cx);
            }
        }
        this
    }

    fn pane(&self, id: usize) -> Option<&Pane> {
        self.layout.pane(id)
    }

    fn pane_mut(&mut self, id: usize) -> Option<&mut Pane> {
        self.layout.pane_mut(id)
    }

    fn focused_pane(&self) -> &Pane {
        self.pane(self.focused)
            .expect("focused pane is in the layout")
    }

    /// pane and tab index of the tab with this view id
    fn find_id(&self, id: EntityId) -> Option<(usize, usize)> {
        // runs on every output wakeup, so it walks the tree without collecting panes
        self.layout.find_tab(id)
    }

    fn find_view(&self, view: &Entity<TerminalView>) -> Option<(usize, usize)> {
        self.find_id(view.entity_id())
    }

    /// tab with this view, wherever it lives
    fn tab_mut(&mut self, view: &Entity<TerminalView>) -> Option<&mut Tab> {
        let (pane_id, ix) = self.find_view(view)?;
        Some(&mut self.pane_mut(pane_id)?.tabs[ix])
    }

    /// give focus to a pane, focusing its active tab
    fn focus_pane(&mut self, pane_id: usize, window: &mut Window, cx: &mut Context<Self>) {
        // a zoomed pane hides the others, so going to one of them ends the zoom
        if self.zoomed.is_some_and(|zoomed| zoomed != pane_id) {
            self.zoomed = None;
        }
        self.focused = pane_id;
        if let Some(pane) = self.pane(pane_id)
            && let Some(tab) = pane.tabs.get(pane.active)
        {
            tab.view.focus_handle(cx).focus(window, cx);
        }
        self.refresh_titles();
        cx.notify();
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

    /// open a tab with the default profile and switch to it, in the folder the shell of the
    /// active tab reported when the profile names none
    pub fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut profile = Settings::get(cx).default_profile().clone();
        if profile.working_directory.is_none() {
            let pane = self.focused_pane();
            profile.working_directory = pane.tabs.get(pane.active).and_then(|tab| {
                let terminal = tab.view.read(cx).terminal().read(cx);
                terminal.reported_cwd().map(Path::to_path_buf)
            });
        }
        self.add_profile_tab(self.focused, &profile, true, window, cx);
    }

    /// open a tab with the default profile in `folder`
    pub fn open_folder(&mut self, folder: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let mut profile = Settings::get(cx).default_profile().clone();
        profile.working_directory = Some(folder);
        self.add_profile_tab(self.focused, &profile, true, window, cx);
    }

    /// open a tab with `profile` in pane `pane_id`, switching to it when `activate`
    fn add_profile_tab(
        &mut self,
        pane_id: usize,
        profile: &Profile,
        activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let window_id = window.window_handle().window_id().as_u64();
        let builder = match spawn_profile(profile, TerminalBounds::default(), window_id, cx) {
            Ok(builder) => builder,
            Err(error) => {
                eprintln!("failed to spawn terminal: {error:#}");
                return;
            }
        };
        self.add_builder_tab(pane_id, builder, profile, activate, window, cx);
    }

    /// show the shell of `builder`, spawned from `profile`, in a new tab of pane `pane_id`,
    /// switching to it when `activate`
    fn add_builder_tab(
        &mut self,
        pane_id: usize,
        builder: TerminalBuilder,
        profile: &Profile,
        activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let appearance = self.appearance;
        let terminal = cx.new(|cx| {
            let mut terminal = builder.subscribe(cx);
            let settings = Settings::get(cx);
            terminal.apply_theme(appearance, &settings.theme);
            terminal.configure_graphics(settings.graphics.enable, settings.graphics.max_memory_mb);
            terminal
        });
        self.attach_terminal(pane_id, terminal, profile, activate, window, cx);
    }

    /// show `terminal` in a new tab of pane `pane_id`, switching to it when `activate`
    fn attach_terminal(
        &mut self,
        pane_id: usize,
        terminal: Entity<Terminal>,
        profile: &Profile,
        activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| TerminalView::new(terminal.clone(), window, cx));

        let subscription = cx.subscribe_in(&terminal, window, {
            let view = view.clone();
            move |this, _, event: &Event, window, cx| match event {
                Event::TitleChanged => this.refresh_titles_later(),
                // shell exited, so close its tab
                // TODO: do we need it at all? probably we need a setting, if to keep exited tab..
                Event::CloseTerminal => {
                    if let Some((pane_id, ix)) = this.find_view(&view) {
                        this.close_tab_at(pane_id, ix, window, cx);
                    }
                }
                // a new program often starts by printing something
                Event::Wakeup => {
                    this.schedule_ready(&view, window, cx);
                    this.refresh_titles_later();
                }
                Event::Bell => this.ring_bell(&view, window, cx),
                Event::ClipboardAsk { id, text } => {
                    this.ask_clipboard(&view, *id, text.clone(), cx)
                }
                Event::CommandFinished { duration, exit } => {
                    this.command_finished(&view, *duration, *exit, window, cx)
                }
                Event::Notify { title, body } => {
                    this.program_notification(&view, title, body, window, cx)
                }
            }
        });
        let clipboard_subscription = cx.subscribe_in(
            &view,
            window,
            |this, view, event: &ClipboardEvent, window, cx| match event {
                ClipboardEvent::Copied(text) => this.notify_copied(text, cx),
                ClipboardEvent::Pasted => this.notify_pasted(cx),
                ClipboardEvent::ConfirmPaste(text) => {
                    this.open_confirm_paste(view.clone(), text.clone(), window, cx)
                }
            },
        );

        let Some(pane) = self.pane_mut(pane_id) else {
            return;
        };
        pane.tabs.push(Tab {
            view,
            profile: profile.name.clone(),
            title: String::new(),
            icon: profile
                .icon
                .clone()
                .unwrap_or_else(|| Settings::get(cx).tab_icon.default.clone()),
            profile_icon: profile.icon.clone(),
            ready: false,
            ready_task: Task::ready(()),
            pending_input: Vec::new(),
            bell: false,
            temp_file: None,
            _subscriptions: [subscription, clipboard_subscription],
        });
        let ix = pane.tabs.len() - 1;
        if activate {
            self.activate_tab(pane_id, ix, window, cx);
        } else {
            self.refresh_titles();
            cx.notify();
        }
    }

    /// sound, window urgency and tab mark for a bell rung in the tab of `view`
    fn ring_bell(
        &mut self,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bell = &Settings::get(cx).bell;
        if bell.audio {
            window.play_system_bell();
        }
        if bell.urgent && !window.is_window_active() {
            window.request_attention();
        }
        if bell.tab_indicator
            && let Some((pane_id, ix)) = self.find_view(view)
            && let Some(pane) = self.pane_mut(pane_id)
            && pane.active != ix
        {
            pane.tabs[ix].bell = true;
            cx.notify();
        }
    }

    /// the shell of a tab printed something, wait for it to quiet down before calling it ready
    fn schedule_ready(
        &mut self,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tab_mut(view) else {
            return;
        };
        if tab.ready {
            return;
        }
        let view = view.clone();
        tab.ready_task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SHELL_READY_SETTLE).await;
            this.update(cx, |this, cx| this.mark_ready(&view, cx)).ok();
        });
    }

    /// the shell stopped printing, so it is at a prompt and can take typed input
    fn mark_ready(&mut self, view: &Entity<TerminalView>, cx: &mut Context<Self>) {
        let Some(tab) = self.tab_mut(view) else {
            return;
        };
        tab.ready = true;
        if tab.pending_input.is_empty() {
            return;
        }
        let input = std::mem::take(&mut tab.pending_input);
        let terminal = tab.view.read(cx).terminal().clone();
        terminal.update(cx, |terminal, _| terminal.input(input));
    }

    /// view of the tab at `ix` in pane `pane_id`
    fn view_at(&self, pane_id: usize, ix: usize) -> Entity<TerminalView> {
        self.pane(pane_id).expect("pane from caller").tabs[ix]
            .view
            .clone()
    }

    /// last tab of the focused pane, where background tabs are added
    fn last_tab_of_focused(&self) -> Option<(usize, usize)> {
        let count = self.focused_pane().tabs.len();
        Some((self.focused, count.saturating_sub(1)))
    }

    /// terminal of the tab at `ix` in pane `pane_id`
    fn terminal_at(&self, pane_id: usize, ix: usize, cx: &App) -> Entity<Terminal> {
        self.view_at(pane_id, ix).read(cx).terminal().clone()
    }

    /// write input to the tab at `ix` in pane `pane_id`, holding it back until a fresh shell is ready
    fn input_to(&mut self, pane_id: usize, ix: usize, input: Vec<u8>, cx: &mut Context<Self>) {
        let tab = &mut self.pane_mut(pane_id).expect("pane from caller").tabs[ix];
        if tab.ready {
            let terminal = tab.view.read(cx).terminal().clone();
            terminal.update(cx, |terminal, _| terminal.input(input));
        } else {
            // a shell still starting echoes raw input from the tty, so wait for its first output
            tab.pending_input.extend_from_slice(&input);
        }
    }

    fn activate_tab(
        &mut self,
        pane_id: usize,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // focus leaves the close dialog, and a dialog without focus could not be answered
        if self.confirm_close.take().is_some() {
            self._overlay_subscriptions.clear();
        }
        let view = {
            let Some(pane) = self.pane_mut(pane_id) else {
                return;
            };
            pane.active = ix;
            pane.tabs[ix].bell = false;
            pane.tab_scroll.scroll_to_item(ix);
            pane.tabs[ix].view.clone()
        };
        self.focused = pane_id;
        // window title follows the active tab, and tab numbers shift after add or close
        self.refresh_titles();
        view.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// move a tab within pane `pane_id`, keeping the active tab on its view
    fn move_tab(&mut self, pane_id: usize, from: usize, to: usize, cx: &mut Context<Self>) {
        {
            let Some(pane) = self.pane_mut(pane_id) else {
                return;
            };
            if from == to || from >= pane.tabs.len() || to >= pane.tabs.len() {
                return;
            }
            let tab = pane.tabs.remove(from);
            pane.tabs.insert(to, tab);
            // active follows its view: it moves with the tab, or shifts when another lands across it
            let active = &mut pane.active;
            if *active == from {
                *active = to;
            } else if from < *active && *active <= to {
                *active -= 1;
            } else if *active < from && to <= *active {
                *active += 1;
            }
        }
        // titles show the tab number, which changed for the shifted tabs
        self.refresh_titles();
        cx.notify();
    }

    /// take the tab at `ix` out of pane `pane_id`, the bool is true when the pane emptied
    fn take_tab(&mut self, pane_id: usize, ix: usize) -> Option<(Tab, bool)> {
        let pane = self.pane_mut(pane_id)?;
        if ix >= pane.tabs.len() {
            return None;
        }
        let tab = pane.tabs.remove(ix);
        // the active tab follows its view, landing on the tab on its left when it closes
        // TODO: should be configurable
        pane.active = if ix < pane.active || (ix == pane.active && ix > 0) {
            pane.active - 1
        } else {
            pane.active.min(pane.tabs.len().saturating_sub(1))
        };
        Some((tab, pane.tabs.is_empty()))
    }

    /// update the split edge a dragged tab would drop at
    fn update_drop_zone(
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
    fn drop_tab_on_pane(
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
    fn split_with_tab(
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
            let mut profile = Settings::get(cx).default_profile().clone();
            if Settings::get(cx).panes.split_inherits_cwd
                && let Some(cwd) = tab_cwd(&tab, cx)
            {
                profile.working_directory = Some(cwd);
            }
            self.add_profile_tab(src_pane, &profile, false, window, cx);
        } else if empty {
            self.layout.remove_leaf(src_pane);
            if self.focused == src_pane {
                self.focused = self.layout.first_pane_id().unwrap_or(target);
            }
        }
        let new_pane = Pane {
            id: self.next_pane_id,
            tabs: vec![tab],
            active: 0,
            tab_scroll: ScrollHandle::new(),
        };
        let new_id = new_pane.id;
        self.next_pane_id += 1;
        self.layout.split_leaf(target, side, new_pane);
        self.focus_pane(new_id, window, cx);
    }

    /// move the tab at `src_ix` of `src_pane` to `dst_ix` of `dst_pane`
    fn move_tab_to_pane(
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
        let ix = dst_ix.min(dst.tabs.len());
        dst.tabs.insert(ix, tab);
        dst.active = ix;
        self.focus_pane(dst_pane, window, cx);
    }

    /// focus the pane next to the focused one on `side`
    fn focus_pane_toward(&mut self, side: DropSide, window: &mut Window, cx: &mut Context<Self>) {
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
    fn resize_pane(&mut self, side: DropSide, cx: &mut Context<Self>) {
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

    /// close every tab of the focused pane, asking once when programs still run in them
    fn request_close_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.focused_pane().tabs.len();
        self.request_close_range(self.focused, 0..count, window, cx);
    }

    /// every tab with its title and folder, for remote control
    pub fn tab_infos(&self, window_id: u64, cx: &App) -> Vec<crate::remote::TabInfo> {
        self.layout
            .panes()
            .into_iter()
            .flat_map(|pane| {
                (0..pane.tabs.len()).map(move |ix| crate::remote::TabInfo {
                    window: window_id,
                    pane: pane.id,
                    number: ix + 1,
                    title: self.tab_title(pane.id, ix, cx),
                    active: pane.id == self.focused && ix == pane.active,
                    cwd: tab_cwd(&pane.tabs[ix], cx),
                })
            })
            .collect()
    }

    /// open a tab with the named profile, or the default one, in `cwd` when given
    pub fn open_tab(
        &mut self,
        profile: Option<&str>,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let settings = Settings::get(cx);
        let mut profile = match profile {
            Some(name) => settings
                .profile(name)
                .cloned()
                .ok_or_else(|| format!("no profile named {name:?}"))?,
            None => settings.default_profile().clone(),
        };
        if cwd.is_some() {
            profile.working_directory = cwd;
        }
        self.add_profile_tab(self.focused, &profile, true, window, cx);
        Ok(())
    }

    /// type `text` into the active tab
    pub fn send_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let (pane, ix) = (self.focused, self.focused_pane().active);
        self.input_to(pane, ix, text.as_bytes().to_vec(), cx);
    }

    /// write `text` to a temp file and open it with `scrollback_pager` in a new tab
    fn open_scrollback(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "kuterm-scrollback-{}-{number}.txt",
            std::process::id()
        ));
        if let Err(error) = std::fs::write(&path, text) {
            self.show_error(format!("failed to write {}: {error}", path.display()), cx);
            return;
        }
        let settings = Settings::get(cx);
        let mut profile = settings.default_profile().clone();
        let (program, args) = settings
            .scrollback_pager
            .split_first()
            .expect("settings keep a pager program");
        profile.command = Shell::WithArguments {
            program: program.clone(),
            args: args
                .iter()
                .cloned()
                .chain([path.to_string_lossy().into_owned()])
                .collect(),
        };
        self.add_profile_tab(self.focused, &profile, true, window, cx);
        // removed with its tab
        if let Some(tab) = self
            .pane_mut(self.focused)
            .and_then(|pane| pane.tabs.last_mut())
        {
            tab.temp_file = Some(path);
        }
    }

    /// set the ratio of the split at `path`
    fn resize_split(&mut self, path: &[bool], ratio: f32, cx: &mut Context<Self>) {
        let Some(LayoutNode::Split { ratio: split, .. }) = layout_at_mut(&mut self.layout, path)
        else {
            return;
        };
        *split = ratio.clamp(MIN_SPLIT_RATIO, 1.0 - MIN_SPLIT_RATIO);
        cx.notify();
    }

    /// open a tab with the named profile in pane `pane_id`, notifying when there is none
    fn add_named_profile_tab(
        &mut self,
        pane_id: usize,
        name: &str,
        activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(profile) = Settings::get(cx).profile(name).cloned() else {
            self.show_notification(format!("no profile named {name:?}"), None, cx);
            return false;
        };
        self.add_profile_tab(pane_id, &profile, activate, window, cx);
        true
    }

    /// name of the program running in the tab at `ix` of pane `pane_id`, none when only the shell is there
    fn running_program(&self, pane_id: usize, ix: usize, cx: &App) -> Option<String> {
        let shell_pid = self.terminal_at(pane_id, ix, cx).read(cx).shell_pid;
        foreground_process(shell_pid)
            .filter(|process| process.pid != shell_pid)
            .map(|process| process.name)
    }

    /// close the tab at `ix` of `pane_id`, asking first when a program still runs in it
    fn request_close_tab(
        &mut self,
        pane_id: usize,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // actions run before the dialog sees keys, so a second close must not replace the dialog
        if self.confirm_close.is_some() {
            return;
        }
        let warn = Settings::get(cx).close_running_tab_warn;
        let Some(program) = warn
            .then(|| self.running_program(pane_id, ix, cx))
            .flatten()
        else {
            self.close_tab_at(pane_id, ix, window, cx);
            return;
        };
        let message = format!(
            "\"{program}\" is still running in tab {} \"{}\"",
            ix + 1,
            self.tab_title(pane_id, ix, cx)
        );
        let id = self.view_at(pane_id, ix).entity_id();
        self.open_confirm(message, CloseTarget::Tabs(vec![id]), window, cx);
    }

    /// close every tab on one side of `ix` in `pane_id`, asking once when programs still run in them
    fn request_close_tabs_side(
        &mut self,
        pane_id: usize,
        ix: usize,
        right: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.pane(pane_id).expect("pane from caller").tabs.len();
        let range = if right { ix + 1..count } else { 0..ix };
        self.request_close_range(pane_id, range, window, cx);
    }

    /// close the tabs in `range` of `pane_id`, asking once when programs still run in them
    fn request_close_range(
        &mut self,
        pane_id: usize,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.confirm_close.is_some() || range.is_empty() {
            return;
        }
        let ids: Vec<EntityId> = range
            .clone()
            .map(|ix| self.view_at(pane_id, ix).entity_id())
            .collect();
        let names: Vec<String> = if Settings::get(cx).close_running_tab_warn {
            range
                .filter_map(|ix| self.running_program(pane_id, ix, cx))
                .collect()
        } else {
            Vec::new()
        };
        if names.is_empty() {
            self.close_tabs(&ids, window, cx);
        } else {
            self.open_confirm(
                side_close_message(&names),
                CloseTarget::Tabs(ids),
                window,
                cx,
            );
        }
    }

    /// close the tabs with these views, skipping any that are already gone
    fn close_tabs(&mut self, ids: &[EntityId], window: &mut Window, cx: &mut Context<Self>) {
        for id in ids {
            if let Some((pane_id, ix)) = self.find_id(*id) {
                self.close_tab_at(pane_id, ix, window, cx);
            }
        }
    }

    /// quit, asking first when programs still run in some tabs
    pub fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_close.is_some() {
            return;
        }
        if !self.ask_before_quit(window, cx) {
            self.quit(cx);
        }
    }

    /// the window is about to close, true lets it
    fn should_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.quitting {
            return true;
        }
        // a second close while asked keeps the dialog up
        let close = self.confirm_close.is_none() && !self.ask_before_quit(window, cx);
        if close {
            self.save_session(cx);
        }
        close
    }

    /// open the quit dialog when `confirm_quit` asks for it, true when it opened
    fn ask_before_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let confirm = Settings::get(cx).confirm_quit;
        if confirm == ConfirmQuit::Never {
            return false;
        }
        let names: Vec<String> = self
            .layout
            .panes()
            .into_iter()
            .flat_map(|pane| (0..pane.tabs.len()).map(move |ix| (pane.id, ix)))
            .filter_map(|(pane_id, ix)| self.running_program(pane_id, ix, cx))
            .collect();
        if names.is_empty() && confirm != ConfirmQuit::Always {
            return false;
        }
        self.open_confirm(quit_message(&names), CloseTarget::Window, window, cx);
        true
    }

    fn open_confirm(
        &mut self,
        message: String,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let confirm = cx.new(|cx| ConfirmClose::new(message, target.clone(), cx));
        self.open_overlay(&confirm, window, cx);
        self._overlay_subscriptions.push(cx.subscribe_in(
            &confirm,
            window,
            move |this, _, _: &CloseConfirmed, window, cx| {
                this.close_overlay(window, cx);
                match &target {
                    // tabs may have moved or closed while the dialog was open
                    CloseTarget::Tabs(ids) => this.close_tabs(ids, window, cx),
                    CloseTarget::Window => this.quit(cx),
                }
            },
        ));
        self.confirm_close = Some(confirm);
    }

    /// ask before pasting `text` into the tab of `view`
    fn open_confirm_paste(
        &mut self,
        view: Entity<TerminalView>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dialog = cx.new(|cx| ConfirmPaste::new(&text, cx));
        self.open_overlay(&dialog, window, cx);
        self._overlay_subscriptions.push(cx.subscribe_in(
            &dialog,
            window,
            move |this, _, _: &PasteConfirmed, window, cx| {
                this.close_overlay(window, cx);
                view.update(cx, |view, cx| view.force_paste(&text, cx));
            },
        ));
        self.confirm_paste = Some(dialog);
    }

    fn quit(&mut self, cx: &mut Context<Self>) {
        if !self.quitting {
            self.save_session(cx);
        }
        self.quitting = true;
        cx.quit();
    }

    fn close_tab_at(
        &mut self,
        pane_id: usize,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let old_active = self.pane(pane_id).map(|pane| pane.active);
        let Some((closed_tab, empty)) = self.take_tab(pane_id, ix) else {
            return;
        };
        let closed = closed_tab.view.entity_id();
        // the shell may exit while its tab is asked about, leaving nothing to confirm.
        // a batch keeps its dialog until every tab it asks about is gone
        let target = self
            .confirm_close
            .as_ref()
            .map(|c| c.read(cx).target.clone());
        if let Some(target) = target
            && target.contains(closed)
            && !self.any_target_present(&target)
        {
            self.close_overlay(window, cx);
        }
        if empty {
            let before = self.layout.pane_ids();
            let at = before.iter().position(|id| *id == pane_id);
            self.layout.remove_leaf(pane_id);
            if self.layout.first_pane_id().is_none() {
                // other windows stay, only the last one quits
                if cx.windows().len() > 1 {
                    self.quitting = true;
                    window.remove_window();
                } else {
                    self.quit(cx);
                }
                return;
            }
            if self.focused == pane_id {
                // focus the pane that takes the removed one's place, not the first pane
                let after = self.layout.pane_ids();
                self.focus_pane(after[at.unwrap_or(0).min(after.len() - 1)], window, cx);
            } else {
                self.refresh_titles();
                cx.notify();
            }
            return;
        }
        let active = self.pane(pane_id).expect("pane from take_tab").active;
        // a dialog that still asks about a remaining tab keeps focus and stays up
        if self.confirm_close.is_some() && Some(active) == old_active {
            self.refresh_titles();
            cx.notify();
        } else if self.focused == pane_id {
            self.activate_tab(pane_id, active, window, cx);
        } else {
            self.refresh_titles();
            cx.notify();
        }
    }

    /// true when some tab still in the layout is part of `target`
    fn any_target_present(&self, target: &CloseTarget) -> bool {
        self.layout
            .panes()
            .into_iter()
            .flat_map(|pane| &pane.tabs)
            .any(|tab| target.contains(tab.view.entity_id()))
    }

    /// read theme files again and recolor the ui and every terminal
    fn reload_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let appearance = theme::appearance(window, cx);
        self.appearance = appearance;
        Theme::apply(appearance, cx);
        for tab in self.layout.panes().into_iter().flat_map(|pane| &pane.tabs) {
            let terminal = tab.view.read(cx).terminal().clone();
            terminal.update(cx, |terminal, cx| {
                terminal.apply_theme(appearance, &Settings::get(cx).theme)
            });
        }
        // terminal views may be cached, force redraw everything with new colors
        window.refresh();
    }

    /// read the config and theme files again and apply them, a broken file keeps the
    /// running config and shows what is wrong
    fn reload_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let load = Config::load();
        // errors from an earlier try are fixed or shown again below
        self.notifications
            .retain(|notification| !notification.error);
        for warning in load.warnings {
            eprintln!("{warning}");
            self.show_error(warning, cx);
        }
        cx.set_global(ConfigFiles {
            paths: load.files,
            problems: Vec::new(),
        });
        let checked = load.config.and_then(|mut config| {
            Cli::get().apply(&mut config.settings);
            Theme::check(&config.settings)?;
            Ok(config)
        });
        let config = match checked {
            Ok(config) => config,
            Err(error) => {
                eprintln!("{error}");
                self.show_error(format!("{error}, keeping the last good config"), cx);
                return;
            }
        };
        let mut settings = config.settings;
        settings.use_installed_fonts(&cx.text_system().all_font_names());
        cx.set_cursor_hide_mode(settings.terminal.cursor_hide_mode());
        for tab in self.layout.panes().into_iter().flat_map(|pane| &pane.tabs) {
            let terminal = tab.view.read(cx).terminal().clone();
            terminal.update(cx, |terminal, _| {
                terminal.apply_settings(&settings.terminal);
                terminal
                    .configure_graphics(settings.graphics.enable, settings.graphics.max_memory_mb);
            });
        }
        cx.set_global(settings);
        cx.set_global(config.tab_icons);
        cx.set_global(config.commands);
        cx.clear_key_bindings();
        cx.bind_keys(config.keybindings.bindings());
        // text fields keep their editing keys, they are not part of the user keybindings
        cx.bind_keys(crate::ui::text_input::bindings());
        // menus show the keys bound to their actions, so they are rebuilt with the new keys
        cx.set_menus(app_menus());
        self.apply_window_settings(window, cx);
        // theme mode and theme files are picked in settings
        self.reload_themes(window, cx);
        self.refresh_titles();
        self.show_notification("config reloaded", None, cx);
    }

    /// run command actions in order against the active tab, or the last background tab
    fn run_command(&mut self, command: &Command, window: &mut Window, cx: &mut Context<Self>) {
        // tab later actions act on, the active tab of the focused pane when none
        let mut target: Option<(usize, usize)> = None;
        for action in &command.actions {
            // closing the last tab quits, nothing is left to act on
            if self.layout.first_pane_id().is_none() {
                return;
            }
            // a closed background tab falls back to the active tab of the focused pane
            let (pane, ix) = target
                .filter(|(pane, ix)| self.pane(*pane).is_some_and(|pane| *ix < pane.tabs.len()))
                .unwrap_or_else(|| (self.focused, self.focused_pane().active));
            match action {
                CommandAction::About => self.open_about(&OpenAbout, window, cx),
                CommandAction::ReloadConfig => self.reload_config(window, cx),
                CommandAction::NewTab => {
                    self.add_tab(window, cx);
                    target = None;
                }
                CommandAction::NewTabWithProfile(name) => {
                    if self.add_named_profile_tab(self.focused, name, true, window, cx) {
                        target = None;
                    }
                }
                CommandAction::NewBackgroundTab => {
                    let profile = Settings::get(cx).default_profile().clone();
                    self.add_profile_tab(self.focused, &profile, false, window, cx);
                    target = self.last_tab_of_focused();
                }
                CommandAction::NewBackgroundTabWithProfile(name) => {
                    if self.add_named_profile_tab(self.focused, name, false, window, cx) {
                        target = self.last_tab_of_focused();
                    }
                }
                CommandAction::CloseTab => {
                    self.request_close_tab(pane, ix, window, cx);
                    target = None;
                }
                CommandAction::CloseTabsToRight => {
                    self.request_close_tabs_side(pane, ix, true, window, cx);
                    target = None;
                }
                CommandAction::CloseTabsToLeft => {
                    self.request_close_tabs_side(pane, ix, false, window, cx);
                    target = None;
                }
                CommandAction::SplitLeft
                | CommandAction::SplitRight
                | CommandAction::SplitUp
                | CommandAction::SplitDown => {
                    let side = match action {
                        CommandAction::SplitLeft => DropSide::Left,
                        CommandAction::SplitRight => DropSide::Right,
                        CommandAction::SplitUp => DropSide::Top,
                        _ => DropSide::Bottom,
                    };
                    self.split_with_tab(pane, ix, pane, side, window, cx);
                    target = None;
                }
                CommandAction::ClosePane => {
                    self.request_close_pane(window, cx);
                    target = None;
                }
                CommandAction::FocusPaneLeft => self.focus_pane_toward(DropSide::Left, window, cx),
                CommandAction::FocusPaneRight => {
                    self.focus_pane_toward(DropSide::Right, window, cx)
                }
                CommandAction::FocusPaneUp => self.focus_pane_toward(DropSide::Top, window, cx),
                CommandAction::FocusPaneDown => {
                    self.focus_pane_toward(DropSide::Bottom, window, cx)
                }
                CommandAction::ResizePaneLeft => self.resize_pane(DropSide::Left, cx),
                CommandAction::ResizePaneRight => self.resize_pane(DropSide::Right, cx),
                CommandAction::ResizePaneUp => self.resize_pane(DropSide::Top, cx),
                CommandAction::ResizePaneDown => self.resize_pane(DropSide::Bottom, cx),
                CommandAction::ZoomPane => {
                    let single = self.layout.pane_ids().len() == 1;
                    self.zoomed = match self.zoomed {
                        Some(_) => None,
                        None if single => None,
                        None => Some(self.focused),
                    };
                }
                CommandAction::EqualizePanes => self.layout.equalize(),
                CommandAction::NextTab => {
                    self.next_tab(&NextTab, window, cx);
                    target = None;
                }
                CommandAction::PrevTab => {
                    let count = self.focused_pane().tabs.len();
                    let ix = (self.focused_pane().active + count - 1) % count;
                    self.activate_tab(self.focused, ix, window, cx);
                    target = None;
                }
                CommandAction::ActivateTab(number) => {
                    let count = self.focused_pane().tabs.len();
                    if let Some(ix) = number.checked_sub(1)
                        && ix < count
                    {
                        self.activate_tab(self.focused, ix, window, cx);
                        target = None;
                    }
                }
                CommandAction::PickTab => self.open_tab_picker(window, cx),
                CommandAction::Copy => {
                    let selection = copied_text(self.terminal_at(pane, ix, cx).read(cx), cx);
                    match selection {
                        Some(text) => {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            self.notify_copied(&text, cx);
                        }
                        None => self.show_notification("nothing selected to copy", None, cx),
                    }
                }
                CommandAction::Paste => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.view_at(pane, ix)
                            .update(cx, |view, cx| view.paste_text(text, cx));
                    }
                }
                CommandAction::Search => self
                    .view_at(pane, ix)
                    .update(cx, |view, cx| view.open_search(window, cx)),
                CommandAction::Hints => self
                    .view_at(pane, ix)
                    .update(cx, |view, cx| view.show_hints(cx)),
                CommandAction::ViMode => self
                    .view_at(pane, ix)
                    .update(cx, |view, cx| view.toggle_vi_mode(cx)),
                CommandAction::JumpToPrevPrompt | CommandAction::JumpToNextPrompt => {
                    let up = *action == CommandAction::JumpToPrevPrompt;
                    self.terminal_at(pane, ix, cx)
                        .update(cx, |terminal, _| terminal.jump_to_prompt(up));
                }
                CommandAction::SelectLastOutput => {
                    let found = self
                        .terminal_at(pane, ix, cx)
                        .update(cx, |terminal, _| terminal.select_last_output());
                    if !found {
                        self.show_notification(
                            "no command output, needs shell integration",
                            None,
                            cx,
                        );
                    }
                }
                CommandAction::CopyLastOutput => {
                    match self.terminal_at(pane, ix, cx).read(cx).last_output_text() {
                        Some(text) => {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            self.notify_copied(&text, cx);
                        }
                        None => self.show_notification(
                            "no command output, needs shell integration",
                            None,
                            cx,
                        ),
                    }
                }
                CommandAction::OpenScrollback => {
                    let text = self.terminal_at(pane, ix, cx).read(cx).scrollback_ansi();
                    self.open_scrollback(text, window, cx);
                    target = None;
                }
                CommandAction::ScrollUp(lines) => self
                    .terminal_at(pane, ix, cx)
                    .update(cx, |terminal, _| terminal.scroll(*lines)),
                CommandAction::ScrollDown(lines) => self
                    .terminal_at(pane, ix, cx)
                    .update(cx, |terminal, _| terminal.scroll(-*lines)),
                CommandAction::ScrollTop => {
                    self.terminal_at(pane, ix, cx).update(cx, |terminal, _| {
                        terminal.scroll_to(terminal.history_size())
                    })
                }
                CommandAction::ScrollBottom => self
                    .terminal_at(pane, ix, cx)
                    .update(cx, |terminal, _| terminal.scroll_to(0)),
                CommandAction::Quit => self.request_quit(window, cx),
                CommandAction::SelectAll => self
                    .terminal_at(pane, ix, cx)
                    .update(cx, |terminal, _| terminal.select_all()),
                CommandAction::Clear => self
                    .terminal_at(pane, ix, cx)
                    .update(cx, |terminal, _| terminal.clear()),
                CommandAction::IncreaseFontSize => self.change_font_size(pane, ix, Some(1.), cx),
                CommandAction::DecreaseFontSize => self.change_font_size(pane, ix, Some(-1.), cx),
                CommandAction::ResetFontSize => self.change_font_size(pane, ix, None, cx),
                CommandAction::OpenSettings => self.open_settings(cx),
                CommandAction::Type(text) => self.input_to(pane, ix, text.clone().into_bytes(), cx),
                CommandAction::SendText(text) => {
                    let bytes = text.clone().into_bytes();
                    self.terminal_at(pane, ix, cx)
                        .update(cx, |terminal, _| terminal.input(bytes));
                }
                CommandAction::Notify(text) => self.show_notification(text.clone(), None, cx),
                CommandAction::NotifyWhenDone(text) => {
                    self.notify_when_done(self.view_at(pane, ix), text.clone(), cx);
                }
            }
        }
        // scrolling and pasting change the grid without a wakeup, and terminal views are
        // cached, so the active tab is redrawn through its own view
        if let Some(pane) = self.pane(self.focused)
            && let Some(tab) = pane.tabs.get(pane.active)
        {
            tab.view.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    // closes whatever floats over the terminal first, only one overlay is open at a time
    fn open_overlay<V: ManagedView>(
        &mut self,
        view: &Entity<V>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(window, cx);
        self._overlay_subscriptions =
            vec![
                cx.subscribe_in(view, window, |this, _, _: &DismissEvent, window, cx| {
                    this.close_overlay(window, cx)
                }),
            ];
        view.focus_handle(cx).focus(window, cx);
    }

    fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.about = None;
        self.confirm_close = None;
        self.confirm_paste = None;
        self._overlay_subscriptions.clear();
        if let Some(pane) = self.pane(self.focused)
            && let Some(tab) = pane.tabs.get(pane.active)
        {
            tab.view.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette.is_some() {
            self.close_overlay(window, cx);
            return;
        }
        if !Settings::get(cx).command_palette.enable {
            return;
        }
        let commands = Commands::get(cx).commands.clone();
        // recency is ignored when disabled, so the palette only sees recent names when it is on
        let recent = if Settings::get(cx).command_palette.show_recent {
            self.recent_commands.clone()
        } else {
            Vec::new()
        };
        let palette = cx.new(|cx| CommandPalette::new(commands, recent, cx));
        self.open_overlay(&palette, window, cx);
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            |this, _, command: &Command, window, cx| {
                this.close_overlay(window, cx);
                this.record_recent(command.label());
                this.run_command(command, window, cx);
            },
        ));
        self.palette = Some(palette);
    }

    /// palette listing the open tabs, picking one switches to it
    fn open_tab_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.focused;
        let tabs = (0..self.focused_pane().tabs.len())
            .map(|ix| Command {
                name: self.tab_title(pane, ix, cx),
                category: Some((ix + 1).to_string()),
                pinned: false,
                actions: vec![CommandAction::ActivateTab(ix + 1)],
            })
            .collect();
        let palette = cx.new(|cx| CommandPalette::tab_picker(tabs, cx));
        self.open_overlay(&palette, window, cx);
        // tabs change all the time, so picks are not remembered as recent commands
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            |this, _, command: &Command, window, cx| {
                this.close_overlay(window, cx);
                this.run_command(command, window, cx);
            },
        ));
        self.palette = Some(palette);
    }

    /// remember a command run from the palette, most recent first, oldest dropped
    fn record_recent(&mut self, label: String) {
        self.recent_commands.retain(|recent| *recent != label);
        self.recent_commands.insert(0, label);
        self.recent_commands.truncate(RECENT_COMMANDS_MAX);
    }

    fn refresh_titles(&self) {
        self.refresh_titles.unbounded_send(true).ok();
    }

    fn refresh_titles_later(&self) {
        self.refresh_titles.unbounded_send(false).ok();
    }

    fn title_inputs(&self, cx: &App) -> TitleSnapshot {
        let settings = Settings::get(cx);
        let mut inputs = Vec::new();
        let mut active = 0;
        for pane in self.layout.panes() {
            for (ix, tab) in pane.tabs.iter().enumerate() {
                if pane.id == self.focused && ix == pane.active {
                    active = inputs.len();
                }
                let terminal = tab.view.read(cx).terminal().read(cx);
                inputs.push((
                    tab.view.entity_id(),
                    TitleInputs {
                        number: ix + 1,
                        shell_pid: terminal.shell_pid,
                        title: terminal.title(&settings.default_title),
                        profile_icon: tab.profile_icon.clone(),
                    },
                ));
            }
        }
        (
            settings.tab_title.clone(),
            settings.window_title.clone(),
            settings.tab_icon.clone(),
            TabIcons::get(cx).clone(),
            active,
            inputs,
        )
    }

    fn set_titles(
        &mut self,
        titles: Vec<(EntityId, String, String)>,
        window_title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let window_title = if window_title.is_empty() {
            Settings::get(cx).default_title.clone()
        } else {
            window_title
        };
        // linux can't read the title back, so the last one is kept here
        if self.window_title != window_title {
            window.set_window_title(&window_title);
            self.window_title = window_title;
        }
        let mut changed = false;
        for (id, title, icon) in titles {
            // tabs closed while building are skipped
            let Some((pane_id, ix)) = self.find_id(id) else {
                continue;
            };
            let tab = &mut self
                .pane_mut(pane_id)
                .expect("pane id comes from the layout")
                .tabs[ix];
            if tab.title != title || tab.icon != icon {
                tab.title = title;
                tab.icon = icon;
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.add_tab(window, cx);
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.focused_pane().active;
        let pane = self.focused;
        self.request_close_tab(pane, ix, window, cx);
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        // wraps from the last tab back to the first
        let count = self.focused_pane().tabs.len();
        let ix = (self.focused_pane().active + 1) % count;
        let pane = self.focused;
        self.activate_tab(pane, ix, window, cx);
    }

    fn activate_tab_action(
        &mut self,
        action: &ActivateTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.focused_pane().tabs.len();
        let pane = self.focused;
        if action.0 < count {
            self.activate_tab(pane, action.0, window, cx);
        }
    }

    fn toggle_fullscreen(
        &mut self,
        _: &ToggleFullscreen,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.toggle_fullscreen();
    }

    fn open_about(&mut self, _: &OpenAbout, window: &mut Window, cx: &mut Context<Self>) {
        let about = cx.new(About::new);
        self.open_overlay(&about, window, cx);
        self.about = Some(about);
    }

    fn quit_action(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
        self.request_quit(window, cx);
    }

    fn minimize(&mut self, _: &Minimize, window: &mut Window, _: &mut Context<Self>) {
        window.minimize_window();
    }

    fn zoom(&mut self, _: &Zoom, window: &mut Window, _: &mut Context<Self>) {
        window.zoom_window();
    }

    fn run_action(&mut self, action: &RunAction, window: &mut Window, cx: &mut Context<Self>) {
        let command = Command {
            name: String::new(),
            category: None,
            pinned: false,
            actions: vec![action.0.clone()],
        };
        self.run_command(&command, window, cx);
    }

    // none step goes back to the size from settings
    fn change_font_size(&self, pane: usize, ix: usize, step: Option<f32>, cx: &mut Context<Self>) {
        self.view_at(pane, ix).update(cx, |view, cx| {
            let size = step.map(|step| view.font_size(cx) + step);
            view.set_font_size(size, cx);
        });
    }

    fn open_settings(&mut self, cx: &mut Context<Self>) {
        let Some(path) = Settings::path() else {
            self.show_notification("no config folder to open settings from", None, cx);
            return;
        };
        // macos has no app set for .jsonc files, so it is opened as plain text
        if cfg!(target_os = "macos") {
            cx.background_spawn(async move {
                std::process::Command::new("open")
                    .arg("-t")
                    .arg(path)
                    .status()
                    .ok();
            })
            .detach();
        } else {
            cx.open_with_system(&path);
        }
    }
}

/// run `f` on the workspace, bringing its window to the front first. app wide actions use
/// it, since with the window minimized or the dock menu in use no window is active
pub fn with_workspace(
    cx: &mut App,
    f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>),
) {
    // the window in front, else any
    let Some(handle) = cx
        .active_window()
        .and_then(|window| window.downcast::<Workspace>())
        .or_else(|| {
            cx.windows()
                .into_iter()
                .find_map(|window| window.downcast::<Workspace>())
        })
    else {
        return;
    };
    cx.activate(true);
    handle
        .update(cx, |workspace, window, cx| {
            window.activate_window();
            f(workspace, window, cx);
        })
        .ok();
}

/// folder the shell of `tab` is in, the one it reported first, none when it can't be told
fn tab_cwd(tab: &Tab, cx: &App) -> Option<PathBuf> {
    let terminal = tab.view.read(cx).terminal().read(cx);
    if let Some(cwd) = terminal.reported_cwd() {
        return Some(cwd.to_path_buf());
    }
    crate::terminal::process_info(terminal.shell_pid)?.cwd
}

/// program names joined for a dialog, cut short after a few
fn name_list(names: &[String]) -> String {
    let mut list = names[..names.len().min(QUIT_DIALOG_MAX_NAMES)].join(", ");
    if names.len() > QUIT_DIALOG_MAX_NAMES {
        list.push_str(", …");
    }
    list
}

/// dialog text naming the programs still running in tabs about to close
fn side_close_message(names: &[String]) -> String {
    let list = name_list(names);
    match names.len() {
        1 => format!("a tab to close is still running a program: {list}"),
        count => format!("{count} tabs to close are still running programs: {list}"),
    }
}

/// quit dialog text naming the programs still running, the list is cut after a few names
fn quit_message(names: &[String]) -> String {
    let list = name_list(names);
    match names.len() {
        0 => "quit kuterm?".to_owned(),
        1 => format!("1 tab is still running a program: {list}"),
        count => format!("{count} tabs are still running programs: {list}"),
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Instant};

    use alacritty_terminal::term::TermMode;
    use gpui::{
        Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
        TestAppContext, VisualTestContext, point,
    };

    use super::*;
    use crate::{
        settings::{Keybindings, Pins},
        terminal::{Terminal, TerminalBounds},
    };

    /// open a workspace with `tabs` tabs and user keybindings `keys`, the last tab is active
    fn open_with<'a>(
        cx: &'a mut TestAppContext,
        tabs: usize,
        keys: &str,
    ) -> (Entity<Workspace>, &'a mut VisualTestContext) {
        open_with_settings(cx, tabs, keys, Settings::default())
    }

    fn open_with_settings<'a>(
        cx: &'a mut TestAppContext,
        tabs: usize,
        keys: &str,
        settings: Settings,
    ) -> (Entity<Workspace>, &'a mut VisualTestContext) {
        let keybindings = Keybindings::parse(keys).unwrap();
        // shells wake gpui tasks from alacritty's pty thread, which the test scheduler
        // only tolerates with parking allowed
        cx.executor().allow_parking();
        cx.update(|cx| {
            cx.set_global(settings);
            // bundled theme, so no theme files are created
            cx.set_global(Theme::default());
            cx.set_global(TabIcons::default());
            cx.set_global(Commands::default());
            cx.set_global(Pins::default());
            cx.bind_keys(keybindings.bindings());
            cx.bind_keys(crate::ui::text_input::bindings());
        });
        let (ws, cx) = cx.add_window_view(Workspace::new);
        cx.run_until_parked();
        for _ in 1..tabs {
            ws.update_in(cx, |ws, window, cx| ws.add_tab(window, cx));
            cx.run_until_parked();
        }
        assert_eq!(
            ws.update(cx, |ws, _| ws.focused_pane().tabs.len()),
            tabs,
            "failed to spawn shells"
        );
        (ws, cx)
    }

    fn open(cx: &mut TestAppContext, tabs: usize) -> (Entity<Workspace>, &mut VisualTestContext) {
        open_with(cx, tabs, "{}")
    }

    fn views(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<EntityId> {
        ws.update(cx, |ws, _| {
            ws.focused_pane()
                .tabs
                .iter()
                .map(|t| t.view.entity_id())
                .collect()
        })
    }

    /// active tab index, checking that focus is on it
    fn current(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> usize {
        ws.update_in(cx, |ws, window, cx| {
            let state = ws.focused_pane();
            let focused: Vec<usize> = state
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, tab)| tab.view.focus_handle(cx).is_focused(window))
                .map(|(ix, _)| ix)
                .collect();
            assert_eq!(
                focused,
                vec![state.active],
                "focus does not follow the active tab"
            );
            state.active
        })
    }

    fn next(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> usize {
        ws.update_in(cx, |ws, window, cx| ws.next_tab(&NextTab, window, cx));
        cx.run_until_parked();
        current(ws, cx)
    }

    fn activate(ws: &Entity<Workspace>, cx: &mut VisualTestContext, ix: usize) -> usize {
        ws.update_in(cx, |ws, window, cx| {
            ws.activate_tab_action(&ActivateTab(ix), window, cx)
        });
        cx.run_until_parked();
        current(ws, cx)
    }

    fn keys(ws: &Entity<Workspace>, cx: &mut VisualTestContext, keystrokes: &str) -> usize {
        cx.run_until_parked();
        cx.simulate_keystrokes(keystrokes);
        cx.run_until_parked();
        current(ws, cx)
    }

    #[gpui::test]
    fn next_tab_wraps_from_last_to_first(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        assert_eq!(current(&ws, cx), 2);
        assert_eq!(next(&ws, cx), 0);
    }

    #[gpui::test]
    fn activate_tab_out_of_range_is_ignored(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        activate(&ws, cx, 1);
        let before = views(&ws, cx);
        for ix in [3, 4, 8, 9, 99, usize::MAX - 1, usize::MAX] {
            assert_eq!(activate(&ws, cx, ix), 1, "index {ix}");
        }
        assert_eq!(views(&ws, cx), before);
    }

    #[gpui::test]
    fn activate_tab_after_close_uses_shifted_indexes(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 4);
        let old = views(&ws, cx);
        ws.update_in(cx, |ws, window, cx| ws.close_tab_at(0, 1, window, cx));
        cx.run_until_parked();
        // index 1 is now the old third tab, index 3 is gone
        assert_eq!(activate(&ws, cx, 1), 1);
        assert_eq!(views(&ws, cx)[1], old[2]);
        assert_eq!(activate(&ws, cx, 3), 1);
        assert_eq!(activate(&ws, cx, 2), 2);
    }

    #[gpui::test]
    fn move_tab_reorders_and_keeps_the_active_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 4);
        let old = views(&ws, cx);
        // the last tab is active, moving another tab before it keeps that one active
        ws.update(cx, |ws, cx| ws.move_tab(0, 0, 2, cx));
        cx.run_until_parked();
        assert_eq!(views(&ws, cx), vec![old[1], old[2], old[0], old[3]]);
        assert_eq!(current(&ws, cx), 3);
        // moving the active tab makes it follow its new position
        activate(&ws, cx, 2);
        ws.update(cx, |ws, cx| ws.move_tab(0, 2, 0, cx));
        cx.run_until_parked();
        assert_eq!(views(&ws, cx), vec![old[0], old[1], old[2], old[3]]);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn move_tab_ignores_out_of_range_indexes(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        let before = views(&ws, cx);
        ws.update(cx, |ws, cx| ws.move_tab(0, 0, 0, cx));
        ws.update(cx, |ws, cx| ws.move_tab(0, 3, 1, cx));
        ws.update(cx, |ws, cx| ws.move_tab(0, 1, 3, cx));
        cx.run_until_parked();
        assert_eq!(views(&ws, cx), before);
    }

    /// press on one tab, move to another and release, like a mouse drag reorder
    fn drag_tab(cx: &mut VisualTestContext, from: &'static str, to: &'static str) {
        let start = cx.debug_bounds(from).unwrap().center();
        let end = cx.debug_bounds(to).unwrap().center();
        drag_to(cx, start, end);
    }

    #[gpui::test]
    fn dragging_a_tab_reorders_it(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        cx.run_until_parked();
        let old = views(&ws, cx);
        drag_tab(cx, "tab-title-0", "tab-title-2");
        assert_eq!(views(&ws, cx), vec![old[1], old[2], old[0]]);
    }

    #[gpui::test]
    fn disabled_drag_tabs_ignores_the_mouse(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"drag_tabs": false}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 3, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        cx.run_until_parked();
        let before = views(&ws, cx);
        drag_tab(cx, "tab-title-0", "tab-title-2");
        assert_eq!(views(&ws, cx), before);
    }

    /// press at `start`, cross the drag threshold, move to `end` and release
    fn drag_to(cx: &mut VisualTestContext, start: Point<Pixels>, end: Point<Pixels>) {
        cx.simulate_event(MouseDownEvent {
            position: start,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        // the first move starts the drag, the second feeds the drag move listeners
        for _ in 0..2 {
            cx.simulate_event(MouseMoveEvent {
                position: end,
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            });
            cx.run_until_parked();
        }
        cx.simulate_event(MouseUpEvent {
            position: end,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 1,
        });
        cx.run_until_parked();
    }

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

    /// split the focused pane at `side`, moving its first tab into the new pane
    fn split_pane(ws: &Entity<Workspace>, cx: &mut VisualTestContext, side: DropSide) -> usize {
        ws.update_in(cx, |ws, window, cx| {
            let target = ws.focused;
            ws.split_with_tab(target, 0, target, side, window, cx);
        });
        cx.run_until_parked();
        ws.update(cx, |ws, _| ws.focused)
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
    fn disabled_next_tab_ignores_ctrl_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open_with(cx, 3, r#"{"next_tab": null}"#);
        assert_eq!(keys(&ws, cx, "ctrl-tab"), 2);
        assert_eq!(keys(&ws, cx, "alt-1"), 0);
    }

    // raw input bytes a focus reader collects before it ends
    const READ: usize = 30;

    /// let the pty threads run, then move the fake clock so batched alacritty events flow
    fn pump(cx: &mut VisualTestContext) {
        std::thread::sleep(Duration::from_millis(15));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
    }

    fn wait_until(
        cx: &mut VisualTestContext,
        what: &str,
        mut done: impl FnMut(&mut VisualTestContext) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done(cx) {
            assert!(Instant::now() < deadline, "timed out: {what}");
            pump(cx);
        }
    }

    // gpui test windows start inactive, activating is like a user click
    fn set_window_active(cx: &mut VisualTestContext, active: bool) {
        if active {
            cx.update(|window, _| window.activate_window());
        } else {
            cx.deactivate_window();
        }
        cx.run_until_parked();
        pump(cx);
    }

    fn bells(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<bool> {
        ws.update(cx, |ws, _| {
            ws.focused_pane().tabs.iter().map(|t| t.bell).collect()
        })
    }

    #[gpui::test]
    fn bell_marks_background_tabs_until_shown(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        // the last tab is active, so the bell of tab 0 rings in the background
        terminal(&ws, cx, 0).update(cx, |_, cx| cx.emit(Event::Bell));
        terminal(&ws, cx, 2).update(cx, |_, cx| cx.emit(Event::Bell));
        cx.run_until_parked();
        assert_eq!(bells(&ws, cx), vec![true, false, false]);
        activate(&ws, cx, 0);
        assert_eq!(bells(&ws, cx), vec![false, false, false]);
    }

    #[gpui::test]
    fn bell_does_not_mark_tabs_without_tab_indicator(cx: &mut TestAppContext) {
        let mut settings = Settings::default();
        settings.bell.tab_indicator = false;
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        terminal(&ws, cx, 0).update(cx, |_, cx| cx.emit(Event::Bell));
        cx.run_until_parked();
        assert_eq!(bells(&ws, cx), vec![false, false]);
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

    fn terminal(ws: &Entity<Workspace>, cx: &mut VisualTestContext, ix: usize) -> Entity<Terminal> {
        ws.update(cx, |ws, cx| {
            ws.focused_pane().tabs[ix].view.read(cx).terminal().clone()
        })
    }

    /// in the active tab: optionally enable focus reporting (mode 1004), then save READ raw
    /// input bytes as hex into the returned file
    fn reader(
        ws: &Entity<Workspace>,
        cx: &mut VisualTestContext,
        focus_mode: bool,
        name: &str,
    ) -> (usize, PathBuf) {
        let mode = if focus_mode {
            "printf '\\033[?1004h'; "
        } else {
            ""
        };
        reader_with(ws, cx, mode, name, |mode| {
            mode.contains(TermMode::FOCUS_IN_OUT) == focus_mode
        })
    }

    /// like `reader`, running `mode` first and waiting until the view sees `ready` modes
    fn reader_with(
        ws: &Entity<Workspace>,
        cx: &mut VisualTestContext,
        mode: &str,
        name: &str,
        ready_mode: impl Fn(TermMode) -> bool,
    ) -> (usize, PathBuf) {
        let file = std::env::temp_dir().join(format!("kuterm_focus_{name}_{}", std::process::id()));
        let ready = file.with_extension("ready");
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(&ready);
        let (f, r) = (file.display(), ready.display());
        let command = format!(
            "{mode}stty raw -echo; touch {r}; dd bs=1 count={READ} 2>/dev/null | od -An -v -tx1 | tr -d ' \\n' > {f}.tmp; mv {f}.tmp {f}; stty sane\r"
        );
        let ix = current(ws, cx);
        let terminal = terminal(ws, cx, ix);
        terminal.update(cx, |t, _| t.input(command.into_bytes()));
        wait_until(cx, "reader never started", |_| ready.exists());
        wait_until(cx, "mode never reached the view", |cx| {
            terminal.read_with(cx, |t, _| ready_mode(t.last_content.mode))
        });
        // let dd start reading
        for _ in 0..5 {
            pump(cx);
        }
        std::fs::remove_file(&ready).ok();
        (ix, file)
    }

    fn send_to(ws: &Entity<Workspace>, cx: &mut VisualTestContext, ix: usize, text: &str) {
        let bytes = text.as_bytes().to_vec();
        terminal(ws, cx, ix).update(cx, |t, _| t.input(bytes));
        pump(cx);
    }

    /// fill the reader up and return what it got before: "I", "O" or other bytes as hex, with
    /// repeated identical reports merged
    fn finish(
        ws: &Entity<Workspace>,
        cx: &mut VisualTestContext,
        (ix, file): (usize, PathBuf),
    ) -> Vec<String> {
        send_to(ws, cx, ix, &"x".repeat(READ));
        wait_until(cx, "no reader output", |_| file.exists());
        let hex = std::fs::read_to_string(&file).unwrap();
        std::fs::remove_file(&file).ok();
        let mut tokens: Vec<String> = Vec::new();
        let mut rest = hex.as_str();
        while !rest.is_empty() {
            if let Some(r) = rest.strip_prefix("1b5b49") {
                tokens.push("I".into());
                rest = r;
            } else if let Some(r) = rest.strip_prefix("1b5b4f") {
                tokens.push("O".into());
                rest = r;
            } else {
                tokens.push(rest[..2].into());
                rest = &rest[2..];
            }
        }
        while tokens.last().is_some_and(|t| t == "78") {
            tokens.pop();
        }
        // gpui's focus listeners may fire twice for one activation change
        tokens.dedup_by(|a, b| a == b && (a == "I" || a == "O"));
        tokens
    }

    #[gpui::test]
    fn focus_mode_reports_out_on_deactivate_and_in_on_activate(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_window_active(cx, true);
        let r = reader(&ws, cx, true, "report");
        set_window_active(cx, false);
        set_window_active(cx, true);
        assert_eq!(finish(&ws, cx, r), ["O", "I"]);
    }

    #[gpui::test]
    fn no_reports_without_focus_mode(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_window_active(cx, true);
        let r = reader(&ws, cx, false, "off");
        for _ in 0..3 {
            set_window_active(cx, false);
            set_window_active(cx, true);
        }
        send_to(&ws, cx, 0, "abc");
        assert_eq!(finish(&ws, cx, r), ["61", "62", "63"]);
    }

    #[gpui::test]
    fn only_the_active_tab_reports_window_activation(cx: &mut TestAppContext) {
        for active in 0..3 {
            let (ws, cx) = open(cx, 3);
            set_window_active(cx, true);
            let mut readers = Vec::new();
            for ix in 0..3 {
                activate(&ws, cx, ix);
                readers.push(reader(&ws, cx, true, &format!("tab{active}_{ix}")));
            }
            activate(&ws, cx, active);
            set_window_active(cx, false);
            set_window_active(cx, true);
            // tabs 0 and 1 lost focus to the next tab during setup, picking `active` moves focus
            // from tab 2, then only `active` sees the window
            let mut want: Vec<Vec<&str>> = vec![vec!["O"], vec!["O"], vec![]];
            if active != 2 {
                want[2].push("O");
                want[active].push("I");
            }
            want[active].extend(["O", "I"]);
            for (ix, r) in readers.into_iter().enumerate() {
                assert_eq!(
                    finish(&ws, cx, r),
                    want[ix],
                    "tab {ix} with tab {active} active"
                );
            }
        }
    }

    #[gpui::test]
    fn plain_ctrl_w_and_ctrl_t_reach_the_shell(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let r = reader(&ws, cx, false, "ctrl_keys");
        cx.simulate_keystrokes("ctrl-w ctrl-t");
        pump(cx);
        assert_eq!(views(&ws, cx).len(), 2, "a shell key changed the tabs");
        assert_eq!(finish(&ws, cx, r), ["17", "14"]);
    }

    #[gpui::test]
    fn ctrl_shift_t_and_ctrl_shift_w_manage_tabs(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        keys(&ws, cx, "ctrl-shift-t");
        assert_eq!(views(&ws, cx).len(), 2);
        keys(&ws, cx, "ctrl-shift-w");
        assert_eq!(views(&ws, cx).len(), 1);
    }

    /// screen line that starts with `text`
    fn line_starting_with(terminal: &Terminal, text: &str) -> Option<usize> {
        let mut lines: Vec<String> = Vec::new();
        for indexed in &terminal.last_content.cells {
            if indexed.point.column.0 == 0 {
                lines.push(String::new());
            }
            lines.last_mut()?.push(indexed.cell.c);
        }
        lines.iter().position(|line| line.starts_with(text))
    }

    /// window position inside `column` of the screen line that starts with `text`, in the
    /// cell's left or right half
    fn cell_of(
        ws: &Entity<Workspace>,
        cx: &mut VisualTestContext,
        text: &str,
        column: usize,
        right: bool,
    ) -> Point<Pixels> {
        let x = column as f32 + if right { 0.8 } else { 0.2 };
        let ix = current(ws, cx);
        terminal(ws, cx, ix).read_with(cx, |t, _| {
            let line = line_starting_with(t, text)
                .unwrap_or_else(|| panic!("no line starts with {text:?}"));
            let b = t.last_content.terminal_bounds;
            point(
                b.bounds.origin.x + b.cell_width * x,
                b.bounds.origin.y + b.line_height * (line as f32 + 0.5),
            )
        })
    }

    fn print_line(ws: &Entity<Workspace>, cx: &mut VisualTestContext, text: &str) {
        let ix = current(ws, cx);
        // split with %s so only the output, not the typed command, starts with `text`
        let (head, tail) = text.split_at(2);
        send_to(
            ws,
            cx,
            ix,
            &format!("clear; printf '{head}%s\\n' '{tail}'\r"),
        );
        let terminal = terminal(ws, cx, ix);
        wait_until(cx, "text never printed", |cx| {
            terminal.read_with(cx, |t, _| line_starting_with(t, text).is_some())
        });
    }

    fn click(cx: &mut VisualTestContext, position: Point<Pixels>, click_count: usize) {
        cx.simulate_event(MouseDownEvent {
            position,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count,
            first_mouse: false,
        });
    }

    fn release(cx: &mut VisualTestContext, position: Point<Pixels>) {
        cx.simulate_event(MouseUpEvent {
            position,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 1,
        });
    }

    fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
        cx.read_from_clipboard().and_then(|item| item.text())
    }

    #[gpui::test]
    fn mouse_drag_selects_and_ctrl_shift_c_copies(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        print_line(&ws, cx, "pick these words");

        let start = cell_of(&ws, cx, "pick these words", 5, false);
        let end = cell_of(&ws, cx, "pick these words", 9, true);
        click(cx, start, 1);
        cx.simulate_event(MouseMoveEvent {
            position: end,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        });
        release(cx, end);
        cx.simulate_keystrokes("ctrl-shift-c");
        assert_eq!(clipboard(cx).as_deref(), Some("these"));

        // a move without the button held does not change the selection
        let hover = cell_of(&ws, cx, "pick these words", 14, true);
        cx.simulate_event(MouseMoveEvent {
            position: hover,
            pressed_button: None,
            modifiers: Modifiers::default(),
        });
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
        cx.simulate_keystrokes("ctrl-shift-c");
        assert_eq!(clipboard(cx).as_deref(), Some("these"));
    }

    #[gpui::test]
    fn double_click_copies_a_word(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        print_line(&ws, cx, "pick these words");
        let position = cell_of(&ws, cx, "pick these words", 12, false);
        click(cx, position, 1);
        release(cx, position);
        click(cx, position, 2);
        release(cx, position);
        cx.simulate_keystrokes("ctrl-shift-c");
        assert_eq!(clipboard(cx).as_deref(), Some("words"));
    }

    #[gpui::test]
    fn copy_without_selection_keeps_clipboard(cx: &mut TestAppContext) {
        let (_ws, cx) = open(cx, 1);
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into()));
        cx.simulate_keystrokes("ctrl-shift-c");
        assert_eq!(clipboard(cx).as_deref(), Some("before"));
    }

    /// raw reader in the active tab of a program that asked for sgr mouse reports
    fn mouse_reader(
        ws: &Entity<Workspace>,
        cx: &mut VisualTestContext,
        name: &str,
    ) -> (usize, PathBuf) {
        reader_with(
            ws,
            cx,
            "printf '\\033[?1000h\\033[?1006h'; ",
            name,
            |mode| mode.contains(TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE),
        )
    }

    /// window position in the middle of a visible cell of the active tab
    fn cell_center(
        ws: &Entity<Workspace>,
        cx: &mut VisualTestContext,
        column: usize,
        line: usize,
    ) -> Point<Pixels> {
        let ix = current(ws, cx);
        terminal(ws, cx, ix).read_with(cx, |t, _| {
            let b = t.last_content.terminal_bounds;
            point(
                b.bounds.origin.x + b.cell_width * (column as f32 + 0.5),
                b.bounds.origin.y + b.line_height * (line as f32 + 0.5),
            )
        })
    }

    fn hex(bytes: &str) -> Vec<String> {
        bytes.bytes().map(|b| format!("{b:02x}")).collect()
    }

    #[gpui::test]
    fn clicks_go_to_a_program_that_asked_for_the_mouse(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let r = mouse_reader(&ws, cx, "mouse_click");
        let at = cell_center(&ws, cx, 4, 2);
        click(cx, at, 1);
        release(cx, at);
        pump(cx);
        let selected = terminal(&ws, cx, 0).read_with(cx, |t, _| t.selection_text());
        assert_eq!(selected, None, "the click also selected");
        assert_eq!(finish(&ws, cx, r), hex("\x1b[<0;5;3M\x1b[<0;5;3m"));
    }

    #[gpui::test]
    fn wheel_goes_to_a_program_that_asked_for_the_mouse(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let r = mouse_reader(&ws, cx, "mouse_wheel");
        let position = cell_center(&ws, cx, 0, 0);
        cx.simulate_event(gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        pump(cx);
        let got = finish(&ws, cx, r);
        let one = hex("\x1b[<64;1;1M");
        assert!(
            !got.is_empty() && got.chunks(one.len()).all(|report| report == one),
            "{got:?}"
        );
    }

    #[gpui::test]
    fn shift_drag_selects_even_when_a_program_asked(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let r = mouse_reader(&ws, cx, "mouse_shift");
        let (start, end) = (cell_center(&ws, cx, 0, 0), cell_center(&ws, cx, 5, 0));
        cx.simulate_event(MouseDownEvent {
            position: start,
            modifiers: Modifiers::shift(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseMoveEvent {
            position: end,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::shift(),
        });
        cx.simulate_event(MouseUpEvent {
            position: end,
            modifiers: Modifiers::shift(),
            button: MouseButton::Left,
            click_count: 1,
        });
        pump(cx);
        let selected = terminal(&ws, cx, 0).read_with(cx, |t, _| t.selection_text());
        assert!(selected.is_some(), "shift drag selected nothing");
        assert_eq!(finish(&ws, cx, r), Vec::<String>::new());
    }

    #[gpui::test]
    fn clicks_are_not_reported_without_mouse_mode(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let r = reader(&ws, cx, false, "mouse_off");
        let at = cell_center(&ws, cx, 4, 2);
        for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
            cx.simulate_event(MouseDownEvent {
                position: at,
                modifiers: Modifiers::default(),
                button,
                click_count: 1,
                first_mouse: false,
            });
            cx.simulate_event(MouseUpEvent {
                position: at,
                modifiers: Modifiers::default(),
                button,
                click_count: 1,
            });
        }
        pump(cx);
        assert_eq!(finish(&ws, cx, r), Vec::<String>::new());
    }

    fn tab_icon_and_title(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> (String, String) {
        ws.update(cx, |ws, _| {
            let tab = &ws.focused_pane().tabs[0];
            (tab.icon.clone(), tab.title.clone())
        })
    }

    fn icon_while_sleeping(cx: &mut TestAppContext, json: &str) -> (String, String) {
        let (ws, cx) = open_with_settings(cx, 1, "{}", Settings::parse(json).unwrap());
        let icons = TabIcons::parse(r#"{"groups": [{"icon": "S", "commands": ["sleep"]}]}"#);
        cx.update(|_, cx| cx.set_global(icons.unwrap()));
        let before = tab_icon_and_title(&ws, cx).0;
        send_to(&ws, cx, 0, "sleep 30\r");
        wait_until(cx, "sleep never showed in the title", |cx| {
            tab_icon_and_title(&ws, cx).1.ends_with("sleep")
        });
        (before, tab_icon_and_title(&ws, cx).0)
    }

    #[gpui::test]
    fn tab_icon_follows_running_program(cx: &mut TestAppContext) {
        let (before, during) =
            icon_while_sleeping(cx, r#"{"tab_icon": {"dynamic": true, "default": "D"}}"#);
        assert_eq!(before, "D");
        assert_eq!(during, "S");
    }

    #[gpui::test]
    fn disabled_dynamic_icon_stays_default(cx: &mut TestAppContext) {
        let (before, during) =
            icon_while_sleeping(cx, r#"{"tab_icon": {"dynamic": false, "default": "D"}}"#);
        assert_eq!(before, "D");
        assert_eq!(during, "D");
    }

    #[gpui::test]
    fn profile_icon_never_changes(cx: &mut TestAppContext) {
        let (before, during) = icon_while_sleeping(
            cx,
            r#"{"tab_icon": {"dynamic": true, "default": "D"},
                "profiles": [{"name": "p", "command": "system", "icon": "P"}]}"#,
        );
        assert_eq!(before, "P");
        assert_eq!(during, "P");
    }

    /// workspace with the tab bar always shown and these profiles
    fn open_profiles<'a>(
        cx: &'a mut TestAppContext,
        profiles: &str,
    ) -> (Entity<Workspace>, &'a mut VisualTestContext) {
        let json = format!(r#"{{"hide_bar_for_one_tab": false, "profiles": [{profiles}]}}"#);
        let (ws, cx) = open_with_settings(cx, 1, "{}", Settings::parse(&json).unwrap());
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        cx.run_until_parked();
        (ws, cx)
    }

    fn right_click(cx: &mut VisualTestContext, position: Point<Pixels>) {
        cx.simulate_event(MouseDownEvent {
            position,
            modifiers: Modifiers::default(),
            button: MouseButton::Right,
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            position,
            modifiers: Modifiers::default(),
            button: MouseButton::Right,
            click_count: 1,
        });
        cx.run_until_parked();
    }

    fn menu_open(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
        ws.update(cx, |ws, _| ws.profile_menu.is_some())
    }

    #[gpui::test]
    fn middle_click_closes_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        cx.run_until_parked();
        let old = views(&ws, cx);
        let title = cx.debug_bounds("tab-title-1").unwrap().center();
        cx.simulate_event(MouseDownEvent {
            position: title,
            modifiers: Modifiers::default(),
            button: MouseButton::Middle,
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        assert_eq!(views(&ws, cx), vec![old[0], old[2]]);
    }

    #[gpui::test]
    fn single_profile_has_no_menu(cx: &mut TestAppContext) {
        let (ws, cx) = open_profiles(cx, r#"{"name": "only", "command": "system"}"#);
        assert!(cx.debug_bounds("profile-hint").is_none());
        let plus = cx.debug_bounds("new-tab").unwrap().center();
        right_click(cx, plus);
        assert!(!menu_open(&ws, cx));
        assert!(cx.debug_bounds("profile-0").is_none());
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[gpui::test]
    fn right_click_on_plus_opens_picked_profile(cx: &mut TestAppContext) {
        let (ws, cx) = open_profiles(
            cx,
            r#"{"name": "main", "default": true, "command": "system"},
               {"name": "other", "command": {"with_arguments": {
                   "program": "/bin/sh", "args": ["-c", "echo \"from_$PICKED\"; sleep 5"]}},
                "env": {"PICKED": "other"}}"#,
        );
        assert!(cx.debug_bounds("profile-hint").is_some());
        let plus = cx.debug_bounds("new-tab").unwrap().center();

        // a click outside closes the menu without a new tab
        right_click(cx, plus);
        assert!(menu_open(&ws, cx));
        assert!(cx.debug_bounds("profile-1").is_some());
        let outside = point(gpui::px(450.), gpui::px(400.));
        click(cx, outside, 1);
        release(cx, outside);
        cx.run_until_parked();
        assert!(!menu_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 1);

        right_click(cx, plus);
        let item = cx.debug_bounds("profile-1").unwrap().center();
        click(cx, item, 1);
        release(cx, item);
        cx.run_until_parked();
        assert!(!menu_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 1);
        let terminal = terminal(&ws, cx, 1);
        wait_until(cx, "picked profile never ran", |cx| {
            terminal.read_with(cx, |t, _| line_starting_with(t, "from_other").is_some())
        });

        // a left click still opens the default profile, "+" moved after the new tab
        let plus = cx.debug_bounds("new-tab").unwrap().center();
        click(cx, plus, 1);
        release(cx, plus);
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 3);
    }

    #[gpui::test]
    fn output_refreshes_titles_before_the_poll(cx: &mut TestAppContext) {
        let file = std::env::temp_dir().join(format!("kuterm_output_{}", std::process::id()));
        std::fs::write(&file, "before").unwrap();
        let json = format!(r#"{{"tab_title": [{{"exec": "cat {}"}}]}}"#, file.display());
        let (ws, cx) = open_with_settings(cx, 1, "{}", Settings::parse(&json).unwrap());
        let title = |cx: &mut VisualTestContext| tab_icon_and_title(&ws, cx).1;
        // let requests queued while opening run out first
        cx.executor().advance_clock(TITLE_REFRESH_MIN_GAP);
        cx.run_until_parked();
        assert_eq!(title(cx), "before");
        std::fs::write(&file, "after").unwrap();
        terminal(&ws, cx, 0).update(cx, |_, cx| cx.emit(Event::Wakeup));
        // well before the next poll, only the output could have refreshed it
        cx.executor().advance_clock(TITLE_REFRESH_MIN_GAP);
        cx.run_until_parked();
        assert!(TITLE_REFRESH_MIN_GAP * 2 < TITLE_REFRESH_INTERVAL);
        assert_eq!(title(cx), "after");
        std::fs::remove_file(&file).ok();
    }

    #[gpui::test]
    fn output_scrolling_into_history_shows_scrollbar(cx: &mut TestAppContext) {
        let json = r#"{"terminal": {"scrollbar": {"auto_hide": 3600}}}"#;
        let (ws, cx) = open_with_settings(cx, 1, "{}", Settings::parse(json).unwrap());
        let visible = |cx: &mut VisualTestContext| {
            ws.update(cx, |ws, cx| {
                ws.focused_pane().tabs[0]
                    .view
                    .read(cx)
                    .scrollbar_visible(cx)
            })
        };
        pump(cx);
        assert!(!visible(cx), "shown before any scrolling");
        send_to(&ws, cx, 0, "seq 1 300\r");
        let terminal = terminal(&ws, cx, 0);
        wait_until(cx, "output never reached history", |cx| {
            terminal.read_with(cx, |t, _| t.history_size() > 0)
        });

        wait_until(
            cx,
            "output scrolled the view but the bar stayed hidden",
            visible,
        );
    }

    fn scroll_offset(terminal: &Entity<Terminal>, cx: &mut VisualTestContext) -> usize {
        terminal.read_with(cx, |t, _| t.last_content.display_offset)
    }

    // click on the scrollbar with the given smooth scroll settings, then draw one frame
    fn click_scrollbar<'a>(
        json: &str,
        cx: &'a mut TestAppContext,
    ) -> (usize, Entity<Terminal>, &'a mut VisualTestContext) {
        let (ws, cx) = open_with_settings(cx, 1, "{}", Settings::parse(json).unwrap());
        send_to(&ws, cx, 0, "seq 1 300\r");
        let terminal = terminal(&ws, cx, 0);
        wait_until(cx, "output never reached history", |cx| {
            terminal.read_with(cx, |t, _| t.history_size() > 100)
        });
        let view = ws.update(cx, |ws, _| ws.focused_pane().tabs[0].view.clone());
        view.update(cx, |view, cx| view.scrollbar_down(100, cx));
        cx.run_until_parked();
        (100, terminal, cx)
    }

    #[gpui::test]
    fn smooth_scroll_glides_to_target(cx: &mut TestAppContext) {
        let json = r#"{"terminal": {"smooth_scroll": {"enable": true, "duration": 200, "easing": "linear"}}}"#;
        let (target, terminal, cx) = click_scrollbar(json, cx);
        let first = scroll_offset(&terminal, cx);
        assert!(first < target, "jumped straight to {first}");
        let mut last = first;
        wait_until(cx, "glide never reached the target", |cx| {
            cx.update(|window, cx| window.simulate_next_frame(cx));
            cx.run_until_parked();
            let offset = scroll_offset(&terminal, cx);
            assert!(offset >= last, "glide went back from {last} to {offset}");
            last = offset;
            offset == target
        });
    }

    #[gpui::test]
    fn disabled_smooth_scroll_jumps(cx: &mut TestAppContext) {
        let json = r#"{"terminal": {"smooth_scroll": {"enable": false}}}"#;
        let (target, terminal, cx) = click_scrollbar(json, cx);
        assert_eq!(scroll_offset(&terminal, cx), target);
    }

    fn palette_open(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
        ws.update(cx, |ws, _| ws.palette.is_some())
    }

    fn palette_focused(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
        ws.update_in(cx, |ws, window, cx| {
            ws.palette
                .as_ref()
                .is_some_and(|palette| palette.focus_handle(cx).is_focused(window))
        })
    }

    fn palette_names(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<String> {
        ws.update(cx, |ws, cx| {
            let palette = ws.palette.as_ref().unwrap().read(cx);
            palette.matches().map(|command| command.label()).collect()
        })
    }

    fn palette_query(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> String {
        ws.update(cx, |ws, cx| {
            ws.palette.as_ref().unwrap().read(cx).query(cx).to_string()
        })
    }

    fn type_keys(cx: &mut VisualTestContext, keystrokes: &str) {
        cx.run_until_parked();
        cx.simulate_keystrokes(keystrokes);
        cx.run_until_parked();
    }

    fn set_commands(cx: &mut VisualTestContext, json: &str) {
        let commands = Commands::parse(json).unwrap();
        cx.update(|_, cx| cx.set_global(commands));
    }

    /// run one command built from a json `actions` array against the workspace
    fn run_actions(ws: &Entity<Workspace>, cx: &mut VisualTestContext, actions: &str) {
        let json = format!(r#"{{"commands": [{{"name": "x", "actions": [{actions}]}}]}}"#);
        let command = Commands::parse(&json).unwrap().commands.remove(0);
        ws.update_in(cx, |ws, window, cx| ws.run_command(&command, window, cx));
        cx.run_until_parked();
    }

    #[gpui::test]
    fn ctrl_shift_p_toggles_palette_and_escape_closes_it(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        type_keys(cx, "ctrl-shift-p");
        assert!(palette_open(&ws, cx));
        assert!(palette_focused(&ws, cx));
        assert!(cx.debug_bounds("command-palette").is_some());
        assert_eq!(
            palette_names(&ws, cx).len(),
            Commands::default().commands.len()
        );

        type_keys(cx, "escape");
        assert!(!palette_open(&ws, cx));
        // focus went back to the active tab
        assert_eq!(current(&ws, cx), 1);

        type_keys(cx, "ctrl-shift-p");
        type_keys(cx, "ctrl-shift-p");
        assert!(!palette_open(&ws, cx));
        assert_eq!(current(&ws, cx), 1);
    }

    #[gpui::test]
    fn disabled_palette_never_opens(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"command_palette": {"enable": false}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        type_keys(cx, "ctrl-shift-p");
        assert!(!palette_open(&ws, cx));
        assert!(cx.debug_bounds("command-palette").is_none());
    }

    #[gpui::test]
    fn font_size_actions_change_only_the_active_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let settings_size = cx.update(|_, cx| Settings::get(cx).terminal.font.size);
        let sizes = |cx: &mut VisualTestContext| -> Vec<f32> {
            ws.update(cx, |ws, cx| {
                ws.focused_pane()
                    .tabs
                    .iter()
                    .map(|tab| tab.view.read(cx).font_size(cx))
                    .collect()
            })
        };

        run_actions(&ws, cx, r#""increase_font_size", "increase_font_size""#);
        assert_eq!(sizes(cx), vec![settings_size, settings_size + 2.]);
        run_actions(&ws, cx, r#""decrease_font_size""#);
        assert_eq!(sizes(cx), vec![settings_size, settings_size + 1.]);
        assert_eq!(
            cx.update(|_, cx| Settings::get(cx).terminal.font.size),
            settings_size
        );

        run_actions(&ws, cx, r#""reset_font_size""#);
        assert_eq!(sizes(cx), vec![settings_size, settings_size]);
    }

    #[gpui::test]
    fn typing_filters_and_arrows_pick_a_command(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [
                {"name": "first", "actions": ["new_tab"]},
                {"name": "second", "actions": ["new_tab", "new_tab"]},
                {"name": "sneaky", "actions": ["new_tab", "new_tab", "new_tab"]},
            ]}"#,
        );
        type_keys(cx, "ctrl-shift-p s");
        assert_eq!(palette_names(&ws, cx), ["first", "second", "sneaky"]);
        type_keys(cx, "e");
        assert_eq!(palette_names(&ws, cx), ["second", "sneaky"]);
        type_keys(cx, "x");
        assert!(palette_names(&ws, cx).is_empty());
        assert!(cx.debug_bounds("command-0").is_none());
        // enter with nothing matched keeps the palette open
        type_keys(cx, "enter");
        assert!(palette_open(&ws, cx));
        type_keys(cx, "backspace down up enter");
        assert!(!palette_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 3);
    }

    #[gpui::test]
    fn arrows_wrap_between_first_and_last_command(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [
                {"name": "first", "actions": ["new_tab"]},
                {"name": "second", "actions": ["new_tab", "new_tab"]},
            ]}"#,
        );
        // down from the last command wraps to the first one
        type_keys(cx, "ctrl-shift-p down down enter");
        assert!(!palette_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 2);
        // up from the first command wraps to the last one, "first" is listed first as recent
        type_keys(cx, "ctrl-shift-p up enter");
        assert!(!palette_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 4);
    }

    #[gpui::test]
    fn pick_tab_lists_tabs_and_switches_to_the_picked_one(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        run_actions(&ws, cx, r#""pick_tab""#);
        assert!(palette_open(&ws, cx));
        assert!(palette_focused(&ws, cx));
        let names = palette_names(&ws, cx);
        assert_eq!(names.len(), 3);
        for (ix, name) in names.iter().enumerate() {
            assert!(name.starts_with(&format!("{}: ", ix + 1)), "{name}");
        }
        // tabs have no pins to click
        assert!(cx.debug_bounds("pin-0").is_none());
        type_keys(cx, "down enter");
        assert!(!palette_open(&ws, cx));
        assert_eq!(current(&ws, cx), 1);
        // picked tabs are not remembered as recent commands
        assert!(ws.update(cx, |ws, _| ws.recent_commands.is_empty()));
    }

    #[gpui::test]
    fn close_button_follows_setting(cx: &mut TestAppContext) {
        for show in [true, false] {
            let json = format!(r#"{{"show_tab_close_button": {show}}}"#);
            let settings = Settings::parse(&json).unwrap();
            let (_, cx) = open_with_settings(cx, 2, "{}", settings);
            assert_eq!(cx.debug_bounds("close-tab-1").is_some(), show);
        }
    }

    #[gpui::test]
    fn palette_edits_the_query_like_a_text_input(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [{"name": "alpha beta", "actions": ["new_tab"]}]}"#,
        );
        type_keys(cx, "ctrl-shift-p a l p h a space b e t a");
        assert_eq!(palette_query(&ws, cx), "alpha beta");

        // alt+backspace deletes the word before the caret
        type_keys(cx, "alt-backspace");
        assert_eq!(palette_query(&ws, cx), "alpha ");

        // select all and copy puts the query on the clipboard
        type_keys(cx, "ctrl-a ctrl-c");
        assert_eq!(clipboard(cx).as_deref(), Some("alpha "));

        // paste replaces the selection
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("beta".into()));
        type_keys(cx, "ctrl-v");
        assert_eq!(palette_query(&ws, cx), "beta");
    }

    #[gpui::test]
    fn click_runs_a_command(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [
                {"name": "one", "actions": ["about"]},
                {"name": "two", "actions": ["new_tab"]},
            ]}"#,
        );
        type_keys(cx, "ctrl-shift-p");
        let item = cx.debug_bounds("command-1").unwrap().center();
        click(cx, item, 1);
        release(cx, item);
        cx.run_until_parked();
        assert!(!palette_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 2);
    }

    #[gpui::test]
    fn about_command_shows_page_until_a_key(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let r = reader(&ws, cx, false, "about");
        type_keys(cx, "ctrl-shift-p a b o u t enter");
        assert!(!palette_open(&ws, cx));
        assert!(ws.update(cx, |ws, _| ws.about.is_some()));
        assert!(cx.debug_bounds("about").is_some());
        type_keys(cx, "q");
        assert!(ws.update(cx, |ws, _| ws.about.is_none()));
        assert!(cx.debug_bounds("about").is_none());
        assert_eq!(current(&ws, cx), 0);
        // keys typed into the palette and the page never reach the shell
        assert_eq!(finish(&ws, cx, r), Vec::<String>::new());
    }

    #[gpui::test]
    fn custom_command_opens_a_tab_and_types_into_it(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [{"name": "hello", "actions":
                ["new_tab", {"type": "printf 'from_%s\\n' palette\n"}]}]}"#,
        );
        type_keys(cx, "ctrl-shift-p h e l l o enter");
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 1);
        let terminal = terminal(&ws, cx, 1);
        wait_until(cx, "typed command never ran", |cx| {
            terminal.read_with(cx, |t, _| line_starting_with(t, "from_palette").is_some())
        });
    }

    #[gpui::test]
    fn typed_command_waits_for_a_new_shell(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let command = Commands::parse(
            r#"{"commands": [{"name": "x", "actions":
                ["new_tab", {"type": "printf 'ok_%s\\n' shell\n"}]}]}"#,
        )
        .unwrap()
        .commands
        .remove(0);
        ws.update_in(cx, |ws, window, cx| ws.run_command(&command, window, cx));

        // the new shell has not printed anything yet, so the input is held back
        assert!(
            !ws.update(cx, |ws, _| ws.focused_pane().tabs[1]
                .pending_input
                .is_empty()),
            "typed input was sent before the shell was ready"
        );
        let terminal = terminal(&ws, cx, 1);
        wait_until(cx, "queued command never ran", |cx| {
            terminal.read_with(cx, |t, _| line_starting_with(t, "ok_shell").is_some())
        });
        assert!(ws.update(cx, |ws, _| {
            ws.focused_pane().tabs[1].pending_input.is_empty()
        }));
    }

    #[gpui::test]
    fn actions_after_close_tab_use_the_remaining_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let command = Commands::parse(
            r#"{"commands": [{"name": "x", "actions": ["close_tab", "next_tab", {"type": "a"}]}]}"#,
        )
        .unwrap()
        .commands
        .remove(0);
        ws.update_in(cx, |ws, window, cx| ws.run_command(&command, window, cx));
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 1);
        assert_eq!(current(&ws, cx), 0);
    }

    fn error_texts(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<String> {
        ws.read_with(cx, |ws, _| {
            ws.notifications
                .iter()
                .filter(|notification| notification.error)
                .map(|notification| notification.text.clone())
                .collect()
        })
    }

    #[gpui::test]
    fn reload_config_applies_every_section_and_keeps_the_last_good_one(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("workspace_reload");
        let config = dir.join("kuterm");
        std::fs::create_dir_all(config.join("themes")).unwrap();
        crate::settings::tests::with_config_home(&dir, || {
            let (ws, cx) = open(cx, 1);
            let reload = |cx: &mut VisualTestContext| {
                ws.update_in(cx, |ws, window, cx| ws.reload_config(window, cx));
                cx.run_until_parked();
            };
            let write = |name: &str, json: &str| std::fs::write(config.join(name), json).unwrap();
            let border = |cx: &mut VisualTestContext| cx.update(|_, cx| Theme::get(cx).border);
            let palette_enabled = |cx: &mut VisualTestContext| {
                cx.update(|_, cx| Settings::get(cx).command_palette.enable)
            };

            write(
                "settings.jsonc",
                r#"{
                    "keybindings": {"command_palette": "ctrl-shift-k"},
                    "commands": [{"name": "only", "actions": ["new_tab"]}],
                    "theme": {"mode": "dark", "dark": "themes/mine.jsonc"},
                }"#,
            );
            write("themes/mine.jsonc", r##"{"border": "#010203"}"##);
            reload(cx);
            assert_eq!(border(cx), gpui::rgb(0x010203).into());
            type_keys(cx, "ctrl-shift-p");
            assert!(!palette_open(&ws, cx));
            type_keys(cx, "ctrl-shift-k");
            assert_eq!(palette_names(&ws, cx), ["only"]);
            type_keys(cx, "escape");

            // theme files are read again too
            write("themes/mine.jsonc", r##"{"border": "#040506"}"##);
            reload(cx);
            assert_eq!(border(cx), gpui::rgb(0x040506).into());
            assert!(error_texts(&ws, cx).is_empty());

            // a broken settings file keeps everything as it was and tells what is wrong
            write(
                "settings.jsonc",
                r#"{"command_palette": {"enable": false}, "tab_width": "wide"}"#,
            );
            reload(cx);
            assert!(palette_enabled(cx));
            assert_eq!(border(cx), gpui::rgb(0x040506).into());
            let errors = error_texts(&ws, cx);
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(errors[0].contains("settings.jsonc"), "{errors:?}");
            assert!(errors[0].contains("wide"), "{errors:?}");

            // so does a broken theme file, the old error is replaced
            write(
                "settings.jsonc",
                r#"{"command_palette": {"enable": false},
                    "theme": {"mode": "dark", "dark": "themes/mine.jsonc"}}"#,
            );
            write("themes/mine.jsonc", r#"{"border": "nope"}"#);
            reload(cx);
            assert!(palette_enabled(cx));
            let errors = error_texts(&ws, cx);
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(errors[0].contains("mine.jsonc"), "{errors:?}");

            // fixed files apply and clear the error
            write("themes/mine.jsonc", r##"{"border": "#070809"}"##);
            reload(cx);
            assert!(!palette_enabled(cx));
            assert_eq!(border(cx), gpui::rgb(0x070809).into());
            assert!(error_texts(&ws, cx).is_empty());
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn reload_config_action_reloads(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("workspace_reload_action");
        let config = dir.join("kuterm");
        std::fs::create_dir_all(&config).unwrap();
        crate::settings::tests::with_config_home(&dir, || {
            let (ws, cx) = open(cx, 1);
            for (width, action) in [(101, "reload_config"), (102, "reload_config")] {
                std::fs::write(
                    config.join("settings.jsonc"),
                    format!(r#"{{"tab_width": {width}}}"#),
                )
                .unwrap();
                let json = format!(r#"{{"commands": [{{"name": "x", "actions": ["{action}"]}}]}}"#);
                let command = Commands::parse(&json).unwrap().commands.remove(0);
                ws.update_in(cx, |ws, window, cx| ws.run_command(&command, window, cx));
                cx.run_until_parked();
                assert_eq!(
                    cx.update(|_, cx| Settings::get(cx).tab_width),
                    width,
                    "{action}"
                );
            }
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn saved_config_files_are_reloaded(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("workspace_watch");
        let config = dir.join("kuterm");
        std::fs::create_dir_all(&config).unwrap();
        crate::settings::tests::with_config_home(&dir, || {
            let (ws, cx) = open(cx, 1);
            let tab_width =
                |cx: &mut VisualTestContext| cx.update(|_, cx| Settings::get(cx).tab_width);
            // the watcher's events come from its own thread
            let reloads_to = |cx: &mut VisualTestContext, width: u32| {
                wait_until(cx, "config was not reloaded", |cx| tab_width(cx) == width)
            };
            let settings = config.join("settings.jsonc");
            std::fs::write(&settings, r#"{"tab_width": 123}"#).unwrap();
            cx.update(|_, cx| {
                cx.set_global(ConfigFiles {
                    paths: vec![settings.clone()],
                    problems: Vec::new(),
                })
            });
            let _watch = cx.update(|_, cx| watch_config(cx));
            pump(cx);

            std::fs::write(&settings, r#"{"tab_width": 124}"#).unwrap();
            reloads_to(cx, 124);

            std::fs::write(&settings, r#"{"tab_width": 1234}"#).unwrap();
            reloads_to(cx, 1234);

            // editors that save to a new file and rename it over the old one
            let saved = config.join("settings.jsonc.tmp");
            std::fs::write(&saved, r#"{"tab_width": 321}"#).unwrap();
            std::fs::rename(&saved, &settings).unwrap();
            reloads_to(cx, 321);

            // an import that shows up later is noticed, it is watched while missing
            std::fs::write(&settings, r#"{"import": ["later.jsonc"], "tab_width": 77}"#).unwrap();
            reloads_to(cx, 77);
            assert_eq!(error_texts(&ws, cx).len(), 1);
            std::fs::write(config.join("later.jsonc"), r#"{"ui_font_size": 13}"#).unwrap();
            wait_until(cx, "import was not read", |cx| {
                cx.update(|_, cx| Settings::get(cx).ui_font_size) == 13.
            });
            assert!(error_texts(&ws, cx).is_empty());

            // a file in a folder made later, the nearest existing folder is watched
            let nested = config.join("more/nested.jsonc");
            std::fs::write(
                &settings,
                r#"{"import": ["more/nested.jsonc"], "tab_width": 78}"#,
            )
            .unwrap();
            reloads_to(cx, 78);
            std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
            std::fs::write(&nested, r#"{"ui_font_size": 14}"#).unwrap();
            wait_until(cx, "nested import was not read", |cx| {
                cx.update(|_, cx| Settings::get(cx).ui_font_size) == 14.
            });
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn launch_problems_are_shown_in_the_window(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(ConfigFiles {
                paths: Vec::new(),
                problems: vec!["invalid settings in x.jsonc".into()],
            })
        });
        let (ws, cx) = open(cx, 1);
        assert_eq!(error_texts(&ws, cx), ["invalid settings in x.jsonc"]);
        // shown once, not again for a later window
        assert!(cx.update(|_, cx| cx.global::<ConfigFiles>().problems.is_empty()));
    }

    fn recent_test_commands(cx: &mut VisualTestContext) {
        set_commands(
            cx,
            r#"{"commands": [
                {"name": "first", "actions": ["new_tab"]},
                {"name": "second", "actions": ["new_tab"]},
                {"name": "third", "actions": ["new_tab"]},
            ]}"#,
        );
    }

    #[gpui::test]
    fn recently_run_commands_are_listed_first(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        recent_test_commands(cx);
        type_keys(cx, "ctrl-shift-p t h i r d enter");
        type_keys(cx, "ctrl-shift-p");
        assert_eq!(palette_names(&ws, cx), ["third", "first", "second"]);
        type_keys(cx, "f i r s t enter");
        type_keys(cx, "ctrl-shift-p");
        assert_eq!(palette_names(&ws, cx), ["first", "third", "second"]);
    }

    #[gpui::test]
    fn disabled_recent_sorts_by_category_and_name(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"command_palette": {"show_recent": false}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        recent_test_commands(cx);
        type_keys(cx, "ctrl-shift-p t h i r d enter");
        type_keys(cx, "ctrl-shift-p");
        assert_eq!(palette_names(&ws, cx), ["first", "second", "third"]);
    }

    #[gpui::test]
    fn commands_are_grouped_by_category_then_name(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [
                {"name": "paste", "category": "edit", "actions": ["new_tab"]},
                {"name": "zeta", "actions": ["new_tab"]},
                {"name": "create new", "category": "tabs", "actions": ["new_tab"]},
                {"name": "copy", "category": "edit", "actions": ["new_tab"]},
            ]}"#,
        );
        type_keys(cx, "ctrl-shift-p");
        // categorized commands first, each group sorted by name, uncategorized last
        assert_eq!(
            palette_names(&ws, cx),
            ["edit: copy", "edit: paste", "tabs: create new", "zeta"]
        );
    }

    #[gpui::test]
    fn pinned_commands_are_listed_first(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        set_commands(
            cx,
            r#"{"commands": [
                {"name": "alpha", "actions": ["new_tab"]},
                {"name": "beta", "pinned": true, "actions": ["new_tab"]},
                {"name": "gamma", "actions": ["new_tab"]},
            ]}"#,
        );
        // run gamma so it is recent, the pinned command still wins
        type_keys(cx, "ctrl-shift-p g a m m a enter");
        type_keys(cx, "ctrl-shift-p");
        assert_eq!(palette_names(&ws, cx), ["beta", "gamma", "alpha"]);
    }

    #[gpui::test]
    fn clicking_the_pin_unpins_and_saves(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("workspace_unpin");
        crate::settings::tests::with_config_home(&dir, || {
            let (ws, cx) = open(cx, 1);
            set_commands(
                cx,
                r#"{"commands": [
                    {"name": "alpha", "actions": ["new_tab"]},
                    {"name": "beta", "pinned": true, "actions": ["new_tab"]},
                ]}"#,
            );
            type_keys(cx, "ctrl-shift-p");
            assert_eq!(palette_names(&ws, cx), ["beta", "alpha"]);

            let pin = cx.debug_bounds("pin-0").unwrap().center();
            click(cx, pin, 1);
            release(cx, pin);
            cx.run_until_parked();

            // the row was not run, the palette stays open and beta moved below alpha
            assert_eq!(views(&ws, cx).len(), 1);
            assert_eq!(palette_names(&ws, cx), ["alpha", "beta"]);
            assert_eq!(Pins::load().overrides.get("beta"), Some(&false));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn selection_scrolls_into_view(cx: &mut TestAppContext) {
        let (_ws, cx) = open(cx, 1);
        let commands = (0..40)
            .map(|n| format!(r#"{{"name": "cmd {n:02}", "actions": ["new_tab"]}}"#))
            .collect::<Vec<_>>()
            .join(",");
        set_commands(cx, &format!(r#"{{"commands": [{commands}]}}"#));
        type_keys(cx, "ctrl-shift-p");
        for _ in 0..35 {
            type_keys(cx, "down");
        }
        let list = cx.debug_bounds("command-list").unwrap();
        let item = cx.debug_bounds("command-35").unwrap();
        assert!(
            item.top() >= list.top() - gpui::px(1.)
                && item.bottom() <= list.bottom() + gpui::px(1.),
            "selected item {item:?} is outside the list {list:?}"
        );
    }

    #[gpui::test]
    fn tab_navigation_commands(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        assert_eq!(current(&ws, cx), 2);
        run_actions(&ws, cx, r#""prev_tab""#);
        assert_eq!(current(&ws, cx), 1);
        run_actions(&ws, cx, r#"{"activate_tab": 1}"#);
        assert_eq!(current(&ws, cx), 0);
        // out of range numbers are ignored
        run_actions(&ws, cx, r#"{"activate_tab": 9}"#);
        assert_eq!(current(&ws, cx), 0);
        // previous wraps from the first tab to the last
        run_actions(&ws, cx, r#""prev_tab""#);
        assert_eq!(current(&ws, cx), 2);
    }

    #[gpui::test]
    fn profile_command_opens_the_named_profile(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"profiles": [
                {"name": "default", "command": "system"},
                {"name": "dev", "command": "system", "icon": "D"},
            ]}"#,
        )
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        run_actions(&ws, cx, r#"{"new_tab_with_profile": "dev"}"#);
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 1);
        assert_eq!(
            ws.update(cx, |ws, _| ws.focused_pane().tabs[1].icon.clone()),
            "D"
        );
    }

    #[gpui::test]
    fn unknown_profile_command_opens_nothing(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        run_actions(&ws, cx, r#"{"new_tab_with_profile": "missing"}"#);
        run_actions(&ws, cx, r#"{"new_background_tab_with_profile": "missing"}"#);
        assert_eq!(views(&ws, cx).len(), 1);
        assert_eq!(
            notifications(&ws, cx),
            [
                r#"no profile named "missing""#,
                r#"no profile named "missing""#
            ]
        );
    }

    fn notifications(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<String> {
        ws.update(cx, |ws, _| {
            ws.notifications
                .iter()
                .map(|notification| match &notification.detail {
                    Some(detail) => format!("{} {detail}", notification.text),
                    None => notification.text.clone(),
                })
                .collect()
        })
    }

    #[gpui::test]
    fn notify_command_shows_until_timeout(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"notifications": {"timeout": 2}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        run_actions(&ws, cx, r#"{"notify": "hello"}"#);
        assert_eq!(notifications(&ws, cx), ["hello"]);
        assert!(cx.debug_bounds("notification-0").is_some());
        cx.executor().advance_clock(Duration::from_millis(1500));
        cx.run_until_parked();
        assert_eq!(notifications(&ws, cx), ["hello"]);
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert!(notifications(&ws, cx).is_empty());
    }

    #[gpui::test]
    fn zero_timeout_keeps_notification_until_clicked(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"notifications": {"timeout": 0}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        run_actions(&ws, cx, r#"{"notify": "sticky"}"#);
        cx.executor().advance_clock(Duration::from_secs(3600));
        cx.run_until_parked();
        assert_eq!(notifications(&ws, cx), ["sticky"]);
        let center = cx.debug_bounds("notification-0").unwrap().center();
        click(cx, center, 1);
        release(cx, center);
        cx.run_until_parked();
        assert!(notifications(&ws, cx).is_empty());
    }

    #[gpui::test]
    fn disabled_notifications_show_nothing(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"notifications": {"enable": false}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        run_actions(&ws, cx, r#"{"notify": "hello"}"#);
        assert!(notifications(&ws, cx).is_empty());
        assert!(cx.debug_bounds("notification-0").is_none());
    }

    #[gpui::test]
    fn palette_actions_report_in_notifications(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("workspace_report");
        // reloading reads the config folder, so it is a temp one
        crate::settings::tests::with_config_home(&dir, || {
            let (ws, cx) = open(cx, 1);
            run_actions(&ws, cx, r#""reload_config", "copy""#);
            assert_eq!(
                notifications(&ws, cx),
                ["config reloaded", "nothing selected to copy"]
            );
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn copy_and_paste_keys_notify(cx: &mut TestAppContext) {
        let settings =
            Settings::parse(r#"{"notifications": {"timeout": 0, "copy": true, "paste": true}}"#)
                .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        print_line(&ws, cx, "pick these words");
        let position = cell_of(&ws, cx, "pick these words", 12, false);
        click(cx, position, 2);
        release(cx, position);
        cx.simulate_keystrokes("ctrl-shift-c");
        cx.simulate_keystrokes("ctrl-shift-v");
        cx.run_until_parked();
        assert_eq!(
            notifications(&ws, cx),
            ["copied words", "pasted from clipboard"]
        );
    }

    #[gpui::test]
    fn copy_and_paste_notifications_can_be_turned_off(cx: &mut TestAppContext) {
        let settings =
            Settings::parse(r#"{"notifications": {"copy": false, "paste": false}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        print_line(&ws, cx, "pick these words");
        let position = cell_of(&ws, cx, "pick these words", 12, false);
        click(cx, position, 2);
        release(cx, position);
        cx.simulate_keystrokes("ctrl-shift-c");
        run_actions(&ws, cx, r#""copy", "paste""#);
        cx.simulate_keystrokes("ctrl-shift-v");
        cx.run_until_parked();
        assert!(notifications(&ws, cx).is_empty());
    }

    #[gpui::test]
    fn background_tab_runs_command_and_notifies_when_done(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"notifications": {"timeout": 0}}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        let file = std::env::temp_dir().join(format!("kuterm_bg_done_{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let actions = format!(
            r#""new_background_tab", {{"type": "sleep 0.5; touch {}\n"}}, {{"notify_when_done": "bg done"}}"#,
            file.display()
        );
        run_actions(&ws, cx, &actions);

        // the new tab opened behind the active one, which keeps focus
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 0);
        wait_until(cx, "background command never finished", |cx| {
            !notifications(&ws, cx).is_empty()
        });
        assert!(file.exists(), "notified before the command finished");
        assert_eq!(notifications(&ws, cx), ["bg done"]);

        // clicking the notification switches to the tab that finished
        let center = cx.debug_bounds("notification-0").unwrap().center();
        click(cx, center, 1);
        release(cx, center);
        cx.run_until_parked();
        assert_eq!(current(&ws, cx), 1);
        assert!(notifications(&ws, cx).is_empty());
        let _ = std::fs::remove_file(&file);
    }

    /// start a long program in the active tab and wait until it runs
    fn start_program(ws: &Entity<Workspace>, cx: &mut VisualTestContext) {
        ws.update(cx, |ws, cx| {
            let pane = ws.focused;
            let ix = ws.focused_pane().active;
            ws.input_to(pane, ix, b"sleep 30\n".to_vec(), cx)
        });
        wait_until(cx, "sleep never started", |cx| {
            ws.update(cx, |ws, cx| {
                let pane = ws.focused;
                let ix = ws.focused_pane().active;
                ws.running_program(pane, ix, cx).is_some()
            })
        });
    }

    fn close_active(ws: &Entity<Workspace>, cx: &mut VisualTestContext) {
        ws.update_in(cx, |ws, window, cx| ws.close_tab(&CloseTab, window, cx));
        cx.run_until_parked();
    }

    #[gpui::test]
    fn running_tab_asks_before_closing(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        assert_eq!(views(&ws, cx).len(), 2);
        assert!(cx.debug_bounds("confirm-close").is_some());
        assert!(ws.update_in(cx, |ws, window, cx| {
            ws.confirm_close
                .as_ref()
                .is_some_and(|confirm| confirm.focus_handle(cx).is_focused(window))
        }));

        // escape keeps the tab and gives focus back to it
        type_keys(cx, "escape");
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 1);

        close_active(&ws, cx);
        type_keys(cx, "enter");
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 1);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn arrows_pick_the_button_enter_presses(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        // "close" starts selected, left moves to "cancel"
        type_keys(cx, "left enter");
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 1);

        close_active(&ws, cx);
        type_keys(cx, "left right enter");
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[gpui::test]
    fn dialog_closes_when_its_tab_exits(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        // ctrl-c stops sleep, then the shell exits and its tab closes by itself
        terminal(&ws, cx, 1).update(cx, |terminal, _| terminal.input(b"\x03exit\n".to_vec()));
        wait_until(cx, "tab never exited", |cx| views(&ws, cx).len() == 1);
        cx.run_until_parked();
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn close_tabs_to_the_right_keeps_the_active_and_its_left_neighbors(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 4);
        let old = views(&ws, cx);
        assert_eq!(activate(&ws, cx, 1), 1);
        run_actions(&ws, cx, r#""close_tabs_to_right""#);
        assert_eq!(views(&ws, cx), vec![old[0], old[1]]);
        assert_eq!(current(&ws, cx), 1);
        assert!(cx.debug_bounds("confirm-close").is_none());
    }

    #[gpui::test]
    fn close_tabs_to_the_left_keeps_the_active_and_its_right_neighbors(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 4);
        let old = views(&ws, cx);
        assert_eq!(activate(&ws, cx, 2), 2);
        run_actions(&ws, cx, r#""close_tabs_to_left""#);
        assert_eq!(views(&ws, cx), vec![old[2], old[3]]);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn closing_an_empty_tab_side_does_nothing(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        let old = views(&ws, cx);
        assert_eq!(activate(&ws, cx, 1), 1);
        run_actions(&ws, cx, r#""close_tabs_to_right""#);
        assert_eq!(views(&ws, cx), old);
        assert_eq!(current(&ws, cx), 1);
        assert_eq!(activate(&ws, cx, 0), 0);
        run_actions(&ws, cx, r#""close_tabs_to_left""#);
        assert_eq!(views(&ws, cx), old);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn close_tabs_side_asks_once_for_running_programs(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        let old = views(&ws, cx);
        start_program(&ws, cx);
        activate(&ws, cx, 0);
        run_actions(&ws, cx, r#""close_tabs_to_right""#);
        assert_eq!(views(&ws, cx), old);
        let (target, message) = dialog(&ws, cx).expect("busy side closed without asking");
        assert_eq!(target, CloseTarget::Tabs(vec![old[1], old[2]]));
        assert_eq!(message, "a tab to close is still running a program: sleep");
        type_keys(cx, "enter");
        assert_eq!(views(&ws, cx), vec![old[0]]);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn close_tabs_side_dialog_survives_one_tab_exiting(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        let old = views(&ws, cx);
        start_program(&ws, cx);
        activate(&ws, cx, 0);
        // ask about the two tabs to the right, then end the shell of the first
        run_actions(&ws, cx, r#""close_tabs_to_right""#);
        assert_eq!(views(&ws, cx), old);
        terminal(&ws, cx, 1).update(cx, |terminal, _| terminal.input(b"exit\n".to_vec()));
        wait_until(cx, "tab never exited", |cx| views(&ws, cx).len() == 2);
        cx.run_until_parked();
        // one target is gone, the other still runs, so the dialog stays up
        assert!(cx.debug_bounds("confirm-close").is_some());
        type_keys(cx, "enter");
        assert_eq!(views(&ws, cx), vec![old[0]]);
        assert_eq!(current(&ws, cx), 0);
    }

    #[test]
    fn side_close_message_lists_a_few_programs() {
        let names = |n: &[&str]| n.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(
            side_close_message(&names(&["vim"])),
            "a tab to close is still running a program: vim"
        );
        assert_eq!(
            side_close_message(&names(&["vim", "cargo"])),
            "2 tabs to close are still running programs: vim, cargo"
        );
    }

    #[gpui::test]
    fn close_button_in_dialog_closes_running_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        run_actions(&ws, cx, r#""close_tab""#);
        let button = cx.debug_bounds("confirm-close-ok").unwrap().center();
        click(cx, button, 1);
        release(cx, button);
        cx.run_until_parked();
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[gpui::test]
    fn disabled_warning_closes_running_tab_at_once(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"close_running_tab_warn": false}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        start_program(&ws, cx);
        close_active(&ws, cx);
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[gpui::test]
    fn idle_tab_closes_without_asking(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"close_running_tab_warn": true}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        close_active(&ws, cx);
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 1);
    }

    fn dialog(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<(CloseTarget, String)> {
        ws.update(cx, |ws, cx| {
            ws.confirm_close.as_ref().map(|confirm| {
                let confirm = confirm.read(cx);
                (confirm.target.clone(), confirm.message().to_string())
            })
        })
    }

    fn quitting(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
        ws.update(cx, |ws, _| ws.quitting)
    }

    #[test]
    fn quit_message_lists_a_few_programs() {
        let names = |n: &[&str]| n.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(
            quit_message(&names(&["vim"])),
            "1 tab is still running a program: vim"
        );
        assert_eq!(
            quit_message(&names(&["vim", "cargo"])),
            "2 tabs are still running programs: vim, cargo"
        );
        assert_eq!(
            quit_message(&names(&["a", "b", "c", "d"])),
            "4 tabs are still running programs: a, b, c, …"
        );
        assert_eq!(quit_message(&[]), "quit kuterm?");
    }

    #[gpui::test]
    fn quit_action_asks_with_running_program(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        run_actions(&ws, cx, r#""quit""#);
        let (target, message) = dialog(&ws, cx).expect("quit asked nothing");
        assert_eq!(target, CloseTarget::Window);
        assert_eq!(message, "1 tab is still running a program: sleep");
        assert!(!quitting(&ws, cx));

        type_keys(cx, "escape");
        assert!(dialog(&ws, cx).is_none());
        assert!(!quitting(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 2);

        run_actions(&ws, cx, r#""quit""#);
        type_keys(cx, "enter");
        assert!(quitting(&ws, cx));
    }

    #[gpui::test]
    fn idle_quit_and_disabled_warning_quit_at_once(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        run_actions(&ws, cx, r#""quit""#);
        assert!(dialog(&ws, cx).is_none());
        assert!(quitting(&ws, cx));

        let settings = Settings::parse(r#"{"confirm_quit": "never"}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        start_program(&ws, cx);
        run_actions(&ws, cx, r#""quit""#);
        assert!(dialog(&ws, cx).is_none());
        assert!(quitting(&ws, cx));
    }

    #[gpui::test]
    fn confirm_quit_always_asks_when_idle(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"confirm_quit": "always"}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        run_actions(&ws, cx, r#""quit""#);
        let (target, message) = dialog(&ws, cx).expect("quit asked nothing");
        assert_eq!(target, CloseTarget::Window);
        assert_eq!(message, "quit kuterm?");
        type_keys(cx, "escape");
        assert!(!quitting(&ws, cx));
        assert!(!cx.simulate_close(), "idle window closed without asking");
        type_keys(cx, "enter");
        assert!(quitting(&ws, cx));
    }

    #[gpui::test]
    fn confirm_quit_is_separate_from_close_tab_warning(cx: &mut TestAppContext) {
        let settings =
            Settings::parse(r#"{"close_running_tab_warn": false, "confirm_quit": "running"}"#)
                .unwrap();
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        start_program(&ws, cx);
        assert!(!cx.simulate_close(), "busy window closed without asking");
        assert_eq!(dialog(&ws, cx).map(|d| d.0), Some(CloseTarget::Window));
    }

    #[gpui::test]
    fn idle_window_closes_at_once(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        assert!(cx.simulate_close(), "idle window kept open");
        assert!(dialog(&ws, cx).is_none());
    }

    #[gpui::test]
    fn window_close_asks_with_running_program(cx: &mut TestAppContext) {
        let (ws2, cx2) = open(cx, 1);
        start_program(&ws2, cx2);
        assert!(!cx2.simulate_close(), "busy window closed without asking");
        assert_eq!(dialog(&ws2, cx2).map(|d| d.0), Some(CloseTarget::Window));
        // closing again while asked keeps the dialog
        assert!(!cx2.simulate_close());
        type_keys(cx2, "escape");
        assert!(dialog(&ws2, cx2).is_none());
        assert!(!quitting(&ws2, cx2));

        assert!(!cx2.simulate_close());
        type_keys(cx2, "enter");
        assert!(quitting(&ws2, cx2));
        assert!(
            cx2.simulate_close(),
            "confirmed quit still blocks the close"
        );
    }

    #[gpui::test]
    fn disabled_warning_closes_busy_window(cx: &mut TestAppContext) {
        let settings = Settings::parse(r#"{"confirm_quit": "never"}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        start_program(&ws, cx);
        assert!(cx.simulate_close());
        assert!(dialog(&ws, cx).is_none());
    }

    #[gpui::test]
    fn middle_click_asks_about_the_clicked_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 3);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        activate(&ws, cx, 1);
        start_program(&ws, cx);
        activate(&ws, cx, 2);
        let old = views(&ws, cx);
        let title = cx.debug_bounds("tab-title-1").unwrap().center();
        cx.simulate_event(MouseDownEvent {
            position: title,
            modifiers: Modifiers::default(),
            button: MouseButton::Middle,
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        let (target, message) = dialog(&ws, cx).expect("busy tab closed without asking");
        assert_eq!(target, CloseTarget::Tabs(vec![old[1]]));
        assert!(
            message.starts_with("\"sleep\" is still running in tab 2 \""),
            "{message}"
        );
        type_keys(cx, "enter");
        assert_eq!(views(&ws, cx), vec![old[0], old[2]]);
        // the active tab stays active, it just moved left
        assert_eq!(current(&ws, cx), 1);
    }

    #[gpui::test]
    fn modified_keys_do_not_answer_the_dialog(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        for keys in [
            "ctrl-y",
            "ctrl-n",
            "alt-y",
            "ctrl-enter",
            "shift-enter",
            "ctrl-escape",
        ] {
            type_keys(cx, keys);
            assert!(dialog(&ws, cx).is_some(), "{keys} answered the dialog");
        }
        // arrows work with modifiers held
        type_keys(cx, "shift-left enter");
        assert!(dialog(&ws, cx).is_none());
        assert_eq!(views(&ws, cx).len(), 2);

        close_active(&ws, cx);
        type_keys(cx, "shift-n");
        assert!(dialog(&ws, cx).is_none());
        assert_eq!(views(&ws, cx).len(), 2);
        close_active(&ws, cx);
        type_keys(cx, "shift-y");
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[gpui::test]
    fn plain_y_and_n_answer_the_dialog(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        type_keys(cx, "n");
        assert!(dialog(&ws, cx).is_none());
        assert_eq!(views(&ws, cx).len(), 2);
        close_active(&ws, cx);
        type_keys(cx, "y");
        assert_eq!(views(&ws, cx).len(), 1);
    }

    #[gpui::test]
    fn close_tab_key_does_not_stack_dialogs(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        type_keys(cx, "ctrl-shift-w");
        let first = ws.update(cx, |ws, _| ws.confirm_close.as_ref().map(|c| c.entity_id()));
        assert!(first.is_some());
        type_keys(cx, "ctrl-shift-w");
        let second = ws.update(cx, |ws, _| ws.confirm_close.as_ref().map(|c| c.entity_id()));
        assert_eq!(first, second, "a second dialog replaced the first");
        type_keys(cx, "escape");
        assert!(dialog(&ws, cx).is_none());
        assert_eq!(views(&ws, cx).len(), 2);
    }

    #[gpui::test]
    fn palette_key_replaces_the_dialog(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        type_keys(cx, "ctrl-shift-p");
        assert!(dialog(&ws, cx).is_none());
        assert!(palette_focused(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 2);
    }

    #[gpui::test]
    fn new_tab_key_closes_the_dialog(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        type_keys(cx, "ctrl-shift-t");
        assert!(dialog(&ws, cx).is_none());
        assert!(cx.debug_bounds("confirm-close").is_none());
        assert_eq!(views(&ws, cx).len(), 3);
        assert_eq!(current(&ws, cx), 2);
    }

    #[gpui::test]
    fn tab_switch_key_closes_the_dialog(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        start_program(&ws, cx);
        close_active(&ws, cx);
        type_keys(cx, "alt-1");
        assert!(dialog(&ws, cx).is_none());
        assert_eq!(views(&ws, cx).len(), 2);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn f11_toggles_fullscreen(cx: &mut TestAppContext) {
        let (_ws, cx) = open(cx, 1);
        assert!(!cx.update(|window, _| window.is_fullscreen()));
        type_keys(cx, "f11");
        assert!(cx.update(|window, _| window.is_fullscreen()));
        type_keys(cx, "f11");
        assert!(!cx.update(|window, _| window.is_fullscreen()));
    }

    #[gpui::test]
    fn close_tab_after_background_tab_closes_it(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let before = views(&ws, cx);
        run_actions(&ws, cx, r#""new_background_tab", "close_tab""#);
        assert_eq!(views(&ws, cx), before);
        assert_eq!(current(&ws, cx), 0);
    }

    #[gpui::test]
    fn copy_and_paste_commands(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        print_line(&ws, cx, "pick these words");
        let start = cell_of(&ws, cx, "pick these words", 5, false);
        let end = cell_of(&ws, cx, "pick these words", 9, true);
        click(cx, start, 1);
        cx.simulate_event(MouseMoveEvent {
            position: end,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        });
        release(cx, end);
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
        run_actions(&ws, cx, r#""copy""#);
        assert_eq!(clipboard(cx).as_deref(), Some("these"));

        // paste sends the clipboard to the shell, read it back as raw bytes
        let r = reader(&ws, cx, false, "paste_command");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("hi".into()));
        run_actions(&ws, cx, r#""paste""#);
        assert_eq!(finish(&ws, cx, r), ["68", "69"]);
    }

    #[gpui::test]
    fn scroll_commands_move_the_view(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let terminal = terminal(&ws, cx, 0);
        send_to(&ws, cx, 0, "seq 1 300; echo DONE\r");
        wait_until(cx, "output never finished", |cx| {
            terminal.read_with(cx, |t, _| line_starting_with(t, "DONE").is_some())
        });
        let top = terminal.read_with(cx, |t, _| t.history_size());
        run_actions(&ws, cx, r#""scroll_top""#);
        assert_eq!(scroll_offset(&terminal, cx), top);
        run_actions(&ws, cx, r#"{"scroll_down": 10}"#);
        assert_eq!(scroll_offset(&terminal, cx), top - 10);
        run_actions(&ws, cx, r#"{"scroll_up": 5}"#);
        assert_eq!(scroll_offset(&terminal, cx), top - 5);
        run_actions(&ws, cx, r#""scroll_bottom""#);
        assert_eq!(scroll_offset(&terminal, cx), 0);
    }

    fn window_settings(json: &str) -> Settings {
        Settings::parse(&format!(
            r#"{{"hide_bar_for_one_tab": true, "terminal": {{"scrollbar": {{"enable": "off"}}}},
                "window": {json}}}"#
        ))
        .unwrap()
    }

    fn grid_bounds(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> TerminalBounds {
        let terminal = terminal(ws, cx, 0);
        terminal.read_with(cx, |t, _| t.last_content.terminal_bounds)
    }

    #[gpui::test]
    fn padding_moves_and_shrinks_the_grid(cx: &mut TestAppContext) {
        let settings = window_settings(
            r#"{"decorations": "full", "padding": {"x": 30, "y": 20}, "padding_balance": false}"#,
        );
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        cx.run_until_parked();
        let b = grid_bounds(&ws, cx);
        assert_eq!(b.bounds.origin, point(gpui::px(30.), gpui::px(20.)));
        assert_eq!(b.bounds.size, gpui::size(gpui::px(840.), gpui::px(560.)));
        assert_eq!(b.num_columns(), (840. / f32::from(b.cell_width)) as usize);
        assert_eq!(b.num_lines(), (560. / f32::from(b.line_height)) as usize);
    }

    #[gpui::test]
    fn balanced_padding_centers_the_grid(cx: &mut TestAppContext) {
        let settings = window_settings(
            r#"{"decorations": "full", "padding": {"x": 30, "y": 20}, "padding_balance": true}"#,
        );
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        cx.run_until_parked();
        let b = grid_bounds(&ws, cx);
        let spare_x = 840. % f32::from(b.cell_width);
        let spare_y = 560. % f32::from(b.line_height);
        // origins are snapped down to whole device pixels
        let expected_x = (30. + spare_x / 2.).floor();
        let expected_y = (20. + spare_y / 2.).floor();
        assert_eq!(f32::from(b.bounds.origin.x), expected_x);
        assert_eq!(f32::from(b.bounds.origin.y), expected_y);
        // the grid is never pushed past the padding on the far side
        let right = f32::from(b.bounds.origin.x + b.cell_width * b.num_columns() as f32);
        let bottom = f32::from(b.bounds.origin.y + b.line_height * b.num_lines() as f32);
        assert!(right <= 870., "{right}");
        assert!(bottom <= 580., "{bottom}");
    }

    #[gpui::test]
    fn startup_size_fits_columns_and_rows(cx: &mut TestAppContext) {
        let settings = window_settings(
            r#"{"decorations": "full", "padding": {"x": 7, "y": 3}, "columns": 83, "rows": 27,
                "startup": "windowed", "remember_size": false}"#,
        );
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        let bounds = cx.update(|_, cx| startup_bounds(cx));
        cx.simulate_resize(bounds.size);
        cx.run_until_parked();
        let b = grid_bounds(&ws, cx);
        assert_eq!((b.num_columns(), b.num_lines()), (83, 27));
        // the shell spawned before the window gets the same grid
        let early = cx.update(|_, cx| window::startup_grid(bounds.size, cx));
        assert_eq!((early.num_columns(), early.num_lines()), (83, 27));
    }

    #[gpui::test]
    fn first_tab_runs_the_shell_spawned_before_the_window(cx: &mut TestAppContext) {
        let settings = window_settings(r#"{"columns": 50, "rows": 10, "remember_size": false}"#);
        let (_ws, cx) = open_with_settings(cx, 1, "{}", settings);
        let bounds = cx.update(|_, cx| startup_bounds(cx));
        let first = cx
            .update(|_, cx| Workspace::spawn_first_tab(bounds.size, cx))
            .expect("failed to spawn shell");
        let (pid, grid) = (
            first.terminal.shell_pid,
            first.terminal.last_content.terminal_bounds,
        );
        assert_eq!((grid.num_columns(), grid.num_lines()), (50, 10));
        let (ws, cx) =
            cx.add_window_view(|window, cx| Workspace::with_start(None, Some(first), window, cx));
        cx.run_until_parked();
        let tabs = ws.update(cx, |ws, _| ws.focused_pane().tabs.len());
        assert_eq!(tabs, 1);
        assert_eq!(terminal(&ws, cx, 0).read_with(cx, |t, _| t.shell_pid), pid);
    }

    #[gpui::test]
    fn startup_size_counts_the_tab_bar_without_frame(cx: &mut TestAppContext) {
        let settings = window_settings(
            r#"{"decorations": "none", "padding": {"x": 0, "y": 0}, "columns": 50, "rows": 10,
                "startup": "windowed", "remember_size": false}"#,
        );
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        let bounds = cx.update(|_, cx| startup_bounds(cx));
        cx.simulate_resize(bounds.size);
        cx.run_until_parked();
        assert!(cx.debug_bounds("drag-area").is_some(), "tab bar is hidden");
        let b = grid_bounds(&ws, cx);
        assert_eq!((b.num_columns(), b.num_lines()), (50, 10));
        let early = cx.update(|_, cx| window::startup_grid(bounds.size, cx));
        assert_eq!((early.num_columns(), early.num_lines()), (50, 10));
    }

    #[gpui::test]
    fn frameless_window_shows_tab_bar_with_drag_area(cx: &mut TestAppContext) {
        let settings = window_settings(r#"{"decorations": "full"}"#);
        let (_ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        assert!(cx.debug_bounds("drag-area").is_none());
        assert!(
            cx.debug_bounds("new-tab").is_none(),
            "one tab shows the bar"
        );

        let settings = window_settings(r#"{"decorations": "none"}"#);
        let (_ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        let area = cx.debug_bounds("drag-area").expect("no drag area");
        let tab = cx.debug_bounds("tab-title-0").expect("no tab bar");
        assert!(area.left() >= tab.right(), "drag area overlaps the tab");
        assert!(cx.debug_bounds("window-close").is_none());
    }

    #[gpui::test]
    fn client_frame_draws_window_buttons_in_the_corner_pane(cx: &mut TestAppContext) {
        let settings = window_settings(r#"{"decorations": "client"}"#);
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        for id in ["window-minimize", "window-maximize", "window-close"] {
            let button = cx.debug_bounds(id).unwrap_or_else(|| panic!("no {id}"));
            assert!(button.left() > gpui::px(450.), "{id} is not on the right");
        }
        run_actions(&ws, cx, r#""split_left""#);
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
        // the moved tab is on the left, the buttons stay at the right edge
        let close = cx.debug_bounds("window-close").expect("no close button");
        assert!(close.left() > gpui::px(450.));
        assert_eq!(ws.update(cx, |ws, _| ws.layout.panes().len()), 2);

        // the close button asks like any other quit
        start_program(&ws, cx);
        cx.simulate_click(close.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(dialog(&ws, cx).map(|d| d.0), Some(CloseTarget::Window));
    }

    #[test]
    fn corner_pane_is_top_right() {
        let leaf = |id| {
            Box::new(LayoutNode::Leaf(Pane {
                id,
                tabs: Vec::new(),
                active: 0,
                tab_scroll: ScrollHandle::new(),
            }))
        };
        assert_eq!(leaf(3).corner_pane_id(), Some(3));
        let side_by_side = LayoutNode::Split {
            axis: SplitAxis::Horizontal,
            ratio: 0.5,
            first: leaf(1),
            second: leaf(2),
        };
        assert_eq!(side_by_side.corner_pane_id(), Some(2));
        // right column split into top and bottom
        let nested = LayoutNode::Split {
            axis: SplitAxis::Horizontal,
            ratio: 0.5,
            first: leaf(1),
            second: Box::new(LayoutNode::Split {
                axis: SplitAxis::Vertical,
                ratio: 0.5,
                first: leaf(4),
                second: leaf(5),
            }),
        };
        assert_eq!(nested.corner_pane_id(), Some(4));
        assert_eq!(LayoutNode::Empty.corner_pane_id(), None);
    }

    #[test]
    fn opacity_and_blur_pick_background_appearance() {
        use gpui::WindowBackgroundAppearance as Appearance;
        for (json, expected) in [
            (r#"{"opacity": 1.0, "blur": true}"#, Appearance::Opaque),
            (
                r#"{"opacity": 0.8, "blur": false}"#,
                Appearance::Transparent,
            ),
            (r#"{"opacity": 0.8, "blur": true}"#, Appearance::Blurred),
        ] {
            assert_eq!(
                background_appearance(&window_settings(json)),
                expected,
                "{json}"
            );
        }
        for (mode, expected) in [
            ("full", gpui::WindowDecorations::Server),
            ("none", gpui::WindowDecorations::Client),
            ("client", gpui::WindowDecorations::Client),
        ] {
            let settings = window_settings(&format!(r#"{{"decorations": "{mode}"}}"#));
            assert_eq!(window_decorations(&settings), expected, "{mode}");
        }
    }

    /// active tab's terminal with bracketed paste forced off, like a shell that never asks for it
    fn without_bracketed_paste(ws: &Entity<Workspace>, cx: &mut VisualTestContext) {
        let ix = current(ws, cx);
        terminal(ws, cx, ix).update(cx, |terminal, _| {
            terminal.last_content.mode.remove(TermMode::BRACKETED_PASTE)
        });
    }

    #[gpui::test]
    fn unsafe_paste_asks_and_pastes_only_when_confirmed(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"notifications": {"timeout": 0}, "terminal": {"paste_protection": true}}"#,
        )
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.write_to_clipboard(ClipboardItem::new_string("echo one\necho two\n".into()));
        without_bracketed_paste(&ws, cx);
        run_actions(&ws, cx, r#""paste""#);
        assert!(ws.update(cx, |ws, _| ws.confirm_paste.is_some()));
        let preview = ws.update(cx, |ws, cx| {
            ws.confirm_paste
                .as_ref()
                .unwrap()
                .read(cx)
                .preview()
                .to_vec()
        });
        assert_eq!(preview, ["echo one", "echo two"]);
        // escape cancels, nothing is pasted
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(ws.update(cx, |ws, _| ws.confirm_paste.is_none()));
        assert!(notifications(&ws, cx).is_empty());

        without_bracketed_paste(&ws, cx);
        run_actions(&ws, cx, r#""paste""#);
        cx.simulate_keystrokes("y");
        cx.run_until_parked();
        assert!(ws.update(cx, |ws, _| ws.confirm_paste.is_none()));
        assert_eq!(notifications(&ws, cx), ["pasted from clipboard"]);
    }

    #[gpui::test]
    fn safe_or_unprotected_paste_does_not_ask(cx: &mut TestAppContext) {
        for (protection, text) in [(true, "echo one"), (false, "echo one\necho two\n")] {
            let settings = Settings::parse(&format!(
                r#"{{"notifications": {{"timeout": 0}}, "terminal": {{"paste_protection": {protection}}}}}"#
            ))
            .unwrap();
            let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
            cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
            without_bracketed_paste(&ws, cx);
            run_actions(&ws, cx, r#""paste""#);
            assert!(
                ws.update(cx, |ws, _| ws.confirm_paste.is_none()),
                "{text:?}"
            );
            assert_eq!(notifications(&ws, cx), ["pasted from clipboard"]);
        }
    }

    #[gpui::test]
    fn clipboard_ask_waits_for_an_answer_button(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"notifications": {"enable": false}, "terminal": {"clipboard": {"write": "ask", "read": "ask"}}}"#,
        )
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
        let terminal = terminal(&ws, cx, 0);
        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                alacritty_terminal::event::Event::ClipboardStore(
                    alacritty_terminal::term::ClipboardType::Clipboard,
                    "from program".into(),
                ),
                cx,
            )
        });
        cx.run_until_parked();
        // asked even with notifications off, a program waits for it
        assert_eq!(notifications(&ws, cx), ["tab 1 wants to copy from program"]);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("before".into())
        );
        // clicking the text only does not answer
        let center = cx.debug_bounds("notification-0").unwrap().center();
        click(cx, center, 1);
        release(cx, center);
        cx.run_until_parked();
        assert_eq!(notifications(&ws, cx).len(), 1);
        let allow = cx.debug_bounds("notification-0-button-0").unwrap().center();
        click(cx, allow, 1);
        release(cx, allow);
        cx.run_until_parked();
        assert!(notifications(&ws, cx).is_empty());
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("from program".into())
        );

        // deny leaves the clipboard alone
        terminal.update(cx, |terminal, cx| {
            terminal.process_event(
                alacritty_terminal::event::Event::ClipboardStore(
                    alacritty_terminal::term::ClipboardType::Clipboard,
                    "second".into(),
                ),
                cx,
            )
        });
        cx.run_until_parked();
        let deny = cx.debug_bounds("notification-1-button-1").unwrap().center();
        click(cx, deny, 1);
        release(cx, deny);
        cx.run_until_parked();
        assert!(notifications(&ws, cx).is_empty());
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("from program".into())
        );
    }

    #[gpui::test]
    fn kitty_release_events_reach_the_program(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let reading = reader_with(&ws, cx, "printf '\\033[>3u'; ", "kitty_release", |mode| {
            mode.contains(TermMode::REPORT_EVENT_TYPES)
        });
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("ctrl-a").unwrap(),
        });
        pump(cx);
        let got = finish(&ws, cx, reading).concat();
        // ESC [ 9 7 ; 5 u, then the same with :3 for the release
        assert!(got.starts_with("1b5b39373b35751b5b39373b353a3375"), "{got}");
    }

    fn search_bar(
        ws: &Entity<Workspace>,
        cx: &mut VisualTestContext,
    ) -> Option<Entity<crate::ui::search_bar::SearchBar>> {
        ws.update(cx, |ws, cx| {
            let pane = ws.focused_pane();
            pane.tabs[pane.active].view.read(cx).search_bar().cloned()
        })
    }

    #[gpui::test]
    fn search_key_opens_bar_and_escape_closes_it(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        print_line(&ws, cx, "find_me_here");
        cx.simulate_keystrokes("ctrl-shift-f");
        cx.run_until_parked();
        let bar = search_bar(&ws, cx).expect("search bar opened");
        assert!(cx.debug_bounds("search-bar").is_some());
        cx.simulate_input("find_me");
        cx.run_until_parked();
        let status = bar.read_with(cx, |bar, _| bar.status().clone()).unwrap();
        assert_eq!((status.count, status.active), (1, Some(0)));
        let terminal = terminal(&ws, cx, 0);
        assert!(terminal.read_with(cx, |t, _| t.last_content.active_match.is_some()));
        // enter walks the matches, nothing reaches the shell
        cx.simulate_keystrokes("enter shift-enter");
        cx.run_until_parked();

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(search_bar(&ws, cx).is_none());
        assert!(terminal.read_with(cx, |t, _| !t.searching()));
        // focus is back on the terminal
        current(&ws, cx);
    }

    #[gpui::test]
    fn search_options_toggle_with_buttons(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        print_line(&ws, cx, "Mixed mixed");
        run_actions(&ws, cx, r#""search""#);
        let bar = search_bar(&ws, cx).expect("palette action opens the bar");
        cx.simulate_input("mixed");
        cx.run_until_parked();
        let count = |cx: &mut VisualTestContext| {
            bar.read_with(cx, |bar, _| bar.status().clone())
                .map(|status| status.count)
        };
        // lower case query ignores case, the typed command line is not printed text
        let both = count(cx).unwrap();
        let center = cx.debug_bounds("search-case").unwrap().center();
        click(cx, center, 1);
        release(cx, center);
        cx.run_until_parked();
        assert_eq!(count(cx).unwrap(), both - 1);
        let center = cx.debug_bounds("search-regex").unwrap().center();
        click(cx, center, 1);
        release(cx, center);
        cx.simulate_input("(");
        cx.run_until_parked();
        assert!(count(cx).is_err());
    }

    #[gpui::test]
    fn search_arrows_walk_matches(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        print_line(&ws, cx, "hit_a hit_b");
        run_actions(&ws, cx, r#""search""#);
        let bar = search_bar(&ws, cx).expect("palette action opens the bar");
        cx.simulate_input("hit_");
        cx.run_until_parked();
        let active = |cx: &mut VisualTestContext| {
            bar.read_with(cx, |bar, _| bar.status().clone())
                .unwrap()
                .active
        };
        let start = active(cx);
        let center = cx.debug_bounds("search-prev").unwrap().center();
        click(cx, center, 1);
        release(cx, center);
        cx.run_until_parked();
        let moved = active(cx);
        assert_ne!(moved, start);
        let center = cx.debug_bounds("search-next").unwrap().center();
        click(cx, center, 1);
        release(cx, center);
        cx.run_until_parked();
        assert_eq!(active(cx), start);
        cx.simulate_keystrokes("up");
        cx.run_until_parked();
        assert_eq!(active(cx), moved);
        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        assert_eq!(active(cx), start);
    }

    fn active_view(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<TerminalView> {
        ws.update(cx, |ws, _| {
            let pane = ws.focused_pane();
            pane.tabs[pane.active].view.clone()
        })
    }

    #[gpui::test]
    fn ctrl_hover_underlines_and_ctrl_click_runs_the_link(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"links": {"modifier": "ctrl", "patterns": [{"regex": "ticket-[0-9]+", "action": "copy"}]}}"#,
        )
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        print_line(&ws, cx, "see ticket-42 now");
        let position = cell_of(&ws, cx, "see ticket-42 now", 6, false);
        let view = active_view(&ws, cx);
        let ctrl = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        // without the modifier nothing is a link
        cx.simulate_event(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Modifiers::default(),
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.hovered_link().is_none()));
        cx.simulate_event(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: ctrl,
        });
        cx.run_until_parked();
        let target = view.read_with(cx, |view, _| view.hovered_link().map(|l| l.target.clone()));
        assert_eq!(target.as_deref(), Some("ticket-42"));
        // letting go of the modifier drops the underline
        cx.simulate_modifiers_change(Modifiers::default());
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.hovered_link().is_none()));

        cx.simulate_event(MouseDownEvent {
            position,
            modifiers: ctrl,
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            position,
            modifiers: ctrl,
            button: MouseButton::Left,
            click_count: 1,
        });
        cx.run_until_parked();
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("ticket-42".into())
        );
        // a link click starts no selection
        let terminal = terminal(&ws, cx, 0);
        assert!(terminal.read_with(cx, |t, _| t.selection_text().is_none()));
    }

    #[gpui::test]
    fn hints_label_matches_and_typing_a_label_runs_it(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"hints": {"alphabet": "fj", "patterns": [{"name": "num", "regex": "[0-9]{4}", "action": "copy"}]}}"#,
        )
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        print_line(&ws, cx, "codes 1111 2222");
        cx.simulate_keystrokes("ctrl-shift-e");
        cx.run_until_parked();
        let view = active_view(&ws, cx);
        let labels = view.read_with(cx, |view, _| {
            view.hints()
                .map(|hints| {
                    hints
                        .shown()
                        .map(|(link, label)| (link.target.clone(), label.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        });
        // the typed command line holds the numbers too, so only the printed ones are checked
        let label = labels
            .iter()
            .rev()
            .find(|(target, _)| target == "2222")
            .map(|(_, label)| label.clone())
            .expect("2222 is labeled");
        cx.simulate_keystrokes(
            &label
                .chars()
                .map(String::from)
                .collect::<Vec<_>>()
                .join(" "),
        );
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.hints().is_none()));
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("2222".into())
        );

        // escape leaves hints mode without running anything
        run_actions(&ws, cx, r#""hints""#);
        assert!(view.read_with(cx, |view, _| view.hints().is_some()));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.hints().is_none()));
    }

    #[gpui::test]
    fn vi_mode_key_takes_keys_until_escape(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        print_line(&ws, cx, "pick me please");
        let terminal = terminal(&ws, cx, 0);
        cx.simulate_keystrokes("ctrl-shift-space");
        cx.run_until_parked();
        assert!(terminal.read_with(cx, |t, _| t.vi_mode()));
        // keys move the vi cursor, the shell gets nothing
        cx.simulate_keystrokes("k k v e y");
        cx.run_until_parked();
        assert!(terminal.read_with(cx, |t, _| !t.vi_mode()));
        assert!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .is_some()
        );

        run_actions(&ws, cx, r#""vi_mode""#);
        assert!(terminal.read_with(cx, |t, _| t.vi_mode()));
        // "/" opens the search bar
        cx.simulate_keystrokes("/");
        cx.run_until_parked();
        assert!(search_bar(&ws, cx).is_some());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(terminal.read_with(cx, |t, _| !t.vi_mode()));
    }

    fn leaf(id: usize) -> LayoutNode {
        LayoutNode::Leaf(Pane {
            id,
            tabs: Vec::new(),
            active: 0,
            tab_scroll: ScrollHandle::new(),
        })
    }

    fn split(axis: SplitAxis, ratio: f32, first: LayoutNode, second: LayoutNode) -> LayoutNode {
        LayoutNode::Split {
            axis,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// 0 on the left, 1 above 2 on the right
    fn three_panes() -> LayoutNode {
        split(
            SplitAxis::Horizontal,
            0.5,
            leaf(0),
            split(SplitAxis::Vertical, 0.5, leaf(1), leaf(2)),
        )
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

    #[gpui::test]
    fn program_and_long_command_notifications_follow_settings(cx: &mut TestAppContext) {
        let settings = Settings::parse(
            r#"{"notifications": {"timeout": 0, "from_programs": "always"}, "shell_integration": {"notify_after": 5}}"#,
        )
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 2, "{}", settings);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let notify = Event::Notify {
            title: "build".into(),
            body: "done".into(),
        };
        terminal(&ws, cx, 1).update(cx, |_, cx| cx.emit(notify.clone()));
        cx.run_until_parked();
        assert_eq!(notifications(&ws, cx), ["build: done"]);

        // a short command, or one in the tab in front, tells nothing
        let finished = |seconds: u64| Event::CommandFinished {
            duration: Duration::from_secs(seconds),
            exit: Some(1),
        };
        terminal(&ws, cx, 0).update(cx, |_, cx| cx.emit(finished(2)));
        terminal(&ws, cx, 1).update(cx, |_, cx| cx.emit(finished(60)));
        cx.run_until_parked();
        assert_eq!(notifications(&ws, cx).len(), 1);
        // a long one in a background tab does
        terminal(&ws, cx, 0).update(cx, |_, cx| cx.emit(finished(60)));
        cx.run_until_parked();
        assert_eq!(
            notifications(&ws, cx),
            ["build: done", "command failed with 1 in tab 1"]
        );

        // "unfocused" leaves out notifications of the tab in front
        let mut settings = cx.update(|_, cx| Settings::get(cx).clone());
        settings.notifications.from_programs = crate::settings::ProgramNotifications::Unfocused;
        cx.update(|_, cx| cx.set_global(settings));
        terminal(&ws, cx, 1).update(cx, |_, cx| cx.emit(notify.clone()));
        cx.run_until_parked();
        assert_eq!(notifications(&ws, cx).len(), 2);
    }

    #[gpui::test]
    fn keymap_sends_text_chords_and_unbinds(cx: &mut TestAppContext) {
        let keys = r#"{"keymap": [
            {"keys": "ctrl-a c", "action": "new_tab"},
            {"keys": "ctrl-shift-k", "action": {"send_text": "sent"}, "when": "terminal"},
            {"keys": "ctrl-shift-w", "action": "unbind"},
            {"keys": "ctrl-shift-z", "action": "new_tab", "when": "vi_mode"},
        ]}"#;
        let full = crate::settings::config_combine(serde_json_lenient::from_str(keys).unwrap());
        let keymap = Keybindings::from_full(&full).unwrap();
        let (ws, cx) = open(cx, 1);
        cx.update(|_, cx| cx.bind_keys(keymap.bindings()));
        // a chord: ctrl-a, then c
        cx.simulate_keystrokes("ctrl-a c");
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 2);
        // unbound, so the tab stays
        cx.simulate_keystrokes("ctrl-shift-w");
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 2);
        // only in vi mode
        cx.simulate_keystrokes("ctrl-shift-z");
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 2);
        run_actions(&ws, cx, r#""vi_mode""#);
        cx.simulate_keystrokes("ctrl-shift-z");
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 3);

        let reading = reader(&ws, cx, false, "keymap_send");
        cx.simulate_keystrokes("ctrl-shift-k");
        pump(cx);
        let got = finish(&ws, cx, reading).concat();
        assert!(got.starts_with("73656e74"), "{got}");
    }

    #[gpui::test]
    fn open_scrollback_runs_the_pager_on_the_history(cx: &mut TestAppContext) {
        let copy = std::env::temp_dir().join(format!("kuterm_pager_{}", std::process::id()));
        let _ = std::fs::remove_file(&copy);
        let pager = format!(r#"cp "$0" {}.tmp; mv {0}.tmp {0}; sleep 5"#, copy.display());
        let settings = Settings::parse(&format!(
            r#"{{"scrollback_pager": ["sh", "-c", {pager:?}]}}"#
        ))
        .unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        print_line(&ws, cx, "remember this line");
        run_actions(&ws, cx, r#""open_scrollback""#);
        assert_eq!(views(&ws, cx).len(), 2);
        wait_until(cx, "pager never got the file", |_| copy.exists());
        let text = std::fs::read_to_string(&copy).unwrap();
        assert!(text.contains("remember this line"), "{text:?}");
        std::fs::remove_file(&copy).ok();
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
    fn remote_requests_open_tabs_type_and_list(cx: &mut TestAppContext) {
        use crate::remote::Request;
        use crate::ui::windows::handle_request;

        let (ws, cx) = open(cx, 1);
        cx.update(|window, _| window.activate_window());
        let reply = cx.cx.update(|cx| {
            handle_request(
                Request::NewTab {
                    cwd: Some(std::env::temp_dir()),
                    profile: None,
                },
                cx,
            )
        });
        assert!(reply.ok, "{reply:?}");
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 2);
        let reply = cx.cx.update(|cx| {
            handle_request(
                Request::NewTab {
                    cwd: None,
                    profile: Some("missing".into()),
                },
                cx,
            )
        });
        assert!(!reply.ok);

        let reply = cx.cx.update(|cx| handle_request(Request::ListTabs, cx));
        assert_eq!(reply.tabs.len(), 2);
        assert!(reply.tabs[1].active && !reply.tabs[0].active);
        assert_eq!(reply.tabs[1].number, 2);

        let reply = cx.cx.update(|cx| {
            handle_request(
                Request::SendText {
                    text: "echo remote_$((2*21))\r".into(),
                },
                cx,
            )
        });
        assert!(reply.ok);
        let terminal = terminal(&ws, cx, 1);
        wait_until(cx, "sent text never ran", |cx| {
            terminal.read_with(cx, |t, _| line_starting_with(t, "remote_42").is_some())
        });
    }
}
