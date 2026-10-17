//! root view: owns tabs and handles new tab / close tab.

mod actions;
mod app_menu;
mod attention;
mod broadcast;
mod closing;
mod command_inputs;
mod command_queue;
mod config_watch;
mod editor_links;
mod end_truncated;
mod find_all;
#[cfg(target_os = "linux")]
mod global_menu;
mod groups;
mod layout;
mod menu_bar;
mod notifications;
mod overlays;
mod palette;
mod panes_view;
mod private_tabs;
mod remote_verbs;
mod render;
mod run_command;
mod scroll_sync;
mod session;
mod source_marks;
mod status_bar;
mod system_notifications;
mod tab_bar;
mod tab_icon;
mod tab_menu;
mod tab_title;
mod tabs;
mod tear_off;
#[cfg(test)]
mod test_support;
mod theme_picker;
mod title_refresh;
mod title_sources;
mod waiting;
mod window;

use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{
    FutureExt, StreamExt,
    channel::mpsc::{UnboundedSender, unbounded},
};
use gpui::{
    Action, App, Bounds, ClipboardItem, Context, DismissEvent, Entity, EntityId, FocusHandle,
    Focusable, ManagedView, Pixels, Point, ScrollHandle, Size, Subscription, Task, Window,
    WindowAppearance, actions, prelude::*,
};

pub use app_menu::{RunLink, app_menus, dock_menu, open_urls, parse_run_link};
use closing::*;
pub use config_watch::watch_config;
#[cfg(target_os = "linux")]
pub use global_menu::start as start_global_menu;
use layout::*;
use menu_bar::{MenuBar, MenuPicked};
use notifications::Notification;
pub use remote_verbs::PaneMatch;
use tab_title::{Click, Piece, TitleInputs};
use tabs::*;
pub use tear_off::MovedTab;
use title_refresh::*;
pub use window::{background_appearance, startup_bounds, window_decorations};

