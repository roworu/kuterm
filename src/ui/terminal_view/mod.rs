//! focusable view around terminal

mod bell;
mod clipboard;
mod completions;
mod links;
mod mouse;
mod scroll;

use std::{
    path::Path,
    time::{Duration, Instant},
};

use alacritty_terminal::{selection::SelectionType, term::TermMode};
use gpui::{
    App, AppContext, BorrowAppContext, ClipboardItem, Context, DismissEvent, Entity, EventEmitter,
    ExternalPaths, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement, KeyContext,
    KeyDownEvent, KeyUpEvent, Modifiers, ModifiersChangedEvent, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Styled,
    Subscription, Task, Window, actions, div, px,
};

use crate::{
    settings::{BellTarget, PatternAction, Settings, SmoothScrollSettings},
    terminal::{
        Event, InputSource, KeyEventKind, Link, MouseAction, MouseButton, Popover, Terminal,
        TextPatterns, ViKey, foreground_process, parse_file_line, paste_check, resolve_file,
        snippet,
    },
    ui::{
        hints::{HintKey, Hints},
        search_bar::SearchBar,
        terminal_element::TerminalElement,
        workspace::open_urls,
    },
};

pub use clipboard::copied_text;
use clipboard::trim_copied;
pub use links::open_link;
use scroll::ScrollAnimation;

actions!(
    terminal,
    [
        Copy,
        Paste,
        Search,
        ShowHints,
        ToggleViMode,
        Complete,
        CompletionNext,
        CompletionPrev,
        CompletionAccept,
        CompletionDismiss
    ]
);

/// something the view did that the workspace shows or asks about
pub enum ViewEvent {
    Copied(String),
    Pasted,
    /// text that looks unsafe to paste, the workspace asks first
    ConfirmPaste(String),
    /// hint picked in hints mode, a command asking for `{hint}` runs on it
    HintPicked(String),
    /// the user scrolled the history this many lines, positive up
    Scrolled(i32),
    /// a link names a line of a file, the workspace looks for an editor to open it in
    OpenFileLine(Link),
}

/// input the view wrote to its terminal, so panes typed into together get the same
#[derive(Clone, Debug)]
pub enum TypedInput {
    /// a key, each terminal turns it into bytes in its own key mode
    Key(gpui::Keystroke, KeyEventKind),
    /// text from the input method
    Text(String),
    Paste(String),
}

/// `links.patterns` with their regexes built, shared by every terminal
#[derive(Default)]
struct LinkPatterns(TextPatterns);

impl gpui::Global for LinkPatterns {}

/// `hints.patterns` with their regexes built, shared by every terminal
#[derive(Default)]
struct HintPatterns(TextPatterns);

