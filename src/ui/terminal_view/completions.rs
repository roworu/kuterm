//! completion popup of a terminal view: follows what the shell reports about its command
//! line, asks the engine for candidates off the main thread, and draws them at the cursor

use std::{path::PathBuf, sync::Arc};

use gpui::{
    Anchor, AnyElement, AppContext, Context, Global, InteractiveElement, IntoElement,
    ParentElement, Pixels, Point, ScrollDelta, ScrollWheelEvent, SharedString, Styled, Task,
    anchored, deferred, div, point, prelude::FluentBuilder as _, px,
};

use crate::{
    completion::{self, Candidate, Completions, Kind, Request, ShellEvent, ShellMessage, Specs},
    settings::{CompletionTrigger, Settings, config_dir},
    theme::Theme,
};

use super::TerminalView;

/// specs of every tab, read once on first use
struct CompletionSpecs(Arc<Specs>);

impl Global for CompletionSpecs {}

// widest label and description shown, in cells, longer ones end in an ellipsis
const MAX_LABEL_CELLS: usize = 40;
const MAX_DESCRIPTION_CELLS: usize = 50;

/// the command line a shell reported last
#[derive(Clone, Debug)]
struct Line {
    conn: u64,
    seq: u64,
    buffer: String,
    cursor: usize,
    cwd: PathBuf,
}

#[derive(Default)]
pub(super) struct CompletionMenu {
    /// none while no shell edits a line
    line: Option<Line>,
    /// PATH the shell reported, kuterm's own until then
    path_var: Option<String>,
    /// candidates with the `seq` of the line they are for
    result: Option<(u64, Completions)>,
    /// the popup should show, it still needs candidates for that
    open: bool,
    selected: usize,
    /// first row shown
    top: usize,
    /// connection and word start escape hid the popup for, it stays hidden for that word
    dismissed: Option<(u64, usize)>,
    /// accept asked for while the candidates were older than the line, done once they come
    pending_accept: bool,
    /// running completion, a new one replaces and so cancels it
    task: Option<Task<()>>,
}

impl CompletionMenu {
    /// true when the popup has something to show
    pub(super) fn visible(&self) -> bool {
        self.open && self.line.is_some() && self.result.is_some()
    }

    fn candidates(&self) -> &[Candidate] {
        self.result
            .as_ref()
            .map_or(&[], |(_, result)| result.candidates.as_slice())
    }

    /// candidates the user would want to see now: none when the only one is what is typed
    fn worth_showing(result: &Completions) -> bool {
        match result.candidates.as_slice() {
            [] => false,
            [only] => only.insert.trim_end() != result.typed,
            _ => true,
        }
    }
}

impl TerminalView {
    /// follow what a shell of this tab says about its command line
    pub(super) fn shell_event(&mut self, event: &ShellEvent, cx: &mut Context<Self>) {
        let settings = Settings::get(cx).autocomplete;
        let menu = &mut self.completions;
        match &event.message {
            ShellMessage::Hello { .. } => {}
            ShellMessage::Env { path } => menu.path_var = Some(path.clone()),
            ShellMessage::Line {
                seq,
                buffer,
                cursor,
                cwd,
            } => {
                let line = Line {
                    conn: event.conn,
                    seq: *seq,
                    buffer: buffer.clone(),
                    cursor: *cursor,
                    cwd: cwd.clone(),
                };
                let blank = line
                    .buffer
                    .chars()
                    .take(line.cursor)
                    .all(char::is_whitespace);
                menu.line = Some(line);
                let auto = settings.trigger == CompletionTrigger::Auto;
                // auto shows nothing on an empty line, the key lists commands there too
                if settings.enable && ((auto && !blank) || (!auto && menu.open)) {
                    self.update_completions(cx);
                } else {
                    menu.open = false;
                    menu.result = None;
                    menu.task = None;
                }
            }
            ShellMessage::End | ShellMessage::Closed => {
                if menu
                    .line
                    .as_ref()
                    .is_some_and(|line| line.conn == event.conn)
                {
                    *menu = CompletionMenu {
                        path_var: menu.path_var.take(),
                        ..CompletionMenu::default()
                    };
                }
            }
        }
        cx.notify();
    }

