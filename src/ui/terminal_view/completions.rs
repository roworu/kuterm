//! completion popup of a terminal view: follows what the shell reports about its command
//! line, asks the engine for candidates off the main thread, and draws them at the cursor

use std::{collections::VecDeque, path::PathBuf, sync::Arc, time::Duration};

use gpui::{
    Anchor, AnyElement, AppContext, Context, Global, InteractiveElement, IntoElement, Keystroke,
    ParentElement, Pixels, Point, ScrollDelta, ScrollWheelEvent, SharedString, Styled, Task,
    anchored, deferred, div, point, prelude::FluentBuilder as _, px,
};

use crate::{
    completion::{self, Candidate, Completions, Kind, Request, ShellEvent, ShellMessage, Specs},
    settings::{CompletionTrigger, Settings, config_dir},
    terminal::KeyEventKind,
    theme::Theme,
};

use super::TerminalView;

/// specs of every tab, read once on first use
struct CompletionSpecs(Arc<Specs>);

impl Global for CompletionSpecs {}

// widest label shown, in cells, longer ones end in an ellipsis
const MAX_LABEL_CELLS: usize = 40;
// widest description shown next to its label, the selected one is also shown in full below
const MAX_DESCRIPTION_CELLS: usize = 32;
// descriptions are smaller than labels, so more of them fits in the popup
const DESCRIPTION_SCALE: f32 = 0.85;
// longest the full description below the list grows before it is cut off
const MAX_DETAIL_LINES: usize = 3;
// gpui's default line height, a multiple of the font size
const LINE_HEIGHT_RATIO: f32 = 1.618;

// popup keys and accepts stop waiting for the shell after this, one that never answers, like
// a nested zsh of an older kuterm, or a slow folder must not eat them
const KEY_WAIT: Duration = Duration::from_millis(300);

/// what a popup key does
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PopupKey {
    Next,
    Prev,
    Accept,
    Dismiss,
}

/// the command line a shell reported last
#[derive(Clone, Debug)]
struct Line {
    conn: u64,
    seq: u64,
    buffer: String,
    cursor: usize,
    cwd: PathBuf,
}

/// a completion sent to the shell that it has not answered yet
struct SentAccept {
    /// the line it was for, none when its candidates were older than the line
    seq: Option<u64>,
    /// the shell turned it down, its line had changed meanwhile
    rejected: bool,
    /// label of the picked candidate, kept when the selection was moved to it
    picked: Option<String>,
    /// key that accepted it, the shell gets it when the newer line has nothing to pick
    keystroke: Option<Keystroke>,
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
    /// the selection was moved by the user since the candidates came
    moved: bool,
    /// first row shown
    top: usize,
    /// connection and word start escape hid the popup for, it stays hidden for that word
    dismissed: Option<(u64, usize)>,
    /// last keystroke this view got, popup keys keep it to pass it on
    keystroke: Option<Keystroke>,
    /// accept the shell has not answered, keys after it wait for the line it makes
    accept: Option<SentAccept>,
    /// keys went to the pty after the line was reported. an accept on the socket could
    /// reach the shell before them, so it waits for the line they make
    typed_ahead: bool,
    /// popup keys typed while candidates or an answer were on the way
    queued: VecDeque<(PopupKey, Option<Keystroke>)>,
    /// stops waiting when the shell takes too long
    wait_timeout: Option<Task<()>>,
    /// running completion, a new one replaces and so cancels it
    task: Option<Task<()>>,
}

impl CompletionMenu {
    /// true when the popup has something to show
    pub(super) fn visible(&self) -> bool {
        self.open && self.line.is_some() && self.result.is_some()
    }

    /// a shell edits its line, so popup keys go through the popup
    pub(super) fn editing(&self) -> bool {
        self.line.is_some()
    }

    /// candidates for the line are still being found
    fn finding(&self) -> bool {
        let Some(line) = &self.line else {
            return false;
        };
        self.task.is_some() && self.result.as_ref().is_none_or(|(seq, _)| *seq != line.seq)
    }

    /// popup keys have to wait: for candidates, or for the shell to answer an accept
    fn busy(&self) -> bool {
        self.line.is_some() && (self.accept.is_some() || self.finding())
    }