impl gpui::Global for HintPatterns {}

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
    /// shows the peek of the `file:line` under the mouse once it rests there
    _peek: Task<()>,
    /// cell the peek waits on or shows, so moving inside it starts nothing new
    peek_cell: Option<(i32, usize)>,
    /// false while a blinking cursor is in its hidden phase
    cursor_on: bool,
    /// flips `cursor_on` on every tick, none while the cursor does not blink
    blink: Option<Task<()>>,
    /// font size of this tab only, none follows settings
    font_size: Option<f32>,
    /// when the bell rang, while its flash is still fading
    bell_at: Option<Instant>,
    /// open search bar, none while it is closed
    search_bar: Option<(Entity<SearchBar>, Subscription)>,
    /// link under the mouse while the link modifier is held
    hovered_link: Option<Link>,
    /// last mouse position over the terminal and the modifiers held then
    last_mouse: Option<(Point<Pixels>, Modifiers)>,
    /// labels shown in hints mode, none outside of it
    hints: Option<Hints>,
    /// match of the hint picked since hints mode last started, for commands asking `{hint}`
    picked_hint: Option<String>,
    /// completion popup for the shell's command line
    completions: completions::CompletionMenu,
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
                Event::Shell(event) => this.shell_event(event, cx),
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
            _peek: Task::ready(()),
            peek_cell: None,
            cursor_on: true,
            blink: None,
            font_size: None,
            bell_at: None,
            search_bar: None,
            hovered_link: None,
            last_mouse: None,
            hints: None,
            picked_hint: None,
            completions: Default::default(),
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
            self.terminal.update(cx, |term, _| {
                term.input(InputSource::Keyboard, text.to_string().into_bytes())
            });
            cx.emit(TypedInput::Text(text.to_string()));
        }
    }

    /// open the search bar, or focus it again when it is open
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bar = match &self.search_bar {
            Some((bar, _)) => bar.clone(),
            None => {
                let mouse = self.last_mouse.map(|(position, _)| position);
                let block = self
                    .terminal
                    .update(cx, |terminal, _| terminal.block_for_search(mouse));
                let bar = cx.new(|cx| SearchBar::new(self.terminal.clone(), block, cx));
                let subscription =
                    cx.subscribe_in(&bar, window, |this, _, _: &DismissEvent, window, cx| {
                        this.close_search(window, cx)
                    });
                self.search_bar = Some((bar.clone(), subscription));
                bar
            }
        };
        bar.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// close the search bar, dropping its highlights
    pub fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_bar.take().is_some() {
            self.terminal
                .update(cx, |terminal, _| terminal.end_search());
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    /// open search bar, exposed for tests
    #[cfg(test)]
    pub(crate) fn search_bar(&self) -> Option<&Entity<SearchBar>> {
        self.search_bar.as_ref().map(|(bar, _)| bar)
    }

    fn search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        self.open_search(window, cx);
    }

    /// label every pattern match on screen, typing a label runs its action
    pub fn show_hints(&mut self, cx: &mut Context<Self>) {
        cx.default_global::<HintPatterns>();
        let terminal = self.terminal.clone();
        let links = cx.update_global(|patterns: &mut HintPatterns, cx| {
            patterns.0.update(&Settings::get(cx).hints.patterns);
            terminal.update(cx, |terminal, _| terminal.visible_matches(&mut patterns.0))
        });
        // a pick from an older hints session is not what a command starting hints mode wants
        self.picked_hint = None;
        self.hints = Hints::new(links, &Settings::get(cx).hints.alphabet);
        cx.notify();
    }

    /// match of the hint picked since hints mode last started
    pub fn picked_hint(&self) -> Option<&str> {
        self.picked_hint.as_deref()
    }

    /// first `file:line` the hint patterns find in the output of the last command of `terminal`
    pub fn last_output_location(terminal: &Entity<Terminal>, cx: &mut App) -> Option<Link> {
        cx.default_global::<HintPatterns>();
        cx.update_global(|patterns: &mut HintPatterns, cx| {
            patterns.0.update(&Settings::get(cx).hints.patterns);
            terminal.read(cx).last_output_location(&mut patterns.0)
        })
    }

    fn show_hints_action(&mut self, _: &ShowHints, _: &mut Window, cx: &mut Context<Self>) {
        self.show_hints(cx);
    }

    /// start or end keyboard selection mode
    pub fn toggle_vi_mode(&mut self, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, _| terminal.toggle_vi_mode());
        cx.notify();
    }

    fn toggle_vi_mode_action(&mut self, _: &ToggleViMode, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_vi_mode(cx);
    }

    // with no shell to complete for, the key reaches the terminal
    fn complete(&mut self, _: &Complete, _: &mut Window, cx: &mut Context<Self>) {
        if !self.open_completions(cx) {
            cx.propagate();
        }
    }

    // the popup keys are plain keys the shell needs, a keymap may bind them anywhere
    fn completion_key(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
        run: impl FnOnce(&mut Self, &mut Context<Self>),
    ) {
        if self.completions_shown(window, cx) {
            run(self, cx);
        } else {
            cx.propagate();
        }
    }

    fn completion_next(&mut self, _: &CompletionNext, window: &mut Window, cx: &mut Context<Self>) {
        self.completion_key(window, cx, |this, cx| this.move_completion(1, cx));
    }

    fn completion_prev(&mut self, _: &CompletionPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.completion_key(window, cx, |this, cx| this.move_completion(-1, cx));
    }

    fn completion_accept(
        &mut self,
        _: &CompletionAccept,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.completion_key(window, cx, Self::accept_completion);
    }

    fn completion_dismiss(
        &mut self,
        _: &CompletionDismiss,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.completion_key(window, cx, Self::dismiss_completions);
    }

    /// the completion popup shows, so its keys go to it
    fn completions_shown(&self, window: &Window, cx: &App) -> bool {
        let content = &self.terminal.read(cx).last_content;
        self.completions.visible()
            && self.focus_handle.is_focused(window)
            && self.hints.is_none()
            && content.display_offset == 0
            && !content.mode.intersects(TermMode::VI | TermMode::ALT_SCREEN)
    }

    /// shown hints, exposed for tests
    #[cfg(test)]
    pub(crate) fn hints(&self) -> Option<&Hints> {
        self.hints.as_ref()
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // keys typed into the search bar bubble up through here
        if !self.focus_handle.is_focused(window) {
            return;
        }
        // hints mode takes every key, none reaches the program
        if let Some(hints) = &mut self.hints {
            cx.stop_propagation();
            match hints.key(&event.keystroke.key) {
                HintKey::Pending => {}
                HintKey::Picked(link) => {
                    self.hints = None;
                    self.picked_hint = Some(link.target.clone());
                    cx.emit(ViewEvent::HintPicked(link.target.clone()));
                    self.run_link(&link, cx);
                }
                HintKey::Cancel => self.hints = None,
            }
            cx.notify();
            return;
        }
        // vi mode takes every key too
        if self
            .terminal
            .read(cx)
            .last_content
            .mode
            .contains(TermMode::VI)
        {
            cx.stop_propagation();
            match self
                .terminal
                .update(cx, |terminal, _| terminal.vi_key(&event.keystroke))
            {
                ViKey::Handled | ViKey::Exit => {}
                ViKey::Copy(text) => {
                    let text = trim_copied(text, cx);
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    cx.emit(ViewEvent::Copied(text));
                }
                ViKey::Search => self.open_search(window, cx),
            }
            cx.notify();
            return;
        }
        // let input handler receive layout-dependent characters, e.g. altgr on windows
        if event.prefer_character_input && event.keystroke.key_char.is_some() {
            return;
        }
        let option_as_meta = Settings::get(cx).terminal.option_as_meta;
        let kind = if event.is_held {
            KeyEventKind::Repeat
        } else {
            KeyEventKind::Press
        };
        if self.terminal.update(cx, |term, _| {
            term.try_keystroke(
                &event.keystroke,
                kind,
                option_as_meta,
                InputSource::Keyboard,
            )
        }) {
            // input jumps to the bottom, a running glide would pull the view back up
            self.input_happened(cx);
            cx.stop_propagation();
            cx.emit(TypedInput::Key(event.keystroke.clone(), kind));
        }
    }

    // releases only reach programs that asked for them through kitty's keyboard protocol
    fn key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let option_as_meta = Settings::get(cx).terminal.option_as_meta;
        if self.terminal.update(cx, |term, _| {
            term.try_keystroke(
                &event.keystroke,
                KeyEventKind::Release,
                option_as_meta,
                InputSource::Keyboard,
            )
        }) {
            cx.stop_propagation();
            cx.emit(TypedInput::Key(
                event.keystroke.clone(),
                KeyEventKind::Release,
            ));
        }
    }
}

