//! focusable view around terminal

use std::{
    path::Path,
    time::{Duration, Instant},
};

use alacritty_terminal::selection::SelectionType;
use gpui::{
    App, ClipboardItem, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, Modifiers, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Styled,
    Subscription, Task, Window, actions, div, px,
};

use crate::{
    settings::{Settings, SmoothScrollSettings},
    terminal::{Event, MouseAction, MouseButton, Terminal},
    ui::terminal_element::TerminalElement,
};

actions!(terminal, [Copy, Paste]);

/// clipboard change made from the view, so the workspace can notify about it
pub enum ClipboardEvent {
    Copied(String),
    Pasted,
}

// alpha of the default bell flash, enough to see without hiding the text
const BELL_FLASH_OPACITY: f32 = 0.2;

/// glide of the viewport between two history offsets
#[derive(Clone, Copy, Debug)]
struct ScrollAnimation {
    from: f32,
    to: f32,
    started: Instant,
}

impl ScrollAnimation {
    /// offset right now and whether the glide is over
    fn position(&self, smooth: &SmoothScrollSettings) -> (f32, bool) {
        let t = (self.started.elapsed().as_secs_f32() * 1000. / smooth.duration).min(1.);
        let offset = self.from + (self.to - self.from) * smooth.easing.apply(t);
        (offset, t >= 1.)
    }
}

pub struct TerminalView {
    terminal: Entity<Terminal>,
    focus_handle: FocusHandle,
    scroll_px: Pixels,
    /// left button went down inside the terminal and is still held
    selecting: bool,
    /// left button went down on the scrollbar and is still held
    dragging_scrollbar: bool,
    /// cell of the last mouse report, motion is only reported when it changes
    last_mouse_cell: Option<(usize, usize)>,
    /// drives scrollbar auto hide
    last_scroll: Option<Instant>,
    /// history seen on the last render, growth means output scrolled the view
    history_size: usize,
    /// running smooth scroll, advanced on every frame
    scroll_animation: Option<ScrollAnimation>,
    /// repaints once the scrollbar should hide, replacing it cancels the old timer
    _hide_scrollbar: Task<()>,
    /// false while a blinking cursor is in its hidden phase
    cursor_on: bool,
    /// flips `cursor_on` on every tick, none while the cursor does not blink
    blink: Option<Task<()>>,
    /// font size of this tab only, none follows settings
    font_size: Option<f32>,
    /// when the bell rang, while its flash is still fading
    bell_at: Option<Instant>,
    _subscriptions: Vec<Subscription>,
}