use crate::{
    automation::events::{BusEvent, EventBus},
    cli::Cli,
    settings::{
        Command, CommandAction, CommandStep, Commands, Config, ConfigFiles, ConfirmQuit, Profile,
        RestoreSession, Settings, Shell, Side, StatusBarState, TabIconSettings, TabIcons,
        TabTitleBlock, Target, Values,
    },
    terminal::{
        Event, InputSource, PaneContext, ProcessSampler, PtyState, Terminal, TerminalBounds,
        TerminalBuilder, TreeSample, foreground_process, pty_modes, stopped_jobs,
        with_shell_integration,
    },
    theme::{self, Theme},
    ui::{
        dialogs::{
            About, CloseConfirmed, CloseTarget, CommandPalette, ConfirmClose, ConfirmPaste,
            InputPrompt, PasteConfirmed,
        },
        terminal_element::ShowHidden,
        terminal_view::{TerminalView, TypedInput, ViewEvent, copied_text},
        text_input::TextInput,
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

/// run the command of this name, keymap entries asking for inputs like the palette does
#[derive(Clone, PartialEq, Action)]
#[action(namespace = workspace, no_json)]
pub struct RunNamedCommand(pub String);

struct Tab {
    view: Entity<TerminalView>,
    /// name of the profile it was opened with
    profile: String,
    /// built from `tab_title` blocks, empty until the first refresh
    title: String,
    icon: String,
    /// byte offsets in `title` the icon is drawn at, one per "icon" block
    icon_at: Vec<usize>,
    /// byte ranges of ports in `title`, opened with the link modifier and a click
    ports_at: Vec<(Range<usize>, u16)>,
    profile_icon: Option<String>,
    /// true once the shell is done starting, so writing to it now is not echoed by the tty
    ready: bool,
    /// fires after a quiet gap and releases input queued for a still starting shell,
    /// replacing it on newer output drops the old task and restarts the gap
    ready_task: Task<()>,
    /// input a command queued before the shell was ready, and what wrote it last
    pending_input: Vec<u8>,
    pending_source: InputSource,
    /// the bell rang while the tab was in the background, cleared when it is shown
    bell: bool,
    /// file only this tab uses, like the history its pager shows, removed with the tab
    temp_file: Option<PathBuf>,
    /// title typed with "rename_tab", shown in place of the `tab_title` blocks
    custom_title: Option<String>,
    /// kept small at the front of its pane, with no close button
    pinned: bool,
    /// an ai agent ran in front of the shell on the last title refresh
    agent: bool,
    /// the agent stopped working and waits for the user
    attention: attention::Attention,
    /// group the tab is in
    group: Option<usize>,
    /// git project of the tab on the last title refresh, while grouped by project
    project: Option<PathBuf>,
    /// context and program in front on the last title refresh, changes go on the bus. the
    /// context is only read while the tab is shown
    context: PaneContext,
    process: String,
    /// the question line a program waited on at the last refresh, and whether it was told,
    /// so it is told once
    asked: Option<(String, bool)>,
    /// id of its terminal for remote control
    pane_id: u64,
    /// name given with "rename_pane" or `kuterm msg new-tab --name`, `--pane` finds it by it
    name: Option<String>,
    /// color the tab is drawn in, set with `kuterm msg set-tab-color`
    color: Option<gpui::Hsla>,
    /// remote runs need no asking in this tab, the user allowed them all
    remote_allowed: bool,
    _subscriptions: [Subscription; 3],
}

impl Drop for Tab {
    fn drop(&mut self) {
        if let Some(path) = &self.temp_file {
            std::fs::remove_file(path).ok();
        }
        EventBus::publish(BusEvent::PaneClosed { pane: self.pane_id });
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

pub struct Workspace {
    layout: LayoutNode,
    /// pane with focus, its active tab is the active tab of the window
    focused: usize,
    /// pane shown alone over the others, none shows every pane
    zoomed: Option<usize>,
    next_pane_id: usize,
    /// pane edge a dragged tab would split at, none while no tab drags over a pane
    drop_zone: Option<(usize, DropSide)>,
    /// pane, tab and part of the tab a dragged tab is over, none while no tab drags over a tab
    tab_drop: Option<(usize, usize, groups::TabDrop)>,
    /// built from `window_title` blocks for the active tab
    window_title: String,
    /// left and right side of the status bar for the active tab, empty while it is off
    status: [Vec<Piece>; 2],
    /// frame busy progress spinners show
    spinner_frame: usize,
    /// a task turns the spinners while some title has one
    spinning: bool,
    /// pane and place the profile menu was opened at, none while it is closed
    profile_menu: Option<(usize, Point<Pixels>)>,
    /// tab and place its right click menu was opened at, none while it is closed
    tab_menu: Option<(EntityId, Point<Pixels>)>,
    /// held by the open tab menu, so keys go to it and not to the shell under it
    tab_menu_focus: FocusHandle,
    /// tab being renamed and the field its title is typed into
    renaming: Option<(EntityId, Entity<TextInput>)>,
    /// columns and rows of the active tab the titles were last built with
    titled_size: (usize, usize),
    /// pane and index of the tab being dragged, so a drop outside the window knows it
    tab_drag: Option<(usize, usize)>,
    /// open command palette, none while it is closed
    palette: Option<Entity<CommandPalette>>,
    /// open about page, none while it is closed
    about: Option<Entity<About>>,
    /// open close tab dialog, none while it is closed
    confirm_close: Option<Entity<ConfirmClose>>,
    /// open paste dialog, none while it is closed
    confirm_paste: Option<Entity<ConfirmPaste>>,
    /// open field asking for one value of a command, none while it is closed
    command_input: Option<Entity<InputPrompt>>,
    /// command picked whose inputs are still being asked for
    pending: Option<command_inputs::PendingCommand>,
    /// menus at the top of the window, drawn only when `window.menu_bar` shows them
    menu_bar: Entity<MenuBar>,
    _menu_bar_subscriptions: [Subscription; 2],
    _overlay_subscriptions: Vec<Subscription>,
    /// panes typed into together, none while input goes to the focused pane alone
    broadcast: Option<std::collections::BTreeSet<usize>>,
    /// panes scrolled together, the active tab of each follows the others
    scroll_sync: std::collections::BTreeSet<usize>,
    /// patterns of questions programs wait on, compiled when they change
    question_patterns: waiting::QuestionPatterns,
    /// tab groups of the window
    groups: Vec<groups::Group>,
    next_group_id: usize,
    /// the rename field sets the pane name, not the title
    renaming_name: bool,
    /// group being renamed and the field its name is typed into
    renaming_group: Option<(usize, Entity<TextInput>)>,
    /// group and place its right click menu was opened at, none while it is closed
    group_menu: Option<(usize, Point<Pixels>)>,
    /// matches listed by the open find in all tabs palette
    found: Vec<find_all::Found>,
    /// search of the last query typed into find in all tabs
    _find_task: Task<()>,
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
        let mut this = Self::empty(window, cx);
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

    /// create a workspace holding a tab moved out of another window
    pub fn with_moved_tab(moved: MovedTab, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::empty(window, cx);
        this.adopt_tab(this.focused, moved, window, cx);
        this
    }

    /// workspace with one empty pane, set up for its window
    fn empty(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (refresh_titles, title_task) = spawn_title_refresh(window, cx);
        let menu_bar = cx.new(MenuBar::new);
        let _menu_bar_subscriptions = [
            cx.subscribe_in(
                &menu_bar,
                window,
                |this, bar, _: &DismissEvent, window, cx| {
                    // a click elsewhere may have focused another pane already
                    if bar.focus_handle(cx).is_focused(window) {
                        this.focus_active_tab(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &menu_bar,
                window,
                |this, _, picked: &MenuPicked, window, cx| {
                    this.run_menu_action(picked.0.boxed_clone(), window, cx);
                },
            ),
        ];
        let this = Self {
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
            tab_drop: None,
            window_title: String::new(),
            status: Default::default(),
            spinner_frame: 0,
            spinning: false,
            profile_menu: None,
            tab_menu: None,
            tab_menu_focus: cx.focus_handle(),
            renaming: None,
            titled_size: (0, 0),
            tab_drag: None,
            palette: None,
            about: None,
            confirm_close: None,
            confirm_paste: None,
            command_input: None,
            pending: None,
            menu_bar,
            _menu_bar_subscriptions,
            _overlay_subscriptions: Vec::new(),
            broadcast: None,
            scroll_sync: Default::default(),
            question_patterns: Default::default(),
            groups: Vec::new(),
            next_group_id: 0,
            renaming_group: None,
            renaming_name: false,
            group_menu: None,
            found: Vec::new(),
            _find_task: Task::ready(()),
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
        #[cfg(target_os = "linux")]
        global_menu::export_window(window, cx);
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

    /// terminal of the active tab of pane `pane_id`
    fn active_terminal_of(&self, pane_id: usize, cx: &App) -> Option<Entity<Terminal>> {
        let pane = self.pane(pane_id)?;
        Some(pane.tabs.get(pane.active)?.view.read(cx).terminal().clone())
    }

    /// active terminals of the other panes in `joined`, when `view` is the active tab of one
    /// of them, for broadcast input and synced scroll
    fn joined_terminals(
        &self,
        joined: &std::collections::BTreeSet<usize>,
        view: &Entity<TerminalView>,
        cx: &App,
    ) -> Vec<Entity<Terminal>> {
        let Some((source, ix)) = self.find_view(view) else {
            return Vec::new();
        };
        if !joined.contains(&source) || self.pane(source).is_none_or(|pane| pane.active != ix) {
            return Vec::new();
        }
        joined
            .iter()
            .filter(|&&pane| pane != source)
            .filter_map(|&pane| self.active_terminal_of(pane, cx))
            .collect()
    }

    /// terminal of the tab at `ix` in pane `pane_id`
    fn terminal_at(&self, pane_id: usize, ix: usize, cx: &App) -> Entity<Terminal> {
        self.view_at(pane_id, ix).read(cx).terminal().clone()
    }

    /// every tab with its title and folder, for remote control
    pub fn tab_infos(&self, window_id: u64, cx: &App) -> Vec<crate::automation::TabInfo> {
        self.layout
            .panes()
            .into_iter()
            .flat_map(|pane| {
                (0..pane.tabs.len()).map(move |ix| crate::automation::TabInfo {
                    id: pane.tabs[ix].pane_id,
                    name: pane.tabs[ix].name.clone(),
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
}

/// run `f` on the workspace, bringing its window to the front first. app wide actions use
/// it, since with the window minimized or the dock menu in use no window is active. none
/// when no window is open
pub fn with_workspace<R>(
    cx: &mut App,
    f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) -> R,
) -> Option<R> {
    // the window in front, else any
    let handle = cx
        .active_window()
        .and_then(|window| window.downcast::<Workspace>())
        .or_else(|| {
            cx.windows()
                .into_iter()
                .find_map(|window| window.downcast::<Workspace>())
        })?;
    cx.activate(true);
    handle
        .update(cx, |workspace, window, cx| {
            window.activate_window();
            f(workspace, window, cx)
        })
        .ok()
}

/// folder the shell of `tab` is in, the one it reported first, none when it can't be told
fn tab_cwd(tab: &Tab, cx: &App) -> Option<PathBuf> {
    let terminal = tab.view.read(cx).terminal().read(cx);
    if let Some(cwd) = terminal.reported_cwd() {
        return Some(cwd.to_path_buf());
    }
    crate::terminal::process_info(terminal.shell_pid)?.cwd
}

#[cfg(test)]
mod tests {
    use crate::ui::workspace::test_support::*;

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

    #[test]
    fn equalizing_counts_only_the_panes_side_by_side_along_a_split() {
        let ratio = |node: &LayoutNode| match node {
            LayoutNode::Split { ratio, .. } => *ratio,
            _ => panic!("not a split"),
        };
        // the stacked panes on the right are one column, so the columns share evenly
        let mut layout = split(
            SplitAxis::Horizontal,
            0.2,
            leaf(0),
            split(SplitAxis::Vertical, 0.5, leaf(1), leaf(2)),
        );
        layout.equalize();
        assert!((ratio(&layout) - 0.5).abs() < 1e-6);
        // a stack on the left beside two columns on the right takes a third
        let mut layout = split(
            SplitAxis::Horizontal,
            0.5,
            split(SplitAxis::Vertical, 0.5, leaf(0), leaf(1)),
            split(SplitAxis::Horizontal, 0.5, leaf(2), leaf(3)),
        );
        layout.equalize();
        assert!((ratio(&layout) - 1. / 3.).abs() < 1e-6);
    }

    #[gpui::test]
    fn moving_focus_out_of_a_zoomed_pane_ends_the_zoom(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 2);
        run_actions(&ws, cx, r#""split_right", "zoom_pane""#);
        let right = ws.update(cx, |ws, _| ws.focused);
        assert_eq!(ws.update(cx, |ws, _| ws.zoomed), Some(right));
        // focusing the zoomed pane again keeps it zoomed
        ws.update_in(cx, |ws, window, cx| ws.focus_pane(right, window, cx));
        assert_eq!(ws.update(cx, |ws, _| ws.zoomed), Some(right));
        run_actions(&ws, cx, r#""focus_pane_left""#);
        assert_ne!(ws.update(cx, |ws, _| ws.focused), right);
        assert_eq!(ws.update(cx, |ws, _| ws.zoomed), None);
    }

    #[gpui::test]
    fn a_pager_tab_takes_its_history_file_along_when_it_closes(cx: &mut TestAppContext) {
        let settings =
            Settings::parse(r#"{"scrollback_pager": ["sh", "-c", "sleep 30"]}"#).unwrap();
        let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
        print_line(&ws, cx, "kept for the pager");
        run_actions(&ws, cx, r#""open_scrollback""#);
        let file = ws.update(cx, |ws, _| ws.focused_pane().tabs[1].temp_file.clone());
        let file = file.expect("the pager tab has no history file");
        assert!(file.exists());
        ws.update_in(cx, |ws, window, cx| {
            ws.close_tab_at(ws.focused, 1, window, cx)
        });
        cx.run_until_parked();
        assert_eq!(views(&ws, cx).len(), 1);
        assert!(!file.exists(), "{} stayed", file.display());
    }
}
