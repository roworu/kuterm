//! what the workspace tests share: opening a workspace over real shells, reading its tabs,
//! typing and clicking

pub(crate) use std::{path::PathBuf, time::Instant};

pub(crate) use alacritty_terminal::term::TermMode;
pub(crate) use gpui::{
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    TestAppContext, VisualTestContext, WindowHandle, point,
};

pub(crate) use super::*;
pub(crate) use crate::{
    settings::{Keybindings, MemoryUnit, Pins},
    terminal::{Terminal, TerminalBounds},
    theme::Theme,
};

/// open a workspace with `tabs` tabs and user keybindings `keys`, the last tab is active
pub(crate) fn open_with<'a>(
    cx: &'a mut TestAppContext,
    tabs: usize,
    keys: &str,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    open_with_settings(cx, tabs, keys, Settings::default())
}

pub(crate) fn open_with_settings<'a>(
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

pub(crate) fn open(
    cx: &mut TestAppContext,
    tabs: usize,
) -> (Entity<Workspace>, &mut VisualTestContext) {
    open_with(cx, tabs, "{}")
}

pub(crate) fn views(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<EntityId> {
    ws.update(cx, |ws, _| {
        ws.focused_pane()
            .tabs
            .iter()
            .map(|t| t.view.entity_id())
            .collect()
    })
}

/// active tab index, checking that focus is on it
pub(crate) fn current(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> usize {
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

pub(crate) fn next(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> usize {
    ws.update_in(cx, |ws, window, cx| ws.next_tab(&NextTab, window, cx));
    cx.run_until_parked();
    current(ws, cx)
}

pub(crate) fn activate(ws: &Entity<Workspace>, cx: &mut VisualTestContext, ix: usize) -> usize {
    ws.update_in(cx, |ws, window, cx| {
        ws.activate_tab_action(&ActivateTab(ix), window, cx)
    });
    cx.run_until_parked();
    current(ws, cx)
}

pub(crate) fn keys(ws: &Entity<Workspace>, cx: &mut VisualTestContext, keystrokes: &str) -> usize {
    cx.run_until_parked();
    cx.simulate_keystrokes(keystrokes);
    cx.run_until_parked();
    current(ws, cx)
}

/// press on one tab, move to another and release, like a mouse drag reorder
pub(crate) fn drag_tab(cx: &mut VisualTestContext, from: &'static str, to: &'static str) {
    let start = cx.debug_bounds(from).unwrap().center();
    let end = cx.debug_bounds(to).unwrap().center();
    drag_to(cx, start, end);
}

/// press at `start`, cross the drag threshold, move to `end` and release
pub(crate) fn drag_to(cx: &mut VisualTestContext, start: Point<Pixels>, end: Point<Pixels>) {
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

/// split the focused pane at `side`, moving its first tab into the new pane
pub(crate) fn split_pane(
    ws: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    side: DropSide,
) -> usize {
    ws.update_in(cx, |ws, window, cx| {
        let target = ws.focused;
        ws.split_with_tab(target, 0, target, side, window, cx);
    });
    cx.run_until_parked();
    ws.update(cx, |ws, _| ws.focused)
}

// raw input bytes a focus reader collects before it ends
pub(crate) const READ: usize = 30;

/// let the pty threads run, then move the fake clock so batched alacritty events flow
pub(crate) fn pump(cx: &mut VisualTestContext) {
    std::thread::sleep(Duration::from_millis(15));
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
}

pub(crate) fn wait_until(
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
pub(crate) fn set_window_active(cx: &mut VisualTestContext, active: bool) {
    if active {
        cx.update(|window, _| window.activate_window());
    } else {
        cx.deactivate_window();
    }
    cx.run_until_parked();
    pump(cx);
}

pub(crate) fn bells(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<bool> {
    ws.update(cx, |ws, _| {
        ws.focused_pane().tabs.iter().map(|t| t.bell).collect()
    })
}

pub(crate) fn terminal(
    ws: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    ix: usize,
) -> Entity<Terminal> {
    ws.update(cx, |ws, cx| {
        ws.focused_pane().tabs[ix].view.read(cx).terminal().clone()
    })
}

/// in the active tab: optionally enable focus reporting (mode 1004), then save READ raw
/// input bytes as hex into the returned file
pub(crate) fn reader(
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
pub(crate) fn reader_with(
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
    terminal.update(cx, |t, _| {
        t.input(InputSource::Keyboard, command.into_bytes())
    });
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

pub(crate) fn send_to(ws: &Entity<Workspace>, cx: &mut VisualTestContext, ix: usize, text: &str) {
    let bytes = text.as_bytes().to_vec();
    terminal(ws, cx, ix).update(cx, |t, _| t.input(InputSource::Keyboard, bytes));
    pump(cx);
}

/// fill the reader up and return what it got before: "I", "O" or other bytes as hex, with
/// repeated identical reports merged
pub(crate) fn finish(
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

/// screen line that starts with `text`
pub(crate) fn line_starting_with(terminal: &Terminal, text: &str) -> Option<usize> {
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
pub(crate) fn cell_of(
    ws: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    text: &str,
    column: usize,
    right: bool,
) -> Point<Pixels> {
    let x = column as f32 + if right { 0.8 } else { 0.2 };
    let ix = current(ws, cx);
    terminal(ws, cx, ix).read_with(cx, |t, _| {
        let line =
            line_starting_with(t, text).unwrap_or_else(|| panic!("no line starts with {text:?}"));
        let b = t.last_content.terminal_bounds;
        point(
            b.bounds.origin.x + b.cell_width * x,
            b.bounds.origin.y + b.line_height * (line as f32 + 0.5),
        )
    })
}

pub(crate) fn print_line(ws: &Entity<Workspace>, cx: &mut VisualTestContext, text: &str) {
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

pub(crate) fn click(cx: &mut VisualTestContext, position: Point<Pixels>, click_count: usize) {
    cx.simulate_event(MouseDownEvent {
        position,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count,
        first_mouse: false,
    });
}

pub(crate) fn release(cx: &mut VisualTestContext, position: Point<Pixels>) {
    cx.simulate_event(MouseUpEvent {
        position,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 1,
    });
}

pub(crate) fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|item| item.text())
}

/// raw reader in the active tab of a program that asked for sgr mouse reports
pub(crate) fn mouse_reader(
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
pub(crate) fn cell_center(
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

pub(crate) fn hex(bytes: &str) -> Vec<String> {
    bytes.bytes().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn tab_icon_and_title(
    ws: &Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> (String, String) {
    ws.update(cx, |ws, _| {
        let tab = &ws.focused_pane().tabs[0];
        (tab.icon.clone(), tab.title.clone())
    })
}

pub(crate) fn icon_while_sleeping(cx: &mut TestAppContext, json: &str) -> (String, String) {
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

/// workspace with the tab bar always shown and these profiles
pub(crate) fn open_profiles<'a>(
    cx: &'a mut TestAppContext,
    profiles: &str,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let json = format!(r#"{{"hide_bar_for_one_tab": false, "profiles": [{profiles}]}}"#);
    let (ws, cx) = open_with_settings(cx, 1, "{}", Settings::parse(&json).unwrap());
    cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(600.)));
    cx.run_until_parked();
    (ws, cx)
}

pub(crate) fn right_click(cx: &mut VisualTestContext, position: Point<Pixels>) {
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

pub(crate) fn menu_open(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
    ws.update(cx, |ws, _| ws.profile_menu.is_some())
}

pub(crate) fn scroll_offset(terminal: &Entity<Terminal>, cx: &mut VisualTestContext) -> usize {
    terminal.read_with(cx, |t, _| t.last_content.display_offset)
}

// click on the scrollbar with the given smooth scroll settings, then draw one frame
pub(crate) fn click_scrollbar<'a>(
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

pub(crate) fn palette_open(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
    ws.update(cx, |ws, _| ws.palette.is_some())
}

pub(crate) fn palette_focused(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
    ws.update_in(cx, |ws, window, cx| {
        ws.palette
            .as_ref()
            .is_some_and(|palette| palette.focus_handle(cx).is_focused(window))
    })
}

pub(crate) fn palette_names(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<String> {
    ws.update(cx, |ws, cx| {
        let palette = ws.palette.as_ref().unwrap().read(cx);
        palette.matches().map(|command| command.label()).collect()
    })
}

pub(crate) fn palette_query(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> String {
    ws.update(cx, |ws, cx| {
        ws.palette.as_ref().unwrap().read(cx).query(cx).to_string()
    })
}

pub(crate) fn type_keys(cx: &mut VisualTestContext, keystrokes: &str) {
    cx.run_until_parked();
    cx.simulate_keystrokes(keystrokes);
    cx.run_until_parked();
}

pub(crate) fn set_commands(cx: &mut VisualTestContext, json: &str) {
    let commands = Commands::parse(json).unwrap();
    cx.update(|_, cx| cx.set_global(commands));
}

/// run one command built from a json `actions` array against the workspace
pub(crate) fn run_actions(ws: &Entity<Workspace>, cx: &mut VisualTestContext, actions: &str) {
    let json = format!(r#"{{"commands": [{{"name": "x", "actions": [{actions}]}}]}}"#);
    let command = Commands::parse(&json).unwrap().commands.remove(0);
    ws.update_in(cx, |ws, window, cx| ws.run_command(&command, window, cx));
    cx.run_until_parked();
}

pub(crate) fn tab_center(cx: &mut VisualTestContext, ix: usize) -> Point<Pixels> {
    cx.debug_bounds(format!("tab-title-{ix}").leak())
        .expect("no such tab")
        .center()
}

pub(crate) fn custom_title(
    ws: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    ix: usize,
) -> Option<String> {
    ws.update(cx, |ws, _| ws.focused_pane().tabs[ix].custom_title.clone())
}

/// workspaces of every open window, in the order gpui lists them
pub(crate) fn workspaces(cx: &mut VisualTestContext) -> Vec<WindowHandle<Workspace>> {
    cx.cx.update(|cx| {
        cx.windows()
            .into_iter()
            .filter_map(|handle| handle.downcast::<Workspace>())
            .collect()
    })
}

/// terminals of the focused pane of a window's workspace
pub(crate) fn terminals_of(
    handle: WindowHandle<Workspace>,
    cx: &mut VisualTestContext,
) -> Vec<EntityId> {
    cx.cx.update(|cx| {
        handle
            .read_with(cx, |ws, cx| {
                ws.focused_pane()
                    .tabs
                    .iter()
                    .map(|tab| tab.view.read(cx).terminal().entity_id())
                    .collect()
            })
            .unwrap_or_default()
    })
}

pub(crate) fn open_with_menu_bar<'a>(
    cx: &'a mut TestAppContext,
    mode: &str,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let json = format!(r#"{{"window": {{"menu_bar": "{mode}"}}}}"#);
    open_with_settings(cx, 1, "{}", Settings::parse(&json).unwrap())
}

pub(crate) fn menu_bar_open(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
    ws.update_in(cx, |ws, window, cx| {
        ws.menu_bar.focus_handle(cx).is_focused(window)
    }) || cx.debug_bounds("menu-dropdown").is_some()
}

/// workspace with these tab title and status bar blocks
pub(crate) fn open_with_status<'a>(
    cx: &'a mut TestAppContext,
    tab_title: &str,
    left: &str,
    right: &str,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let json = format!(
        r#"{{"tab_title": {tab_title}, "status_bar": {{"enable": true, "left": {left},
            "right": {right}}}}}"#
    );
    open_with_settings(cx, 1, "{}", Settings::parse(&json).unwrap())
}

/// port the first status bar side shows, once the sampler found one
pub(crate) fn status_port(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> u16 {
    let port = |cx: &mut VisualTestContext| {
        ws.update(cx, |ws, _| {
            ws.status[0].iter().find_map(|piece| match piece.click {
                Some(Click::Port(port)) => Some(port),
                _ => None,
            })
        })
    };
    wait_until(cx, "port never showed in the status bar", |cx| {
        port(cx).is_some()
    });
    port(cx).unwrap()
}

pub(crate) fn error_texts(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<String> {
    ws.read_with(cx, |ws, _| {
        ws.notifications
            .iter()
            .filter(|notification| notification.error)
            .map(|notification| notification.text.clone())
            .collect()
    })
}

pub(crate) fn recent_test_commands(cx: &mut VisualTestContext) {
    set_commands(
        cx,
        r#"{"commands": [
            {"name": "first", "actions": ["new_tab"]},
            {"name": "second", "actions": ["new_tab"]},
            {"name": "third", "actions": ["new_tab"]},
        ]}"#,
    );
}

pub(crate) fn notifications(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<String> {
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

/// start a long program in the active tab and wait until it runs
pub(crate) fn start_program(ws: &Entity<Workspace>, cx: &mut VisualTestContext) {
    ws.update(cx, |ws, cx| {
        let pane = ws.focused;
        let ix = ws.focused_pane().active;
        ws.input_to(pane, ix, InputSource::Keyboard, b"sleep 30\n".to_vec(), cx)
    });
    wait_until(cx, "sleep never started", |cx| {
        ws.update(cx, |ws, cx| {
            let pane = ws.focused;
            let ix = ws.focused_pane().active;
            ws.running_program(pane, ix, cx).is_some()
        })
    });
}

pub(crate) fn close_active(ws: &Entity<Workspace>, cx: &mut VisualTestContext) {
    ws.update_in(cx, |ws, window, cx| ws.close_tab(&CloseTab, window, cx));
    cx.run_until_parked();
}

pub(crate) fn dialog(
    ws: &Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> Option<(CloseTarget, String)> {
    ws.update(cx, |ws, cx| {
        ws.confirm_close.as_ref().map(|confirm| {
            let confirm = confirm.read(cx);
            (confirm.target.clone(), confirm.message().to_string())
        })
    })
}

pub(crate) fn quitting(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
    ws.update(cx, |ws, _| ws.quitting)
}

pub(crate) fn window_settings(json: &str) -> Settings {
    Settings::parse(&format!(
        r#"{{"hide_bar_for_one_tab": true, "terminal": {{"scrollbar": {{"enable": "off"}}}},
            "window": {json}}}"#
    ))
    .unwrap()
}

pub(crate) fn grid_bounds(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> TerminalBounds {
    let terminal = terminal(ws, cx, 0);
    terminal.read_with(cx, |t, _| t.last_content.terminal_bounds)
}

/// active tab's terminal with bracketed paste forced off, like a shell that never asks for it
pub(crate) fn without_bracketed_paste(ws: &Entity<Workspace>, cx: &mut VisualTestContext) {
    let ix = current(ws, cx);
    terminal(ws, cx, ix).update(cx, |terminal, _| {
        terminal.last_content.mode.remove(TermMode::BRACKETED_PASTE)
    });
}

pub(crate) fn search_bar(
    ws: &Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> Option<Entity<crate::ui::search_bar::SearchBar>> {
    ws.update(cx, |ws, cx| {
        let pane = ws.focused_pane();
        pane.tabs[pane.active].view.read(cx).search_bar().cloned()
    })
}

pub(crate) fn active_view(
    ws: &Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> Entity<TerminalView> {
    ws.update(cx, |ws, _| {
        let pane = ws.focused_pane();
        pane.tabs[pane.active].view.clone()
    })
}

pub(crate) fn leaf(id: usize) -> LayoutNode {
    LayoutNode::Leaf(Pane {
        id,
        tabs: Vec::new(),
        active: 0,
        tab_scroll: ScrollHandle::new(),
    })
}

pub(crate) fn split(
    axis: SplitAxis,
    ratio: f32,
    first: LayoutNode,
    second: LayoutNode,
) -> LayoutNode {
    LayoutNode::Split {
        axis,
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    }
}

/// 0 on the left, 1 above 2 on the right
pub(crate) fn three_panes() -> LayoutNode {
    split(
        SplitAxis::Horizontal,
        0.5,
        leaf(0),
        split(SplitAxis::Vertical, 0.5, leaf(1), leaf(2)),
    )
}

/// run `command` in tab `ix` and wait until shell integration marked it finished, giving the
/// id of its block. tests that use it run inside `with_own_state`, other tests swap the state
/// folder the integration scripts live in
pub(crate) fn finished_block(
    ws: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    ix: usize,
    command: &str,
) -> u64 {
    // held back until the shell is ready, text typed before its first prompt gets no block
    let line = format!("{command}\r").into_bytes();
    ws.update(cx, |ws, cx| {
        ws.input_to(ws.focused, ix, InputSource::Keyboard, line, cx)
    });
    let terminal = terminal(ws, cx, ix);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let found = terminal.update(cx, |t, _| {
            t.blocks()
                .iter()
                .find(|block| block.command == command && block.ended.is_some())
                .map(|block| block.id)
        });
        if let Some(id) = found {
            return id;
        }
        if Instant::now() > deadline {
            let (commands, screen) = terminal.update(cx, |t, _| {
                let commands: Vec<String> = t
                    .blocks()
                    .iter()
                    .map(|block| block.command.clone())
                    .collect();
                (commands, t.visible_screen_text(false).0)
            });
            panic!("{command:?} never finished as a block, blocks {commands:?}, screen:\n{screen}");
        }
        pump(cx);
    }
}

/// run `f` with its own config and state folders, held against other tests changing them, so
/// shells it starts find kuterm's shell integration scripts until it ends
pub(crate) fn with_own_state(name: &str, f: impl FnOnce()) {
    let dir = crate::settings::tests::temp_dir(name);
    crate::settings::tests::with_config_home(&dir, f);
    let _ = std::fs::remove_dir_all(&dir);
}