impl TerminalView {
    /// create a view that renders and forwards input to the terminal
    pub fn new(terminal: Entity<Terminal>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.subscribe(&terminal, |this, _, event: &Event, cx| match event {
                Event::Wakeup => cx.notify(),
                Event::Bell => this.ring_bell(cx),
                _ => {}
            }),
            cx.on_focus_in(&focus_handle, window, |this, _, cx| {
                this.terminal.read(cx).focus_changed(true);
                cx.notify();
            }),
            cx.on_focus_out(&focus_handle, window, |this, _, _, cx| {
                this.terminal.read(cx).focus_changed(false);
                cx.notify();
            }),
        ];
        Self {
            terminal,
            focus_handle,
            scroll_px: px(0.),
            selecting: false,
            dragging_scrollbar: false,
            last_mouse_cell: None,
            last_scroll: None,
            history_size: 0,
            scroll_animation: None,
            _hide_scrollbar: Task::ready(()),
            cursor_on: true,
            blink: None,
            font_size: None,
            bell_at: None,
            _subscriptions: subscriptions,
        }
    }

    /// terminal model this view renders
    pub fn terminal(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    /// terminal font size of this view
    pub fn font_size(&self, cx: &App) -> f32 {
        self.font_size
            .unwrap_or(Settings::get(cx).terminal.font.size)
    }

    /// change font size of this view only, none goes back to settings
    pub fn set_font_size(&mut self, size: Option<f32>, cx: &mut Context<Self>) {
        self.font_size = size.map(Settings::clamp_font_size);
        cx.notify();
    }

    // a new ring restarts the flash
    fn ring_bell(&mut self, cx: &mut Context<Self>) {
        if Settings::get(cx).bell.visual.enable {
            self.bell_at = Some(Instant::now());
            cx.notify();
        }
    }

    // fades with each frame until the duration is over
    fn bell_flash(&mut self, cx: &App) -> Option<Hsla> {
        let rang = self.bell_at?;
        let visual = Settings::get(cx).bell.visual;
        let t = rang.elapsed().as_secs_f32() * 1000. / visual.duration;
        // a zero duration gives nan or infinity, which shows no flash
        if !visual.enable || t.is_nan() || t >= 1. {
            self.bell_at = None;
            return None;
        }
        let color = visual.color.unwrap_or_else(|| {
            let theme = self.terminal.read(cx).theme(cx);
            theme.terminal_foreground.opacity(BELL_FLASH_OPACITY)
        });
        Some(color.opacity(1. - t))
    }

    // shows the cursor and restarts its phase, so it stays visible while typing
    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_on = true;
        let settings = &Settings::get(cx).terminal;
        let interval = Duration::from_secs_f32(settings.cursor_blink_interval / 1000.);
        let timeout = Duration::from_secs_f32(settings.cursor_blink_timeout);
        self.blink = Some(cx.spawn(async move |this, cx| {
            // counted in ticks, not wall time, so the test clock drives it too
            let mut elapsed = Duration::ZERO;
            loop {
                cx.background_executor().timer(interval).await;
                elapsed += interval;
                // every blink repaints the whole window, a still cursor lets an idle one sleep
                let stop = !timeout.is_zero() && elapsed >= timeout;
                let flipped = this.update(cx, |this, cx| {
                    this.cursor_on = stop || !this.cursor_on;
                    cx.notify();
                });
                if flipped.is_err() || stop {
                    break;
                }
            }
        }));
    }

    // typing should never land while the cursor is hidden
    fn input_happened(&mut self, cx: &mut Context<Self>) {
        self.scroll_animation = None;
        if self.blink.is_some() {
            self.restart_blink(cx);
        }
    }

    /// send committed text from  input handler to pty
    pub fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if !text.is_empty() {
            self.input_happened(cx);
            self.terminal
                .update(cx, |term, _| term.input(text.to_string().into_bytes()));
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        // let input handler receive layout-dependent characters, e.g. altgr on windows
        if event.prefer_character_input && event.keystroke.key_char.is_some() {
            return;
        }
        let option_as_meta = Settings::get(cx).terminal.option_as_meta;
        if self.terminal.update(cx, |term, _| {
            term.try_keystroke(&event.keystroke, option_as_meta)
        }) {
            // input jumps to the bottom, a running glide would pull the view back up
            self.input_happened(cx);
            cx.stop_propagation();
        }
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let line_height = self
            .terminal
            .read(cx)
            .last_content
            .terminal_bounds
            .line_height;
        let multiplier = Settings::get(cx).terminal.scroll_multiplier;
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => (delta.y * multiplier) as i32,
            ScrollDelta::Pixels(delta) => {
                // accumulate touchpad pixels until they add up to whole lines
                self.scroll_px += delta.y * multiplier;
                let lines = (self.scroll_px / line_height) as i32;
                self.scroll_px -= line_height * lines as f32;
                lines
            }
        };
        if lines != 0 && event.modifiers.control {
            // one step per event, a wheel notch is several lines
            let size = self.font_size(cx) + lines.signum() as f32;
            self.set_font_size(Some(size), cx);
            return;
        }
        if lines != 0 {
            let terminal = self.terminal.read(cx);
            if terminal.owns_mouse(&event.modifiers) {
                let cell = terminal.mouse_cell(event.position);
                let button = if lines > 0 {
                    MouseButton::WheelUp
                } else {
                    MouseButton::WheelDown
                };
                self.terminal.update(cx, |term, _| {
                    for _ in 0..lines.unsigned_abs() {
                        term.report_mouse(cell, button, MouseAction::Press, &event.modifiers);
                    }
                });
                return;
            }
            if !event.modifiers.shift
                && self
                    .terminal
                    .update(cx, |term, _| term.alternate_scroll(lines))
            {
                return;
            }
            if Settings::get(cx).terminal.smooth_scroll.active() {
                let terminal = self.terminal.read(cx);
                // keep adding to the target, so fast wheel spins are not lost mid glide
                let current = self
                    .scroll_animation
                    .map_or(terminal.last_content.display_offset as f32, |a| a.to);
                let target = (current + lines as f32).clamp(0., terminal.history_size() as f32);
                self.animate_scroll(target, cx);
            } else {
                self.terminal.update(cx, |term, _| term.scroll(lines));
            }
            self.show_scrollbar(cx);
            cx.notify();
        }
    }

    // starts from where a running glide is now, so retargeting never jumps
    fn animate_scroll(&mut self, target: f32, cx: &mut Context<Self>) {
        let smooth = Settings::get(cx).terminal.smooth_scroll;
        let from = self.scroll_animation.map_or_else(
            || self.terminal.read(cx).last_content.display_offset as f32,
            |animation| animation.position(&smooth).0,
        );
        self.scroll_animation = Some(ScrollAnimation {
            from,
            to: target,
            started: Instant::now(),
        });
    }

    fn show_scrollbar(&mut self, cx: &mut Context<Self>) {
        self.last_scroll = Some(Instant::now());
        let auto_hide = Settings::get(cx).terminal.scrollbar.auto_hide;
        if auto_hide == 0. {
            return;
        }
        let delay = Duration::from_secs_f32(auto_hide);
        self._hide_scrollbar = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        });
    }

    /// false once auto hide kicked in
    pub(crate) fn scrollbar_visible(&self, cx: &App) -> bool {
        let auto_hide = Settings::get(cx).terminal.scrollbar.auto_hide;
        auto_hide == 0.
            || self.dragging_scrollbar
            || self
                .last_scroll
                .is_some_and(|at| at.elapsed() < Duration::from_secs_f32(auto_hide))
    }

    /// click on the scrollbar jumps there and starts a drag
    pub fn scrollbar_down(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.dragging_scrollbar = true;
        self.scrollbar_drag(offset, cx);
    }

    /// move the viewport with the thumb while the button is held
    pub fn scrollbar_drag(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.dragging_scrollbar {
            if Settings::get(cx).terminal.smooth_scroll.active() {
                self.animate_scroll(offset as f32, cx);
            } else {
                self.terminal.update(cx, |term, _| term.scroll_to(offset));
            }
            self.show_scrollbar(cx);
            cx.notify();
        }
    }

    /// tell the program about the mouse when it asked for it, true when it did
    fn report_mouse(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        action: MouseAction,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        let terminal = self.terminal.read(cx);
        if !terminal.owns_mouse(modifiers) {
            return false;
        }
        let cell = terminal.mouse_cell(position);
        // programs only care about the cell, not every pixel of motion inside it
        if action == MouseAction::Motion && self.last_mouse_cell == Some(cell) {
            return true;
        }
        self.last_mouse_cell = Some(cell);
        self.terminal.update(cx, |term, _| {
            term.report_mouse(cell, button, action, modifiers)
        });
        true
    }

    /// a press goes to the program when it asked for the mouse, otherwise a left click starts a
    /// selection: single click, double selects words, triple lines, shift extends. a right click
    /// copies the selection
    pub fn mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if let Some(button) = MouseButton::from_gpui(event.button)
            && self.report_mouse(
                event.position,
                button,
                MouseAction::Press,
                &event.modifiers,
                cx,
            )
        {
            return;
        }
        if event.button == gpui::MouseButton::Right && Settings::get(cx).terminal.right_click_copy {
            self.copy_selection(cx);
            return;
        }
        if event.button != gpui::MouseButton::Left {
            return;
        }
        let ty = match event.click_count {
            0 | 1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        self.terminal.update(cx, |term, _| {
            if event.modifiers.shift && ty == SelectionType::Simple {
                term.extend_selection(event.position);
            } else {
                term.start_selection(event.position, ty);
            }
        });
        self.selecting = true;
        cx.notify();
    }

    /// extend the selection while the button is held, even outside the terminal area,
    /// or report the motion to a program that asked for it
    pub fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.selecting {
            self.terminal
                .update(cx, |term, _| term.extend_selection(event.position));
            cx.notify();
            return;
        }
        if self.dragging_scrollbar {
            return;
        }
        let button = match event.pressed_button {
            Some(button) => match MouseButton::from_gpui(button) {
                Some(button) => button,
                None => return,
            },
            None => MouseButton::None,
        };
        self.report_mouse(
            event.position,
            button,
            MouseAction::Motion,
            &event.modifiers,
            cx,
        );
    }

    /// finish the drag, the selection stays until the next click or input
    pub fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.selecting && Settings::get(cx).terminal.select_to_copy {
            self.copy_selection(cx);
        }
        let was_local = self.selecting || self.dragging_scrollbar;
        self.selecting = false;
        self.dragging_scrollbar = false;
        if !was_local && let Some(button) = MouseButton::from_gpui(event.button) {
            self.report_mouse(
                event.position,
                button,
                MouseAction::Release,
                &event.modifiers,
                cx,
            );
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = copied_text(self.terminal.read(cx), cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            cx.emit(ClipboardEvent::Copied(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.input_happened(cx);
            self.terminal.update(cx, |term, _| term.paste(&text));
            cx.emit(ClipboardEvent::Pasted);
        }
    }

    /// type paths dropped from other apps, quoted for the shell
    pub fn drop_paths(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let mut text: String = paths
            .paths()
            .iter()
            .map(|path| shell_quote(path))
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            return;
        }
        // a trailing space lets the next dropped path or typed argument follow right away
        text.push(' ');
        self.input_happened(cx);
        self.terminal.update(cx, |term, _| term.paste(&text));
    }
}