impl EventEmitter<ViewEvent> for TerminalView {}
impl EventEmitter<TypedInput> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    // "vi_mode", "alt_screen" and "completion" let keybindings apply only then
    fn key_context(&self, completions: bool, cx: &App) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        context.add("Terminal");
        let mode = self.terminal.read(cx).last_content.mode;
        if mode.contains(TermMode::VI) {
            context.add("vi_mode");
        }
        if mode.contains(TermMode::ALT_SCREEN) {
            context.add("alt_screen");
        }
        if completions {
            context.add("completion");
        }
        context
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
        // textures of images the program dropped go with them
        for image in self.terminal.read(cx).take_dropped_images() {
            cx.drop_image(image, Some(window));
        }
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
        // output and scrolling move the text under a hovered link
        if self.hovered_link.is_some() {
            self.update_hovered_link(cx);
        }
        let bell_flash = self.bell_flash(cx);
        if bell_flash.is_some() {
            window.request_animation_frame();
        }
        let completions = self.completions_shown(window, cx);
        let popup = completions
            .then(|| self.render_completions(window.viewport_size().height, cx))
            .flatten();
        div()
            .id("terminal-view")
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context(self.key_context(completions, cx))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::search))
            .on_action(cx.listener(Self::show_hints_action))
            .on_action(cx.listener(Self::toggle_vi_mode_action))
            .on_action(cx.listener(Self::complete))
            .on_action(cx.listener(Self::completion_next))
            .on_action(cx.listener(Self::completion_prev))
            .on_action(cx.listener(Self::completion_accept))
            .on_action(cx.listener(Self::completion_dismiss))
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .relative()
            .child(TerminalElement::new(
                self.terminal.clone(),
                cx.entity(),
                self.focus_handle.clone(),
                focused,
                self.cursor_on,
                self.scrollbar_visible(cx),
                // the workspace flashes the tab, or the pane edges while the tab bar is hidden
                bell_flash.filter(|_| Settings::get(cx).bell.visual.target == BellTarget::Terminal),
                self.hovered_link
                    .as_ref()
                    .map(|link| link.ranges().cloned().collect())
                    .unwrap_or_default(),
                self.hints
                    .as_ref()
                    .map(|hints| {
                        hints
                            .shown()
                            .map(|(link, label)| (*link.range.start(), label.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
            ))
            .children(
                self.search_bar
                    .as_ref()
                    .map(|(bar, _)| div().absolute().top_2().right_4().child(bar.clone())),
            )
            .children(popup)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, TestAppContext, VisualTestContext};

    use super::*;
    use crate::terminal::{TerminalBounds, TerminalBuilder};

    /// focused view over a real shell, with terminal settings overridden by `json`
    pub(super) fn focused_view<'a>(
        json: &str,
        cx: &'a mut TestAppContext,
    ) -> (Entity<TerminalView>, &'a mut VisualTestContext) {
        let settings = Settings::parse(json).unwrap();
        cx.update(|cx| {
            cx.set_global(settings.clone());
            cx.set_global(crate::theme::Theme::default());
        });
        let builder = TerminalBuilder::new(
            &settings.terminal,
            settings.default_profile(),
            TerminalBounds::default(),
            0,
        )
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

    /// visible text of the view's terminal, one line per row
    pub(super) fn screen(view: &Entity<TerminalView>, cx: &mut VisualTestContext) -> String {
        view.update(cx, |view, cx| {
            view.terminal.update(cx, |terminal, _| {
                terminal.sync();
                let mut text = String::new();
                for indexed in &terminal.last_content.cells {
                    if indexed.point.column.0 == 0 && !text.is_empty() {
                        text.push('\n');
                    }
                    text.push(indexed.cell.c);
                }
                text
            })
        })
    }

    pub(super) fn wait_for(
        cx: &mut VisualTestContext,
        what: &str,
        mut done: impl FnMut(&mut VisualTestContext) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done(cx) {
            assert!(Instant::now() < deadline, "timed out: {what}");
            std::thread::sleep(Duration::from_millis(20));
            cx.run_until_parked();
        }
    }

    // raw input bytes a reader collects before it ends
    const RAW_BYTES: usize = 40;

    /// in the view's shell: run `setup`, then save RAW_BYTES raw input bytes as hex into the
    /// returned file. waits until the terminal modes pass `ready`
    pub(super) fn raw_reader(
        view: &Entity<TerminalView>,
        cx: &mut VisualTestContext,
        name: &str,
        setup: &str,
        ready: impl Fn(TermMode) -> bool,
    ) -> std::path::PathBuf {
        let file =
            std::env::temp_dir().join(format!("kuterm_view_raw_{name}_{}", std::process::id()));
        let started = file.with_extension("ready");
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(&started);
        let (f, r) = (file.display(), started.display());
        let command = format!(
            "{setup}stty raw -echo; touch {r}; dd bs=1 count={RAW_BYTES} 2>/dev/null | od -An -v -tx1 | tr -d ' \\n' > {f}.tmp; mv {f}.tmp {f}; stty sane\r"
        );
        view.update(cx, |view, cx| {
            view.terminal
                .update(cx, |terminal, _| terminal.type_text(command.as_bytes()))
        });
        wait_for(cx, "reader never started", |_| started.exists());
        wait_for(cx, "modes never reached the view", |cx| {
            view.update(cx, |view, cx| {
                view.terminal.update(cx, |terminal, _| {
                    terminal.sync();
                    ready(terminal.last_content.mode)
                })
            })
        });
        // let dd start reading
        std::thread::sleep(Duration::from_millis(100));
        std::fs::remove_file(&started).ok();
        file
    }

    /// fill the reader up and return what it got before, as text
    pub(super) fn raw_read(
        view: &Entity<TerminalView>,
        cx: &mut VisualTestContext,
        file: &Path,
    ) -> String {
        let fill = "x".repeat(RAW_BYTES);
        view.update(cx, |view, cx| {
            view.terminal
                .update(cx, |terminal, _| terminal.type_text(fill.as_bytes()))
        });
        wait_for(cx, "no reader output", |_| file.exists());
        let hex = std::fs::read_to_string(file).unwrap();
        std::fs::remove_file(file).ok();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
            .collect();
        String::from_utf8_lossy(&bytes)
            .trim_end_matches('x')
            .to_string()
    }

    #[gpui::test]
    fn held_keys_are_typed_as_repeats(cx: &mut TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        let (view, cx) = focused_view("{}", cx);
        let typed = Rc::new(RefCell::new(Vec::new()));
        let seen = typed.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&view, move |_, input: &TypedInput, _| {
                if let TypedInput::Key(keystroke, kind) = input {
                    seen.borrow_mut().push((keystroke.key.clone(), *kind));
                }
            })
        });
        for is_held in [false, true, true] {
            cx.simulate_event(KeyDownEvent {
                keystroke: gpui::Keystroke::parse("ctrl-a").unwrap(),
                is_held,
                prefer_character_input: false,
            });
        }
        cx.run_until_parked();
        let a = || "a".to_string();
        assert_eq!(
            *typed.borrow(),
            [
                (a(), KeyEventKind::Press),
                (a(), KeyEventKind::Repeat),
                (a(), KeyEventKind::Repeat)
            ]
        );
    }

    #[gpui::test]
    fn search_key_again_refocuses_the_open_bar(cx: &mut TestAppContext) {
        let (view, cx) = focused_view("{}", cx);
        view.update_in(cx, |view, window, cx| view.open_search(window, cx));
        let bar = view.read_with(cx, |view, _| view.search_bar().cloned().unwrap());
        // typing in the terminal again, then searching keeps the query in the same bar
        view.update_in(cx, |view, window, cx| {
            view.focus_handle.focus(window, cx);
            view.open_search(window, cx)
        });
        let again = view.read_with(cx, |view, _| view.search_bar().cloned().unwrap());
        assert_eq!(again.entity_id(), bar.entity_id());
        assert!(cx.update(|window, cx| bar.focus_handle(cx).is_focused(window)));
        view.update_in(cx, |view, window, cx| view.close_search(window, cx));
        assert!(view.read_with(cx, |view, _| view.search_bar().is_none()));
        assert!(cx.update(|window, cx| view.read(cx).focus_handle.is_focused(window)));
    }

    /// a view over a shell that printed three hint matches, hints labelled from "ab"
    pub(super) fn view_with_hints(
        cx: &mut TestAppContext,
    ) -> (Entity<TerminalView>, &mut VisualTestContext) {
        let json = r#"{"hints": {"alphabet": "ab", "patterns": [{"name": "tag", "regex": "zz[0-9]", "action": "copy"}]}}"#;
        let (view, cx) = focused_view(json, cx);
        view.update(cx, |view, cx| {
            view.terminal.update(cx, |terminal, _| {
                terminal.type_text(b"printf '%s\\n' 'z''z1 z''z2 z''z3'\r")
            })
        });
        wait_for(cx, "matches never printed", |cx| {
            screen(&view, cx).contains("zz1 zz2 zz3")
        });
        view.update(cx, |view, cx| view.show_hints(cx));
        assert!(view.read_with(cx, |view, _| view.hints().is_some()));
        (view, cx)
    }

    #[gpui::test]
    fn two_letter_hint_waits_for_its_second_letter(cx: &mut TestAppContext) {
        let (view, cx) = view_with_hints(cx);
        cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
        // three matches and two letters: labels "aa", "ab" and "ba"
        cx.simulate_keystrokes("a");
        assert!(view.read_with(cx, |view, _| view.hints().is_some()));
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("before")
        );
        cx.simulate_keystrokes("b");
        assert!(view.read_with(cx, |view, _| view.hints().is_none()));
        let picked = view.read_with(cx, |view, _| view.picked_hint().map(str::to_string));
        assert_eq!(picked.as_deref(), Some("zz2"));
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("zz2")
        );
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
}