    /// keys were sent to the shell, the line it reported last is behind them
    pub(super) fn typed(&mut self) {
        self.typed_ahead = true;
    }

    /// remember the keystroke being dispatched, its popup key may pass it on later
    pub(super) fn set_keystroke(&mut self, keystroke: Keystroke) {
        self.keystroke = Some(keystroke);
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
                history,
            } => {
                let line = Line {
                    conn: event.conn,
                    seq: *seq,
                    buffer: buffer.clone(),
                    cursor: *cursor,
                    cwd: cwd.clone(),
                };
                // a new shell, like a nested one, answers nothing the old one was sent
                if menu.line.as_ref().is_some_and(|old| old.conn != line.conn) {
                    menu.accept = None;
                }
                let blank = line
                    .buffer
                    .chars()
                    .take(line.cursor)
                    .all(char::is_whitespace);
                menu.line = Some(line);
                menu.typed_ahead = false;
                let auto = settings.trigger == CompletionTrigger::Auto;
                // auto shows nothing on an empty line, the key lists commands there too.
                // moving through history shows nothing either, so the next up or down
                // scrolls the history instead of the popup; the complete key still opens it
                if settings.enable && ((auto && !blank && !*history) || (!auto && menu.open)) {
                    self.update_completions(cx);
                } else {
                    menu.open = false;
                    menu.result = None;
                    menu.task = None;
                }
            }
            ShellMessage::Answered { seq, applied } => {
                if let Some(accept) = &mut menu.accept
                    && accept.seq == Some(*seq)
                {
                    if *applied {
                        menu.accept = None;
                    } else {
                        accept.rejected = true;
                    }
                }
            }
            ShellMessage::End | ShellMessage::Closed => {
                if menu
                    .line
                    .as_ref()
                    .is_some_and(|line| line.conn == event.conn)
                {
                    // keys still waiting go to whatever reads them now
                    *menu = CompletionMenu {
                        path_var: menu.path_var.take(),
                        keystroke: menu.keystroke.take(),
                        queued: std::mem::take(&mut menu.queued),
                        ..CompletionMenu::default()
                    };
                }
            }
        }
        self.run_popup_keys(cx);
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
        menu.moved = false;
        menu.top = 0;
        menu.result = Some((seq, result));
        self.run_popup_keys(cx);
        cx.notify();
    }

    /// a popup key, true when the popup took it. while candidates or an answer of the shell
    /// are on the way it waits for them
    pub(super) fn popup_key(&mut self, key: PopupKey, cx: &mut Context<Self>) -> bool {
        let menu = &mut self.completions;
        if menu.busy() || !menu.queued.is_empty() {
            let keystroke = menu.keystroke.clone();
            menu.queued.push_back((key, keystroke));
            self.run_popup_keys(cx);
            return true;
        }
        if !menu.visible() {
            return false;
        }
        let keystroke = menu.keystroke.clone();
        self.run_popup_key(key, keystroke, cx);
        true
    }

    fn run_popup_key(
        &mut self,
        key: PopupKey,
        keystroke: Option<Keystroke>,
        cx: &mut Context<Self>,
    ) {
        match key {
            PopupKey::Next => self.move_completion(1, cx),
            PopupKey::Prev => self.move_completion(-1, cx),
            PopupKey::Accept => self.accept_completion(keystroke, cx),
            PopupKey::Dismiss => self.dismiss_completions(cx),
        }
    }

    /// do what waits once nothing is on the way: an accept the shell turned down again on
    /// the newer line, then the queued keys, on the popup when it shows, else in the shell
    fn run_popup_keys(&mut self, cx: &mut Context<Self>) {
        let menu = &mut self.completions;
        let seq = menu.line.as_ref().map(|line| line.seq);
        // a line newer than the one turned down, the same one would be turned down again
        let newer = |accept: &mut SentAccept| {
            accept.rejected
                && accept
                    .seq
                    .is_none_or(|sent| seq.is_some_and(|seq| seq > sent))
        };
        if !menu.finding()
            && !menu.typed_ahead
            && let Some(accept) = menu.accept.take_if(newer)
        {
            self.redo_accept(accept, cx);
        }
        let mut ran = false;
        while !self.completions.busy() {
            let Some((key, keystroke)) = self.completions.queued.pop_front() else {
                break;
            };
            ran = true;
            if self.completions.visible() {
                self.run_popup_key(key, keystroke, cx);
            } else if let Some(keystroke) = keystroke {
                self.send_key(&keystroke, KeyEventKind::Press, cx);
            }
        }
        let menu = &mut self.completions;
        if !menu.busy() && menu.queued.is_empty() {
            menu.wait_timeout = None;
        } else if ran || menu.wait_timeout.is_none() {
            menu.wait_timeout = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(KEY_WAIT).await;
                this.update(cx, |this, cx| this.waited_too_long(cx)).ok();
            }));
        }
    }

    /// nothing came in time. typed keys that brought no new line changed nothing, so an
    /// accept waiting for them is done on the line there is
    fn waited_too_long(&mut self, cx: &mut Context<Self>) {
        let menu = &mut self.completions;
        if std::mem::take(&mut menu.typed_ahead)
            && menu
                .accept
                .as_ref()
                .is_some_and(|accept| accept.seq.is_none())
        {
            menu.wait_timeout = None;
            self.run_popup_keys(cx);
        } else {
            self.stop_waiting(cx);
        }
    }

    /// the shell or the engine took too long: an unanswered accept is given up, keys still
    /// waiting go to the shell
    fn stop_waiting(&mut self, cx: &mut Context<Self>) {
        let menu = &mut self.completions;
        menu.wait_timeout = None;
        menu.accept = None;
        self.run_popup_keys(cx);
        let menu = &mut self.completions;
        menu.wait_timeout = None;
        for (_, keystroke) in std::mem::take(&mut menu.queued) {
            if let Some(keystroke) = keystroke {
                self.send_key(&keystroke, KeyEventKind::Press, cx);
            }
        }
    }

    /// send the queued keys to the shell, a key typed after them must not pass them
    pub(super) fn forward_popup_keys(&mut self, cx: &mut Context<Self>) {
        if !self.completions.queued.is_empty() {
            self.stop_waiting(cx);
        }
    }

    /// accept on the candidates of the newer line what the shell turned down for an older
    /// one: the same candidate when it was picked by moving to it, else the first
    fn redo_accept(&mut self, accept: SentAccept, cx: &mut Context<Self>) {
        let menu = &mut self.completions;
        let fresh = menu
            .result
            .as_ref()
            .filter(|(seq, _)| menu.line.as_ref().is_some_and(|line| line.seq == *seq))
            .map_or(&[][..], |(_, result)| result.candidates.as_slice());
        let picked = accept
            .picked
            .as_ref()
            .and_then(|label| fresh.iter().position(|c| &c.label == label));
        if fresh.is_empty() {
            if let Some(keystroke) = accept.keystroke {
                self.send_key(&keystroke, KeyEventKind::Press, cx);
            }
            return;
        }
        menu.selected = picked.unwrap_or(0);
        menu.moved = picked.is_some();
        self.accept_completion(accept.keystroke, cx);
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
        menu.moved = true;
        // keep the selected row in view
        if menu.selected < menu.top {
            menu.top = menu.selected;
        } else if menu.selected >= menu.top + rows {
            menu.top = menu.selected + 1 - rows;
        }
        cx.notify();
    }

    /// put the selected candidate into the shell's command line. `keystroke` is the key that
    /// asked, the shell gets it when the line changed to one with nothing to pick
    pub(super) fn accept_completion(
        &mut self,
        keystroke: Option<Keystroke>,
        cx: &mut Context<Self>,
    ) {
        let menu = &mut self.completions;
        let (Some(line), Some((seq, result))) = (&menu.line, &menu.result) else {
            return;
        };
        let Some(candidate) = result.candidates.get(menu.selected) else {
            return;
        };
        let picked = menu.moved.then(|| candidate.label.clone());
        menu.open = false;
        // the shell would turn an answer to an older line down, so it waits for the fresh one
        if *seq != line.seq || menu.typed_ahead {
            menu.accept = Some(SentAccept {
                seq: None,
                rejected: true,
                picked,
                keystroke,
            });
            if menu.typed_ahead {
                self.run_popup_keys(cx);
            }
            return;
        }
        let message =
            completion::protocol::accept(*seq, result.start, result.end, &candidate.insert);
        let (seq, conn) = (*seq, line.conn);
        // the line the shell reports with it brings the popup back for what follows, keys
        // typed until it answers wait for that
        if let Some(message) = message
            && self.terminal.read(cx).send_to_shell(conn, &message)
        {
            self.completions.accept = Some(SentAccept {
                seq: Some(seq),
                rejected: false,
                picked,
                keystroke,
            });
            self.run_popup_keys(cx);
        }
        cx.notify();
    }

    /// hide the popup until the next word
    pub(super) fn dismiss_completions(&mut self, cx: &mut Context<Self>) {
        let menu = &mut self.completions;
        menu.open = false;
        if let (Some(line), Some((_, result))) = (&menu.line, &menu.result) {
            menu.dismissed = Some((line.conn, result.start));
        }
        cx.notify();
    }

    fn completion_clicked(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.completions.selected = ix;
        self.completions.moved = true;
        self.accept_completion(None, cx);
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
        let cells = |text: &str, max: usize| unicode_width::UnicodeWidthStr::width(text).min(max);
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
        let description_size = font_size * DESCRIPTION_SCALE;
        let description_line = description_size * LINE_HEIGHT_RATIO;
        // the selected candidate's description is shown whole under the list: rows have
        // room for only so much of it
        let detail = descriptions
            .then(|| candidates.get(menu.selected))
            .flatten()
            .and_then(|candidate| candidate.description.as_deref());
        let detail_lines = detail.map_or(0, |text| {
            let per_line = ((width - pad * 2.) / (cell * DESCRIPTION_SCALE))
                .floor()
                .max(1.) as usize;
            unicode_width::UnicodeWidthStr::width(text)
                .div_ceil(per_line)
                .min(MAX_DETAIL_LINES)
        });
        let detail_height = if detail_lines > 0 {
            // the gap and border around the description, then its lines
            description_line * detail_lines as f32 + px(7.)
        } else {
            px(0.)
        };
        let height = line_height * shown.len().max(1) as f32 + px(6.) + detail_height;
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
                        description_size,
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
        // the whole description of the selected completion, below the list so the rows
        // above keep their height while the selection moves
        let details = detail.map(|text| {
            div()
                .id("completion-details")
                .debug_selector(|| "completion-details".into())
                .mx(pad)
                .mt(px(3.))
                .pt(px(3.))
                .border_t_1()
                .border_color(theme.border)
                .text_size(description_size)
                .line_height(description_line)
                .text_color(theme.text_muted)
                .whitespace_normal()
                .max_h(description_line * MAX_DETAIL_LINES as f32)
                .overflow_hidden()
                .child(text.to_string())
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
            .children(details)
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

#[allow(clippy::too_many_arguments)]
fn completion_row(
    candidate: &Candidate,
    selected: bool,
    theme: &Theme,
    label_width: Pixels,
    descriptions: bool,
    description_size: Pixels,
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
                        .text_size(description_size)
                        .text_color(theme.text_muted)
                        .child(description),
                )
            },
        )
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        cell::RefCell,
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixStream,
        rc::Rc,
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
                history: false,
            },
        }
    }

    /// like `line_event`, for a line the shell got by moving through history
    fn history_line_event(shell: &FakeShell, seq: u64, buffer: &str) -> ShellEvent {
        let mut event = line_event(shell, seq, buffer);
        if let ShellMessage::Line { history, .. } = &mut event.message {
            *history = true;
        }
        event
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

    /// the shell tells whether it put the accept for line `seq` in
    fn answer(
        view: &Entity<TerminalView>,
        cx: &mut VisualTestContext,
        shell: &FakeShell,
        seq: u64,
        applied: bool,
    ) {
        let event = ShellEvent {
            conn: shell.conn,
            message: ShellMessage::Answered { seq, applied },
        };
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
    fn history_moves_without_opening_the_popup(cx: &mut TestAppContext) {
        let (view, cx, shell) = with_shell("history", AUTO, cx);
        let recalled = history_line_event(&shell, 1, "cat al");
        view.update(cx, |view, cx| view.shell_event(&recalled, cx));
        cx.run_until_parked();
        assert!(shown(&view, cx).is_empty(), "history opened the popup");

        // the complete key still asks for the recalled line
        cx.simulate_keystrokes("ctrl-space");
        assert_eq!(shown(&view, cx), ["alpine/", "alpha.txt"]);

        // typing on the line keeps the popup following it
        type_line(&view, cx, &shell, 2, "cat alp");
        assert_eq!(shown(&view, cx), ["alpine/", "alpha.txt"]);

        // scrolling past this word again hides what was shown for it
        let recalled = history_line_event(&shell, 3, "cat al");
        view.update(cx, |view, cx| view.shell_event(&recalled, cx));
        cx.run_until_parked();
        assert!(shown(&view, cx).is_empty());
        type_line(&view, cx, &shell, 4, "cat alp");
        assert_eq!(shown(&view, cx), ["alpine/", "alpha.txt"]);
    }

    #[gpui::test]
    fn the_selected_completion_shows_its_whole_description_below(cx: &mut TestAppContext) {
        let (view, cx, shell) = with_shell("details", AUTO, cx);
        with_details_spec(cx);
        type_line(&view, cx, &shell, 1, "tool a");
        assert_eq!(shown(&view, cx), ["alpha", "alpine"]);
        let details = |cx: &mut VisualTestContext| {
            cx.debug_bounds("completion-details")
                .expect("no description below the list")
        };
        let first = details(cx);
        assert!(first.size.width > px(0.) && first.size.height > px(0.));
        // the description below follows the selection, and the rows keep their height
        let popup_top = cx.debug_bounds("completions").unwrap().top();
        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        let second = details(cx);
        assert_eq!(
            second.top(),
            first.top(),
            "the list moved under the selection"
        );
        let popup_top_after = cx.debug_bounds("completions").unwrap().top();
        assert_eq!(popup_top_after, popup_top);
    }

    #[gpui::test]
    fn descriptions_off_draws_no_details(cx: &mut TestAppContext) {
        let json =
            r#"{"autocomplete": {"enable": true, "trigger": "auto", "show_descriptions": false}}"#;
        let (view, cx, shell) = with_shell("no-details", json, cx);
        with_details_spec(cx);
        type_line(&view, cx, &shell, 1, "tool a");
        assert!(!shown(&view, cx).is_empty());
        assert!(cx.debug_bounds("completion-details").is_none());
    }

    #[gpui::test]
    fn bundled_specs_show_details_too(cx: &mut TestAppContext) {
        let (view, cx, shell) = with_shell("bundled", AUTO, cx);
        type_line(&view, cx, &shell, 1, "git ");
        assert!(!shown(&view, cx).is_empty());
        let details = cx.debug_bounds("completion-details").expect("no details");
        let popup = cx.debug_bounds("completions").unwrap();
        assert!(details.size.height > px(0.));
        assert!(
            details.bottom() <= popup.bottom(),
            "details outside the popup"
        );
    }

    /// a spec with descriptions, so the popup doesn't depend on the bundled ones
    fn with_details_spec(cx: &mut VisualTestContext) {
        let mut specs = Specs::default();
        specs.insert(
            "tool",
            r#"{"name": "tool", "subcommands": [
                {"name": "alpha", "description": "Alpha does something whose description is long enough to wrap onto another line in the popup"},
                {"name": "alpine", "description": "A second one"},
            ]}"#,
        );
        cx.update(|_, cx| cx.set_global(CompletionSpecs(Arc::new(specs))));
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
        // on an empty line the key lists commands. no PATH leaves the builtins, a long one
        // pushes "cd" past the candidates kept
        cx.simulate_keystrokes("escape");
        let env = ShellEvent {
            conn: shell.conn,
            message: ShellMessage::Env {
                path: String::new(),
            },
        };
        view.update(cx, |view, cx| view.shell_event(&env, cx));
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
            view.accept_completion(None, cx);
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

    /// window position of the middle of popup row `row`
    fn row_center(
        view: &Entity<TerminalView>,
        cx: &mut VisualTestContext,
        row: f32,
    ) -> Point<Pixels> {
        let popup = cx.debug_bounds("completions").expect("popup not drawn");
        let line_height = view.read_with(cx, |view, cx| {
            view.terminal
                .read(cx)
                .last_content
                .terminal_bounds
                .line_height
        });
        // under the border and the top padding
        point(
            popup.center().x,
            popup.top() + px(4.) + line_height * (row + 0.5),
        )
    }

    #[gpui::test]
    fn clicking_a_candidate_accepts_it(cx: &mut TestAppContext) {
        let (view, cx, mut shell) = with_shell("click", AUTO, cx);
        type_line(&view, cx, &shell, 1, "cat al");
        assert_eq!(shown(&view, cx), ["alpine/", "alpha.txt"]);
        let second = row_center(&view, cx, 1.);
        cx.simulate_event(gpui::MouseDownEvent {
            position: second,
            button: gpui::MouseButton::Left,
            click_count: 1,
            ..gpui::MouseDownEvent::default()
        });
        assert_eq!(shell.received(), "1 accept 1 4 6 alpha.txt \n");
        assert!(shown(&view, cx).is_empty());
    }

    #[gpui::test]
    fn wheel_and_arrows_scroll_a_list_longer_than_the_popup(cx: &mut TestAppContext) {
        let json = r#"{"autocomplete": {"enable": true, "trigger": "auto", "max_items": 1}}"#;
        let (view, cx, shell) = with_shell("wheel", json, cx);
        type_line(&view, cx, &shell, 1, "cat al");
        let top = |cx: &mut VisualTestContext| view.read_with(cx, |view, _| view.completions.top);
        let wheel = |delta: ScrollDelta, cx: &mut VisualTestContext| {
            let position = cx.debug_bounds("completions").unwrap().center();
            cx.simulate_event(ScrollWheelEvent {
                position,
                delta,
                ..ScrollWheelEvent::default()
            });
            cx.run_until_parked();
        };
        // a notch down shows the next row, the list ends there
        wheel(ScrollDelta::Lines(point(0., -1.)), cx);
        assert_eq!(top(cx), 1);
        wheel(ScrollDelta::Lines(point(0., -3.)), cx);
        assert_eq!(top(cx), 1);
        // touchpad pixels move a row per event
        wheel(ScrollDelta::Pixels(point(px(0.), px(5.))), cx);
        assert_eq!(top(cx), 0);
        // the terminal under the popup did not scroll
        assert_eq!(
            view.read_with(cx, |view, cx| view
                .terminal
                .read(cx)
                .last_content
                .display_offset),
            0
        );

        // up from the first wraps to the last, scrolled into view, and back
        let selected = |cx: &mut VisualTestContext| {
            view.read_with(cx, |view, _| {
                (view.completions.selected, view.completions.top)
            })
        };
        cx.simulate_keystrokes("up");
        assert_eq!(selected(cx), (1, 1));
        cx.simulate_keystrokes("up");
        assert_eq!(selected(cx), (0, 0));
    }

    #[gpui::test]
    fn popup_with_nothing_found_says_so_and_keys_do_nothing(cx: &mut TestAppContext) {
        let json = r#"{"autocomplete": {"enable": true, "trigger": "key"}}"#;
        let (view, cx, mut shell) = with_shell("none", json, cx);
        type_line(&view, cx, &shell, 1, "cat zzqx");
        cx.simulate_keystrokes("ctrl-space");
        cx.run_until_parked();
        assert!(cx.debug_bounds("completions").is_some(), "popup not drawn");
        assert!(view.read_with(cx, |view, _| view.completions.candidates().is_empty()));
        cx.simulate_keystrokes("down tab");
        assert_eq!(view.read_with(cx, |view, _| view.completions.selected), 0);
        // nothing went to the shell
        shell
            .reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let mut line = String::new();
        assert!(shell.reader.read_line(&mut line).is_err(), "sent {line:?}");
    }

    #[gpui::test]
    fn popup_goes_above_a_cursor_at_the_bottom(cx: &mut TestAppContext) {
        let (view, cx, shell) = with_shell("above", AUTO, cx);
        view.update(cx, |view, cx| {
            view.terminal
                .update(cx, |terminal, _| terminal.type_text(b"seq 1 500\r"))
        });
        let cursor_row = |cx: &mut VisualTestContext| {
            view.update(cx, |view, cx| {
                view.terminal.update(cx, |terminal, _| {
                    terminal.sync();
                    let content = &terminal.last_content;
                    (
                        content.cursor.point.line.0,
                        content.terminal_bounds.num_lines() as i32,
                    )
                })
            })
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while {
            let (row, lines) = cursor_row(cx);
            row < lines - 1
        } {
            assert!(
                std::time::Instant::now() < deadline,
                "output never filled the screen"
            );
            std::thread::sleep(Duration::from_millis(20));
            cx.run_until_parked();
        }
        type_line(&view, cx, &shell, 1, "cat al");
        let popup = cx.debug_bounds("completions").expect("popup not drawn");
        let cursor_top = view.read_with(cx, |view, cx| {
            let content = &view.terminal.read(cx).last_content;
            let bounds = content.terminal_bounds;
            bounds.bounds.origin.y + bounds.line_height * content.cursor.point.line.0 as f32
        });
        assert!(
            popup.bottom() <= cursor_top,
            "{popup:?} covers the cursor at {cursor_top:?}"
        );
    }

    /// keys and text the view sends to the shell, in order
    fn sent_keys(
        view: &Entity<TerminalView>,
        cx: &mut VisualTestContext,
    ) -> (Rc<RefCell<Vec<String>>>, gpui::Subscription) {
        let keys = Rc::new(RefCell::new(Vec::new()));
        let seen = keys.clone();
        let subscription = cx.update(|_, cx| {
            cx.subscribe(
                view,
                move |_, input: &crate::ui::terminal_view::TypedInput, _| match input {
                    crate::ui::terminal_view::TypedInput::Key(keystroke, _) => {
                        seen.borrow_mut().push(keystroke.key.clone())
                    }
                    crate::ui::terminal_view::TypedInput::Text(text) => {
                        seen.borrow_mut().push(text.clone())
                    }
                },
            )
        });
        (keys, subscription)
    }

    #[gpui::test]
    fn tab_before_the_popup_shows_waits_for_it(cx: &mut TestAppContext) {
        let (view, cx, mut shell) = with_shell("early", AUTO, cx);
        let (keys, _subscription) = sent_keys(&view, cx);
        // the prompt is up, then tab comes before the candidates for the line do
        type_line(&view, cx, &shell, 1, "");
        let event = line_event(&shell, 2, "cat al");
        view.update(cx, |view, cx| view.shell_event(&event, cx));
        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        assert_eq!(shell.received(), "1 accept 2 4 6 alpine/\n");
        assert!(keys.borrow().is_empty(), "tab reached the shell");
        answer(&view, cx, &shell, 2, true);

        // nothing to show for the line, so the waiting tab is the shell's
        let event = line_event(&shell, 3, "cat zzqx");
        view.update(cx, |view, cx| view.shell_event(&event, cx));
        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        assert_eq!(*keys.borrow(), ["tab"]);
    }

    #[gpui::test]
    fn tab_after_an_accept_waits_for_the_new_line(cx: &mut TestAppContext) {
        let (view, cx, mut shell) = with_shell("again", AUTO, cx);
        let (keys, _subscription) = sent_keys(&view, cx);
        type_line(&view, cx, &shell, 1, "cat al");
        cx.simulate_keystrokes("down tab");
        assert_eq!(shell.received(), "1 accept 1 4 6 alpha.txt \n");
        // the shell hasn't reported the completed line yet, the second tab waits for it
        cx.simulate_keystrokes("tab");
        assert!(keys.borrow().is_empty(), "tab reached the shell");
        type_line(&view, cx, &shell, 2, "cat alpha.txt ");
        assert!(keys.borrow().is_empty());
        answer(&view, cx, &shell, 1, true);
        assert_eq!(shell.received(), "1 accept 2 14 14 alpine/\n");
        assert!(keys.borrow().is_empty());

        // a shell that never reports the line gets the key after a while
        cx.simulate_keystrokes("tab");
        assert!(keys.borrow().is_empty());
        cx.executor().advance_clock(KEY_WAIT);
        cx.run_until_parked();
        assert_eq!(*keys.borrow(), ["tab"]);
    }

    #[gpui::test]
    fn tab_typed_ahead_of_the_shell_waits_for_its_line(cx: &mut TestAppContext) {
        let (view, cx, mut shell) = with_shell("ahead", AUTO, cx);
        type_line(&view, cx, &shell, 1, "cat al");
        assert!(!shown(&view, cx).is_empty());
        // "p" went to the pty before tab, an accept for "cat al" on the socket could pass it,
        // so tab waits for the line with the "p"
        cx.simulate_keystrokes("p tab");
        type_line(&view, cx, &shell, 2, "cat alp");
        assert_eq!(shell.received(), "1 accept 2 4 7 alpine/\n");

        // keys the shell got some other way make it turn the accept down. the selection
        // moved to a candidate stays on it, and one the newer line has nothing for goes to
        // the shell as the key it was
        let (keys, _subscription) = sent_keys(&view, cx);
        answer(&view, cx, &shell, 2, true);
        type_line(&view, cx, &shell, 3, "cat al");
        cx.simulate_keystrokes("down tab");
        assert_eq!(shell.received(), "1 accept 3 4 6 alpha.txt \n");
        type_line(&view, cx, &shell, 4, "cat alph");
        answer(&view, cx, &shell, 3, false);
        assert_eq!(shell.received(), "1 accept 4 4 8 alpha.txt \n");
        answer(&view, cx, &shell, 4, true);
        type_line(&view, cx, &shell, 5, "cat al");
        cx.simulate_keystrokes("tab");
        assert_eq!(shell.received(), "1 accept 5 4 6 alpine/\n");
        type_line(&view, cx, &shell, 6, "cat alx");
        answer(&view, cx, &shell, 5, false);
        assert_eq!(*keys.borrow(), ["tab"]);
    }

    #[gpui::test]
    fn tab_after_a_key_that_changed_nothing_accepts_on_the_line(cx: &mut TestAppContext) {
        let (view, cx, mut shell) = with_shell("still", AUTO, cx);
        type_line(&view, cx, &shell, 1, "cat al");
        // the shell reports no new line after the key, so the accept goes on the one there is
        cx.simulate_keystrokes("p tab");
        cx.executor().advance_clock(KEY_WAIT);
        cx.run_until_parked();
        assert_eq!(shell.received(), "1 accept 1 4 6 alpine/\n");
    }

    #[gpui::test]
    fn a_waiting_key_goes_before_the_next_typed_one(cx: &mut TestAppContext) {
        let (view, cx, shell) = with_shell("order", AUTO, cx);
        let (keys, _subscription) = sent_keys(&view, cx);
        type_line(&view, cx, &shell, 1, "");
        let event = line_event(&shell, 2, "cat al");
        view.update(cx, |view, cx| view.shell_event(&event, cx));
        cx.simulate_keystrokes("tab x");
        assert_eq!(*keys.borrow(), ["tab", "x"]);
    }

    #[gpui::test]
    fn popup_keys_reach_the_shell_while_it_is_hidden(cx: &mut TestAppContext) {
        let (view, cx, shell) = with_shell("hidden", AUTO, cx);
        let (keys, _subscription) = sent_keys(&view, cx);
        // a blank line shows nothing, so down is the shell's
        type_line(&view, cx, &shell, 1, " ");
        cx.simulate_keystrokes("down");
        assert_eq!(*keys.borrow(), ["down"]);
        type_line(&view, cx, &shell, 2, "cat al");
        assert!(!shown(&view, cx).is_empty());
        cx.simulate_keystrokes("down");
        assert_eq!(*keys.borrow(), ["down"], "the popup key reached the shell");
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