/// selected text as it goes into the clipboard, none when nothing is selected
pub fn copied_text(terminal: &Terminal, cx: &App) -> Option<String> {
    let text = terminal.selection_text()?;
    if !Settings::get(cx).terminal.copy_trim_trailing_whitespace {
        return Some(text);
    }
    Some(trim_trailing_whitespace(&text))
}

fn trim_trailing_whitespace(text: &str) -> String {
    text.split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n")
}

/// path as one shell word, single quoted when it has characters the shell would expand
fn shell_quote(path: &Path) -> String {
    let path = path.to_string_lossy();
    let plain = !path.is_empty()
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+,:@%=".contains(c));
    if plain {
        path.into_owned()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

impl EventEmitter<ClipboardEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window) && window.is_window_active();
        if let Some(animation) = self.scroll_animation {
            let smooth = Settings::get(cx).terminal.smooth_scroll;
            let (offset, done) = animation.position(&smooth);
            self.terminal
                .update(cx, |term, _| term.scroll_to(offset.round() as usize));
            if done {
                self.scroll_animation = None;
            } else {
                window.request_animation_frame();
            }
        }
        // synced here rather than on every wakeup, so busy output does not take the term lock
        // from the io thread more than once a frame. prepaint syncs again only after a resize
        let history_size = self.terminal.update(cx, |term, _| {
            term.sync();
            term.last_content.history_size
        });
        // output pushing lines into history scrolls the view too
        if history_size > self.history_size {
            self.show_scrollbar(cx);
        }
        self.history_size = history_size;
        let blinking = focused
            && Settings::get(cx)
                .terminal
                .cursor_blink
                .active(self.terminal.read(cx).last_content.cursor_blinking);
        if !blinking {
            self.blink = None;
            self.cursor_on = true;
        } else if self.blink.is_none() {
            self.restart_blink(cx);
        }
        let bell_flash = self.bell_flash(cx);
        if bell_flash.is_some() {
            window.request_animation_frame();
        }
        div()
            .id("terminal-view")
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("Terminal")
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_key_down(cx.listener(Self::key_down))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .child(TerminalElement::new(
                self.terminal.clone(),
                cx.entity(),
                self.focus_handle.clone(),
                focused,
                self.cursor_on,
                self.scrollbar_visible(cx),
                bell_flash,
            ))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, TestAppContext, VisualTestContext};

    use super::*;
    use crate::terminal::TerminalBuilder;

    /// focused view over a real shell, with terminal settings overridden by `json`
    fn focused_view<'a>(
        json: &str,
        cx: &'a mut TestAppContext,
    ) -> (Entity<TerminalView>, &'a mut VisualTestContext) {
        let settings = Settings::parse(json).unwrap();
        cx.update(|cx| {
            cx.set_global(settings.clone());
            cx.set_global(crate::theme::Theme::default());
        });
        let builder = TerminalBuilder::new(&settings.terminal, settings.default_profile(), 0)
            .expect("failed to spawn shell");
        let terminal = cx.new(|_| builder.into_terminal());
        let (view, cx) = cx.add_window_view(|window, cx| TerminalView::new(terminal, window, cx));
        cx.update(|window, cx| {
            window.activate_window();
            view.read(cx).focus_handle.clone().focus(window, cx);
        });
        cx.run_until_parked();
        (view, cx)
    }

    #[gpui::test]
    fn cursor_blinks_and_typing_shows_it(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(
            r#"{"terminal": {"cursor_blink": "on", "cursor_blink_interval": 400, "cursor_blink_timeout": 0}}"#,
            cx,
        );
        assert!(view.read_with(cx, |view, _| view.blink.is_some() && view.cursor_on));
        cx.executor().advance_clock(Duration::from_millis(450));
        cx.run_until_parked();
        assert!(!view.read_with(cx, |view, _| view.cursor_on));
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.cursor_on));
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        assert!(!view.read_with(cx, |view, _| view.cursor_on));

        cx.simulate_keystrokes("a");
        assert!(view.read_with(cx, |view, _| view.cursor_on));
    }

    #[gpui::test]
    fn cursor_stops_blinking_after_timeout_and_typing_restarts_it(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(
            r#"{"terminal": {"cursor_blink": "on", "cursor_blink_interval": 400, "cursor_blink_timeout": 1}}"#,
            cx,
        );
        // 3 ticks reach the timeout, the last one leaves the cursor shown
        cx.executor().advance_clock(Duration::from_millis(1250));
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.cursor_on));
        cx.executor().advance_clock(Duration::from_secs(3));
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.cursor_on));

        cx.simulate_keystrokes("a");
        cx.executor().advance_clock(Duration::from_millis(450));
        cx.run_until_parked();
        assert!(!view.read_with(cx, |view, _| view.cursor_on));
    }

    #[gpui::test]
    fn cursor_does_not_blink_when_off(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"terminal": {"cursor_blink": "off"}}"#, cx);
        cx.executor().advance_clock(Duration::from_secs(3));
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.blink.is_none() && view.cursor_on));
    }

    #[gpui::test]
    fn ctrl_wheel_changes_font_size_of_the_view(cx: &mut TestAppContext) {
        let (view, cx) = focused_view("{}", cx);
        let settings_size = cx.update(|_, cx| Settings::get(cx).terminal.font.size);
        let wheel = |y: f32, control: bool| ScrollWheelEvent {
            delta: ScrollDelta::Lines(gpui::point(0., y)),
            modifiers: Modifiers {
                control,
                ..Modifiers::default()
            },
            ..ScrollWheelEvent::default()
        };
        let font_size =
            |cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.font_size(cx));

        view.update_in(cx, |view, window, cx| {
            view.scroll_wheel(&wheel(1., true), window, cx)
        });
        assert_eq!(font_size(cx), settings_size + 1.);
        view.update_in(cx, |view, window, cx| {
            view.scroll_wheel(&wheel(-1., true), window, cx);
            view.scroll_wheel(&wheel(-1., true), window, cx);
        });
        assert_eq!(font_size(cx), settings_size - 1.);
        // plain wheel scrolls, size stays
        view.update_in(cx, |view, window, cx| {
            view.scroll_wheel(&wheel(1., false), window, cx)
        });
        assert_eq!(font_size(cx), settings_size - 1.);
        assert_eq!(
            cx.update(|_, cx| Settings::get(cx).terminal.font.size),
            settings_size
        );
    }

    /// select the whole screen once the shell echoed "picked", and put "before" in the clipboard
    fn select_echoed(view: &Entity<TerminalView>, cx: &mut VisualTestContext) {
        view.update(cx, |view, cx| {
            view.terminal
                .update(cx, |term, _| term.input(b"echo picked\r".to_vec()))
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        while !view.update(cx, |view, cx| {
            view.terminal.update(cx, |term, _| {
                term.sync();
                term.select_all();
                term.selection_text()
                    .is_some_and(|text| text.contains("picked"))
            })
        }) {
            assert!(Instant::now() < deadline, "shell never echoed");
            std::thread::sleep(Duration::from_millis(50));
        }
        cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
    }

    /// release the left button after a drag over the whole screen, returns the clipboard text
    fn drag_and_release(view: &Entity<TerminalView>, cx: &mut VisualTestContext) -> Option<String> {
        select_echoed(view, cx);
        view.update(cx, |view, cx| {
            view.selecting = true;
            view.mouse_up(
                &MouseUpEvent {
                    button: gpui::MouseButton::Left,
                    ..MouseUpEvent::default()
                },
                cx,
            );
        });
        cx.read_from_clipboard().and_then(|item| item.text())
    }

    #[gpui::test]
    fn select_to_copy_copies_on_release(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"terminal": {"select_to_copy": true}}"#, cx);
        let copied = drag_and_release(&view, cx);
        assert!(copied.is_some_and(|text| text.contains("picked")));
    }

    #[gpui::test]
    fn release_does_not_copy_without_select_to_copy(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"terminal": {"select_to_copy": false}}"#, cx);
        assert_eq!(drag_and_release(&view, cx).as_deref(), Some("before"));
    }

    /// select the whole screen once the shell echoed, then right click, returns the clipboard text
    fn select_and_right_click(
        view: &Entity<TerminalView>,
        cx: &mut VisualTestContext,
    ) -> Option<String> {
        select_echoed(view, cx);
        view.update(cx, |view, cx| {
            view.mouse_down(
                &MouseDownEvent {
                    button: gpui::MouseButton::Right,
                    ..MouseDownEvent::default()
                },
                cx,
            )
        });
        cx.read_from_clipboard().and_then(|item| item.text())
    }

    #[gpui::test]
    fn right_click_copies_the_selection(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"terminal": {"right_click_copy": true}}"#, cx);
        let copied = select_and_right_click(&view, cx);
        assert!(copied.is_some_and(|text| text.contains("picked")));
    }

    #[gpui::test]
    fn right_click_does_not_copy_without_right_click_copy(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"terminal": {"right_click_copy": false}}"#, cx);
        assert_eq!(select_and_right_click(&view, cx).as_deref(), Some("before"));
    }

    #[gpui::test]
    fn scroll_multiplier_scales_wheel_steps(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"terminal": {"scroll_multiplier": 0.5}}"#, cx);
        let start = view.read_with(cx, |view, cx| view.font_size(cx));
        // ctrl-wheel changes the size by one per event that adds up to a whole line
        let wheel = ScrollWheelEvent {
            delta: ScrollDelta::Lines(gpui::point(0., 1.)),
            modifiers: Modifiers {
                control: true,
                ..Modifiers::default()
            },
            ..ScrollWheelEvent::default()
        };
        view.update_in(cx, |view, window, cx| view.scroll_wheel(&wheel, window, cx));
        assert_eq!(view.read_with(cx, |view, cx| view.font_size(cx)), start);

        cx.update(|_, cx| {
            let mut settings = Settings::get(cx).clone();
            settings.terminal.scroll_multiplier = 3.;
            cx.set_global(settings);
        });
        view.update_in(cx, |view, window, cx| view.scroll_wheel(&wheel, window, cx));
        assert_eq!(
            view.read_with(cx, |view, cx| view.font_size(cx)),
            start + 1.
        );
    }

    #[test]
    fn trailing_whitespace_is_trimmed_per_line() {
        assert_eq!(
            trim_trailing_whitespace("a b  \n\tc\t \n  \nd"),
            "a b\n\tc\n\nd"
        );
        assert_eq!(trim_trailing_whitespace("line\n"), "line\n");
    }

    /// line the shell printed with a trailing space and tab, as copied with `json` settings
    fn copied_padded_line(json: &str, cx: &mut TestAppContext) -> String {
        let (view, cx) = focused_view(json, cx);
        // a tab leaves a tab cell behind, so alacritty copies it with the space before it
        view.update(cx, |view, cx| {
            view.terminal.update(cx, |term, _| {
                term.input(b"printf 'padded \\t\\n'\r".to_vec())
            })
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let copied = view.update(cx, |view, cx| {
                view.terminal.update(cx, |term, _| {
                    term.sync();
                    term.select_all();
                });
                copied_text(view.terminal.read(cx), cx)
            });
            if let Some(line) = copied
                .as_deref()
                .and_then(|text| text.lines().find(|line| line.starts_with("padded")))
            {
                return line.to_string();
            }
            assert!(Instant::now() < deadline, "shell never printed");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[gpui::test]
    fn copy_trims_trailing_whitespace(cx: &mut TestAppContext) {
        let json = r#"{"terminal": {"copy_trim_trailing_whitespace": true}}"#;
        assert_eq!(copied_padded_line(json, cx), "padded");
    }

    #[gpui::test]
    fn copy_keeps_trailing_whitespace_without_trimming(cx: &mut TestAppContext) {
        let json = r#"{"terminal": {"copy_trim_trailing_whitespace": false}}"#;
        assert_eq!(copied_padded_line(json, cx), "padded \t");
    }

    #[gpui::test]
    fn bell_starts_a_flash_only_when_visual_bell_is_on(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"bell": {"visual": {"enable": true}}}"#, cx);
        let terminal = view.read_with(cx, |view, _| view.terminal.clone());
        terminal.update(cx, |_, cx| cx.emit(Event::Bell));
        assert!(view.read_with(cx, |view, _| view.bell_at.is_some()));

        cx.update(|_, cx| {
            let mut settings = Settings::get(cx).clone();
            settings.bell.visual.enable = false;
            cx.set_global(settings);
        });
        view.update(cx, |view, _| view.bell_at = None);
        terminal.update(cx, |_, cx| cx.emit(Event::Bell));
        assert!(view.read_with(cx, |view, _| view.bell_at.is_none()));
    }

    #[gpui::test]
    fn bell_flash_fades_and_ends(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(
            r##"{"bell": {"visual": {"enable": true, "color": "#ff0000", "duration": 200}}}"##,
            cx,
        );
        let flash = |cx: &mut VisualTestContext| view.update(cx, |view, cx| view.bell_flash(cx));
        view.update(cx, |view, _| view.bell_at = Some(Instant::now()));
        let first = flash(cx).expect("flash right after the bell");
        assert!(first.a > 0.5 && first.a <= 1.);
        view.update(cx, |view, _| {
            view.bell_at = Some(Instant::now() - Duration::from_millis(150))
        });
        let later = flash(cx).expect("flash before the duration ends");
        assert!(later.a < first.a && later.a > 0.);
        view.update(cx, |view, _| {
            view.bell_at = Some(Instant::now() - Duration::from_millis(250))
        });
        assert_eq!(flash(cx), None);
        assert!(view.read_with(cx, |view, _| view.bell_at.is_none()));
    }

    #[test]
    fn dropped_paths_are_quoted_only_when_needed() {
        assert_eq!(
            shell_quote(Path::new("/home/me/a-b_c.txt")),
            "/home/me/a-b_c.txt"
        );
        assert_eq!(shell_quote(Path::new("/tmp/my dir")), "'/tmp/my dir'");
        assert_eq!(shell_quote(Path::new("/tmp/it's $x")), "'/tmp/it'\\''s $x'");
        assert_eq!(shell_quote(Path::new("/tmp/été")), "'/tmp/été'");
    }
}