    /// ask the engine about the current line, off the main thread
    fn update_completions(&mut self, cx: &mut Context<Self>) {
        let Some(line) = self.completions.line.clone() else {
            return;
        };
        if !cx.has_global::<CompletionSpecs>() {
            let dir = config_dir().map(|dir| dir.join("completions"));
            cx.set_global(CompletionSpecs(Arc::new(Specs::load(dir.as_deref()))));
        }
        let specs = cx.global::<CompletionSpecs>().0.clone();
        let request = Request {
            buffer: line.buffer,
            cursor: line.cursor,
            cwd: line.cwd,
            path_var: self
                .completions
                .path_var
                .clone()
                .or_else(|| std::env::var("PATH").ok())
                .unwrap_or_default(),
            home: std::env::var_os("HOME").map(PathBuf::from),
        };
        let seq = line.seq;
        let found = cx.background_spawn(async move { completion::complete(&request, &specs) });
        self.completions.task = Some(cx.spawn(async move |this, cx| {
            let result = found.await;
            this.update(cx, |this, cx| this.completions_found(seq, result, cx))
                .ok();
        }));
    }

    fn completions_found(&mut self, seq: u64, result: Completions, cx: &mut Context<Self>) {
        let auto = Settings::get(cx).autocomplete.trigger == CompletionTrigger::Auto;
        let menu = &mut self.completions;
        let Some(line) = &menu.line else { return };
        // the line changed again, its own result is on the way
        if line.seq != seq {
            return;
        }
        let conn = line.conn;
        if menu
            .dismissed
            .is_some_and(|dismissed| dismissed != (conn, result.start))
        {
            menu.dismissed = None;
        }
        if auto {
            menu.open = menu.dismissed.is_none() && CompletionMenu::worth_showing(&result);
        }
        menu.selected = 0;
        menu.top = 0;
        menu.result = Some((seq, result));
        if std::mem::take(&mut menu.pending_accept) {
            self.accept_completion(cx);
        }
        cx.notify();
    }

    /// show the popup for the current line, false when no shell reported one
    pub(super) fn open_completions(&mut self, cx: &mut Context<Self>) -> bool {
        if self.completions.line.is_none() || !Settings::get(cx).autocomplete.enable {
            return false;
        }
        self.completions.open = true;
        self.completions.dismissed = None;
        self.update_completions(cx);
        cx.notify();
        true
    }

    /// move the selection by `delta` rows, wrapping around
    pub(super) fn move_completion(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = Settings::get(cx).autocomplete.max_items;
        let menu = &mut self.completions;
        let count = menu.candidates().len();
        if count == 0 {
            return;
        }
        menu.selected = (menu.selected as isize + delta).rem_euclid(count as isize) as usize;
        // keep the selected row in view
        if menu.selected < menu.top {
            menu.top = menu.selected;
        } else if menu.selected >= menu.top + rows {
            menu.top = menu.selected + 1 - rows;
        }
        cx.notify();
    }

    /// put the selected candidate into the shell's command line
    pub(super) fn accept_completion(&mut self, cx: &mut Context<Self>) {
        let menu = &mut self.completions;
        let (Some(line), Some((seq, result))) = (&menu.line, &menu.result) else {
            return;
        };
        // the shell would drop an answer to an older line, so wait for the fresh one
        if *seq != line.seq {
            menu.pending_accept = true;
            return;
        }
        let Some(candidate) = result.candidates.get(menu.selected) else {
            return;
        };
        let message =
            completion::protocol::accept(*seq, result.start, result.end, &candidate.insert);
        let conn = line.conn;
        menu.open = false;
        // the next line the shell reports brings the popup back for what follows
        if let Some(message) = message {
            self.terminal.read(cx).send_to_shell(conn, &message);
        }
        cx.notify();
    }

    /// hide the popup until the next word
    pub(super) fn dismiss_completions(&mut self, cx: &mut Context<Self>) {
        let menu = &mut self.completions;
        menu.open = false;
        menu.pending_accept = false;
        if let (Some(line), Some((_, result))) = (&menu.line, &menu.result) {
            menu.dismissed = Some((line.conn, result.start));
        }
        cx.notify();
    }

    fn completion_clicked(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.completions.selected = ix;
        self.accept_completion(cx);
    }

    fn completion_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let rows = Settings::get(cx).autocomplete.max_items;
        let menu = &mut self.completions;
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => -delta.y.round() as isize,
            ScrollDelta::Pixels(delta) => -(delta.y.signum() as isize),
        };
        let max_top = menu.candidates().len().saturating_sub(rows);
        menu.top = (menu.top as isize + lines).clamp(0, max_top as isize) as usize;
        cx.notify();
    }

    /// the popup at the cursor, below it or above when there is no room below
    pub(super) fn render_completions(
        &self,
        window_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = &self.completions;
        let (_, result) = menu.result.as_ref()?;
        let settings = Settings::get(cx);
        let font = settings.terminal.font.family.clone();
        let rows = settings.autocomplete.max_items;
        let descriptions = settings.autocomplete.show_descriptions;
        let terminal = self.terminal.read(cx);
        let content = &terminal.last_content;
        let bounds = content.terminal_bounds;
        let theme = terminal.theme(cx).clone();
        let font_size = px(self.font_size(cx));
        let (cell, line_height) = (bounds.cell_width, bounds.line_height);

        // the word being completed starts left of the cursor
        let typed = result.end.saturating_sub(result.start);
        let column = content.cursor.point.column.0.saturating_sub(typed);
        let row = content.cursor.point.line.0 + content.display_offset as i32;
        let x = bounds.bounds.origin.x + cell * column as f32;
        let top = bounds.bounds.origin.y + line_height * row as f32;
        let bottom = top + line_height;

        let candidates = &result.candidates;
        let shown = &candidates[menu.top.min(candidates.len())..];
        let shown = &shown[..shown.len().min(rows)];
        let cells = |text: &str, max: usize| text.chars().count().min(max);
        let label_cells = shown
            .iter()
            .map(|c| cells(&c.label, MAX_LABEL_CELLS))
            .max()
            .unwrap_or(0)
            .max(12);
        let description_cells = shown
            .iter()
            .filter(|_| descriptions)
            .filter_map(|c| Some(cells(c.description.as_ref()?, MAX_DESCRIPTION_CELLS)))
            .max()
            .map_or(0, |cells| cells + 2);
        let pad = px(6.);
        let width = (cell * (label_cells + description_cells) as f32 + pad * 2.)
            .min(bounds.bounds.size.width);
        let height = line_height * shown.len().max(1) as f32 + px(6.);
        // shifted left to stay inside its own pane, the window clamp lets it cross a divider
        let x = x
            .min(bounds.bounds.origin.x + bounds.bounds.size.width - width)
            .max(bounds.bounds.origin.x);
        // below unless it doesn't fit there and fits better above
        let above = bottom + height > window_height && top > window_height - bottom;
        let (anchor, position): (Anchor, Point<Pixels>) = if above {
            (Anchor::BottomLeft, point(x, top))
        } else {
            (Anchor::TopLeft, point(x, bottom))
        };

        let items: Vec<AnyElement> = if shown.is_empty() {
            vec![
                div()
                    .h(line_height)
                    .px(pad)
                    .text_color(theme.text_muted)
                    .child("no completions")
                    .into_any_element(),
            ]
        } else {
            shown
                .iter()
                .enumerate()
                .map(|(offset, candidate)| {
                    let ix = menu.top + offset;
                    completion_row(
                        candidate,
                        ix == menu.selected,
                        &theme,
                        cell * label_cells as f32,
                        descriptions,
                        line_height,
                        pad,
                    )
                    .id(("completion", ix))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.completion_clicked(ix, cx);
                        }),
                    )
                    .into_any_element()
                })
                .collect()
        };
        let scrollbar = (candidates.len() > rows).then(|| {
            let track = line_height * rows as f32;
            let thumb = (track * (rows as f32 / candidates.len() as f32)).max(px(8.));
            let offset = (track - thumb) * (menu.top as f32 / (candidates.len() - rows) as f32);
            div()
                .absolute()
                .right(px(1.))
                .top(px(3.) + offset)
                .w(px(3.))
                .h(thumb)
                .rounded_sm()
                .bg(theme.scrollbar)
        });
        let popup = div()
            .id("completions")
            .debug_selector(|| "completions".into())
            .relative()
            .w(width)
            .py(px(3.))
            .bg(theme.tab_bar_background)
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .shadow_lg()
            .overflow_hidden()
            .font_family(SharedString::from(font))
            .text_size(font_size)
            .text_color(theme.text)
            // keeps clicks from reaching the terminal below
            .occlude()
            .on_scroll_wheel(cx.listener(|this, event, _, cx| {
                cx.stop_propagation();
                this.completion_wheel(event, cx);
            }))
            .children(items)
            .children(scrollbar);
        Some(
            deferred(
                anchored()
                    .anchor(anchor)
                    .position(position)
                    .snap_to_window_with_margin(px(4.))
                    .child(popup),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

/// label color of a kind of candidate, from the terminal palette so it fits the theme. like
/// ls: folders blue, programs green, plain files in the text color
fn kind_color(kind: Kind, theme: &Theme) -> gpui::Hsla {
    match kind {
        Kind::Subcommand => theme.ansi[5],
        Kind::Suggestion => theme.ansi[6],
        Kind::Option => theme.ansi[3],
        Kind::Command => theme.ansi[2],
        Kind::Folder => theme.ansi[4],
        Kind::File => theme.text,
    }
}

fn completion_row(
    candidate: &Candidate,
    selected: bool,
    theme: &Theme,
    label_width: Pixels,
    descriptions: bool,
    height: Pixels,
    pad: Pixels,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .h(height)
        .px(pad)
        .whitespace_nowrap()
        .when(selected, |row| row.bg(theme.tab_active_background))
        .child(
            div()
                .flex_none()
                .w(label_width)
                .truncate()
                .text_color(kind_color(candidate.kind, theme))
                .child(candidate.label.clone()),
        )
        .when_some(
            candidate.description.clone().filter(|_| descriptions),
            |row, description| {
                row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pl(height * 0.5)
                        .truncate()
                        .text_color(theme.text_muted)
                        .child(description),
                )
            },
        )
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixStream,
        time::Duration,
    };

    use futures::StreamExt;
    use gpui::{Entity, TestAppContext, VisualTestContext};

    use super::*;
    use crate::{
        completion::Bridge, settings::Keybindings, ui::terminal_view::tests::focused_view,
    };

    /// the far end of the tab's socket, where a shell would be
    struct FakeShell {
        conn: u64,
        reader: BufReader<UnixStream>,
        cwd: PathBuf,
    }

    impl FakeShell {
        /// next line kuterm sent to the shell
        fn received(&mut self) -> String {
            let mut line = String::new();
            self.reader
                .read_line(&mut line)
                .expect("nothing sent to the shell");
            line
        }
    }

    impl Drop for FakeShell {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.cwd).ok();
        }
    }

    /// a view with settings from `json` and the keys of the bundled keybindings, a fake
    /// shell connected to its tab, in a folder with "alpha.txt" and "alpine/"
    fn with_shell<'a>(
        name: &str,
        json: &str,
        cx: &'a mut TestAppContext,
    ) -> (Entity<TerminalView>, &'a mut VisualTestContext, FakeShell) {
        let (view, cx) = focused_view(json, cx);
        cx.update(|_, cx| cx.bind_keys(Keybindings::default().bindings()));
        let mut bridge = Bridge::new().unwrap();
        let mut events = bridge.take_events().unwrap();
        let mut stream = UnixStream::connect(bridge.path()).unwrap();
        stream
            .write_all(b"{\"v\":1,\"type\":\"hello\",\"shell\":\"zsh\"}\n")
            .unwrap();
        let conn = futures::executor::block_on(events.next()).unwrap().conn;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        view.update(cx, |view, cx| {
            view.terminal
                .update(cx, |terminal, _| terminal.set_completion_bridge(bridge))
        });
        let cwd = std::env::temp_dir().join(format!("kuterm-popup-{name}-{}", std::process::id()));
        std::fs::create_dir_all(cwd.join("alpine")).unwrap();
        std::fs::write(cwd.join("alpha.txt"), "").unwrap();
        let shell = FakeShell {
            conn,
            reader: BufReader::new(stream),
            cwd,
        };
        (view, cx, shell)
    }

    fn line_event(shell: &FakeShell, seq: u64, buffer: &str) -> ShellEvent {
        ShellEvent {
            conn: shell.conn,
            message: ShellMessage::Line {
                seq,
                buffer: buffer.into(),
                cursor: buffer.chars().count(),
                cwd: shell.cwd.clone(),
            },
        }
    }

    /// the shell reports `buffer` with the cursor at its end
    fn type_line(
        view: &Entity<TerminalView>,
        cx: &mut VisualTestContext,
        shell: &FakeShell,
        seq: u64,
        buffer: &str,
    ) {
        let event = line_event(shell, seq, buffer);
        view.update(cx, |view, cx| view.shell_event(&event, cx));
        cx.run_until_parked();
    }

    /// labels in the popup, empty while it doesn't show
    fn shown(view: &Entity<TerminalView>, cx: &mut VisualTestContext) -> Vec<String> {
        view.update_in(cx, |view, window, cx| {
            if !view.completions_shown(window, cx) {
                return Vec::new();
            }
            let candidates = view.completions.candidates();
            candidates.iter().map(|c| c.label.clone()).collect()
        })
    }

    const AUTO: &str = r#"{"autocomplete": {"enable": true, "trigger": "auto"}}"#;

    #[gpui::test]
    fn auto_popup_follows_the_line_and_accepts_into_it(cx: &mut TestAppContext) {
        let (view, cx, mut shell) = with_shell("auto", AUTO, cx);
        type_line(&view, cx, &shell, 1, "cat al");
        assert_eq!(shown(&view, cx), ["alpine/", "alpha.txt"]);
        assert!(cx.debug_bounds("completions").is_some(), "popup not drawn");

        cx.simulate_keystrokes("down");
        assert_eq!(view.read_with(cx, |view, _| view.completions.selected), 1);
        cx.simulate_keystrokes("tab");
        assert_eq!(shell.received(), "1 accept 1 4 6 alpha.txt \n");
        // closed until the shell reports the completed line
        assert!(shown(&view, cx).is_empty());
        assert!(cx.debug_bounds("completions").is_none());

        // nothing to show on an empty line, and nothing once the line is run
        type_line(&view, cx, &shell, 2, "  ");
        assert!(shown(&view, cx).is_empty());
        type_line(&view, cx, &shell, 3, "cat al");
        let end = ShellEvent {
            conn: shell.conn,
            message: ShellMessage::End,
        };
        view.update(cx, |view, cx| view.shell_event(&end, cx));
        assert!(shown(&view, cx).is_empty());
    }

    #[gpui::test]
    fn escape_hides_the_popup_until_the_next_word(cx: &mut TestAppContext) {
        let (view, cx, shell) = with_shell("escape", AUTO, cx);
        type_line(&view, cx, &shell, 1, "cat al");
        assert!(!shown(&view, cx).is_empty());
        cx.simulate_keystrokes("escape");
        assert!(shown(&view, cx).is_empty());
        type_line(&view, cx, &shell, 2, "cat alp");
        assert!(shown(&view, cx).is_empty());
        type_line(&view, cx, &shell, 3, "cat alpha.txt al");
        assert_eq!(shown(&view, cx), ["alpine/", "alpha.txt"]);
        // the complete key brings it back for the same word
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes("ctrl-space");
        assert_eq!(shown(&view, cx), ["alpine/", "alpha.txt"]);
    }

    #[gpui::test]
    fn key_trigger_waits_for_the_key(cx: &mut TestAppContext) {
        let json = r#"{"autocomplete": {"enable": true, "trigger": "key"}}"#;
        let (view, cx, shell) = with_shell("key", json, cx);
        type_line(&view, cx, &shell, 1, "cat al");
        assert!(shown(&view, cx).is_empty());
        cx.simulate_keystrokes("ctrl-space");
        assert_eq!(shown(&view, cx), ["alpine/", "alpha.txt"]);
        // it stays open while typing, then the list follows the line
        type_line(&view, cx, &shell, 2, "cat alpi");
        assert_eq!(shown(&view, cx), ["alpine/"]);
        // on an empty line the key lists commands
        cx.simulate_keystrokes("escape");
        type_line(&view, cx, &shell, 3, "");
        cx.simulate_keystrokes("ctrl-space");
        assert!(shown(&view, cx).contains(&"cd".to_string()));
    }

    #[gpui::test]
    fn accept_waits_for_the_candidates_of_the_newest_line(cx: &mut TestAppContext) {
        let (view, cx, mut shell) = with_shell("stale", AUTO, cx);
        type_line(&view, cx, &shell, 1, "cat al");
        let newer = line_event(&shell, 2, "cat alph");
        view.update(cx, |view, cx| {
            view.shell_event(&newer, cx);
            view.accept_completion(cx);
        });
        cx.run_until_parked();
        assert_eq!(shell.received(), "1 accept 2 4 8 alpha.txt \n");
    }

    #[gpui::test]
    fn nothing_shows_while_autocomplete_is_off(cx: &mut TestAppContext) {
        let json = r#"{"autocomplete": {"enable": false}}"#;
        let (view, cx, shell) = with_shell("off", json, cx);
        type_line(&view, cx, &shell, 1, "cat al");
        assert!(shown(&view, cx).is_empty());
        cx.simulate_keystrokes("ctrl-space");
        assert!(shown(&view, cx).is_empty());
    }

    #[test]
    fn kinds_have_colors_of_their_own() {
        let theme = Theme::default();
        let kinds = [
            Kind::Subcommand,
            Kind::Suggestion,
            Kind::Option,
            Kind::Command,
            Kind::Folder,
            Kind::File,
        ];
        let colors: Vec<_> = kinds.map(|kind| kind_color(kind, &theme)).into();
        for (ix, color) in colors.iter().enumerate() {
            assert!(!colors[ix + 1..].contains(color), "{:?}", kinds[ix]);
        }
    }

    #[test]
    fn a_lone_candidate_equal_to_the_word_is_not_worth_showing() {
        let mut found = completion::complete(
            &Request {
                buffer: "cd".into(),
                cursor: 2,
                cwd: "/".into(),
                path_var: String::new(),
                home: None,
            },
            &Specs::default(),
        );
        assert_eq!(found.typed, "cd");
        found.candidates.retain(|c| c.label == "cd");
        assert!(!CompletionMenu::worth_showing(&found));
        found.typed = "c".into();
        assert!(CompletionMenu::worth_showing(&found));
    }
}
