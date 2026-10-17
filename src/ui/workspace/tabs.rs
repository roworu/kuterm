//! opening tabs: spawning their shell, wiring their terminal and view events, holding input
//! back until a fresh shell is ready, switching and reordering tabs

use super::*;

// a fresh shell prints startup output before its prompt, so typed input waits for a short
// quiet gap. writing sooner lets the tty echo the input before the shell reads it
pub(super) const SHELL_READY_SETTLE: Duration = Duration::from_millis(50);

/// spawn `profile` in a pty, with shell integration and the remote socket added
pub(super) fn spawn_profile(
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
    // only kuterm's own zsh script talks to it
    #[cfg(unix)]
    let bridge = if settings.autocomplete.enable && settings.shell_integration.inject {
        crate::completion::Bridge::new()
            .inspect_err(|error| eprintln!("autocomplete is off in this tab: {error}"))
            .ok()
    } else {
        None
    };
    #[cfg(unix)]
    if let Some(bridge) = &bridge {
        let path = bridge.path().to_string_lossy().into_owned();
        profile
            .env
            .insert(crate::completion::SOCKET_ENV.into(), path);
    }
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = TerminalBuilder::new(&settings.terminal, &profile, bounds, window_id)?;
    #[cfg(unix)]
    if let Some(bridge) = bridge {
        builder.terminal.set_completion_bridge(bridge);
    }
    Ok(builder)
}

impl Workspace {
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

    /// open a tab with the command of `link` typed, in its folder when that exists
    pub fn open_run_link(&mut self, link: RunLink, window: &mut Window, cx: &mut Context<Self>) {
        let mut profile = Settings::get(cx).default_profile().clone();
        if let Some(cwd) = link.cwd.filter(|cwd| cwd.is_dir()) {
            profile.working_directory = Some(cwd);
        }
        let pane = self.focused;
        let before = self.pane(pane).map_or(0, |pane| pane.tabs.len());
        self.add_profile_tab(pane, &profile, true, window, cx);
        if self.pane(pane).is_some_and(|pane| pane.tabs.len() > before) {
            self.input_to(
                pane,
                before,
                InputSource::Automation,
                link.command.into_bytes(),
                cx,
            );
        }
    }

    /// open a tab with `profile` in pane `pane_id`, switching to it when `activate`
    pub(super) fn add_profile_tab(
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
    pub(super) fn add_builder_tab(
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
    pub(super) fn attach_terminal(
        &mut self,
        pane_id: usize,
        terminal: Entity<Terminal>,
        profile: &Profile,
        activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| TerminalView::new(terminal.clone(), window, cx));

        let remote_id = terminal.read(cx).pane_id;
        let pane = remote_id;
        EventBus::publish(BusEvent::PaneOpened { pane });
        let subscription = cx.subscribe_in(&terminal, window, {
            let view = view.clone();
            move |this, terminal, event: &Event, window, cx| match event {
                Event::TitleChanged => {
                    let title = terminal.read(cx).title(&Settings::get(cx).default_title);
                    EventBus::publish(BusEvent::TitleChanged { pane, title });
                    this.refresh_titles_later()
                }
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
                Event::Bell => {
                    EventBus::publish(BusEvent::Bell { pane });
                    this.ring_bell(&view, window, cx)
                }
                Event::ClipboardAsk { id, text } => {
                    this.ask_clipboard(&view, *id, text.clone(), cx)
                }
                Event::CommandFinished { duration, exit } => {
                    let block = terminal.update(cx, |terminal, _| {
                        terminal.blocks().back().map(|block| block.id)
                    });
                    EventBus::publish(BusEvent::CommandFinished {
                        pane,
                        block,
                        exit: *exit,
                        seconds: duration.as_secs_f64(),
                    });
                    this.attention_ended(&view, cx);
                    this.command_finished(&view, *duration, *exit, window, cx)
                }
                Event::CommandStarted { block } => {
                    let mode = Settings::get(cx).shell_integration.mark_sources;
                    let theme = Theme::get(cx).clone();
                    let command = terminal.update(cx, |terminal, _| {
                        let blocks = terminal.blocks();
                        let found = blocks.iter().find(|found| found.id == *block);
                        let (command, sources) = found
                            .map(|found| (found.command.clone(), found.source))
                            .unwrap_or_default();
                        if let Some(mark) =
                            source_marks::source_mark(sources, &command, mode, &theme)
                        {
                            terminal
                                .decorate(|decorations| decorations.gutter.insert(*block, mark));
                        }
                        command
                    });
                    EventBus::publish(BusEvent::CommandStarted {
                        pane,
                        block: *block,
                        command,
                    });
                }
                Event::CwdChanged(cwd) => EventBus::publish(BusEvent::CwdChanged {
                    pane,
                    cwd: cwd.clone(),
                }),
                Event::BusyChanged(busy) => {
                    EventBus::publish(BusEvent::Progress { pane, busy: *busy });
                    if *busy {
                        this.attention_ended(&view, cx)
                    } else {
                        this.agent_stopped(&view, window, cx)
                    }
                }
                Event::Notify { title, body } => {
                    EventBus::publish(BusEvent::Notify {
                        pane,
                        title: title.clone(),
                        body: body.clone(),
                    });
                    this.program_notification(&view, title, body, window, cx)
                }
                // the view shows completions
                Event::Shell(_) => {}
                Event::QueueChanged => cx.notify(),
                Event::QueuePaused { exit } => {
                    let code = exit.map_or(String::new(), |exit| format!(" with {exit}"));
                    this.show_notification(
                        format!("a command failed{code}, the queue waits for \"resume queue\""),
                        Some(view.clone()),
                        cx,
                    );
                    cx.notify();
                }
            }
        });
        let input_subscription = cx.subscribe(&view, |this, view, input: &TypedInput, cx| {
            this.broadcast_input(&view, input, cx)
        });
        let clipboard_subscription = cx.subscribe_in(
            &view,
            window,
            |this, view, event: &ViewEvent, window, cx| match event {
                ViewEvent::Copied(text) => this.notify_copied(text, cx),
                ViewEvent::Pasted => this.notify_pasted(cx),
                ViewEvent::ConfirmPaste(text) => {
                    this.open_confirm_paste(view.clone(), text.clone(), window, cx)
                }
                ViewEvent::HintPicked(text) => this.hint_picked(view, text.clone(), window, cx),
                ViewEvent::Scrolled(delta) => this.sync_scroll(view, *delta, cx),
                ViewEvent::OpenFileLine(link) => this.open_file_line(view, link, window, cx),
            },
        );

        let settings = Settings::get(cx);
        // every block is empty before the first refresh, so all icons sit at the start
        let icons = settings
            .tab_title
            .iter()
            .filter(|block| **block == TabTitleBlock::Icon);
        let icon_at = vec![0; icons.count()];
        let icon = profile
            .icon
            .clone()
            .unwrap_or_else(|| settings.icons.default.clone());
        let Some(pane) = self.pane_mut(pane_id) else {
            return;
        };
        pane.tabs.push(Tab {
            view,
            profile: profile.name.clone(),
            title: String::new(),
            icon,
            icon_at,
            ports_at: Vec::new(),
            profile_icon: profile.icon.clone(),
            ready: false,
            ready_task: Task::ready(()),
            pending_input: Vec::new(),
            pending_source: InputSource::Macro,
            bell: false,
            temp_file: None,
            custom_title: None,
            pinned: false,
            agent: false,
            attention: Default::default(),
            group: None,
            project: None,
            context: PaneContext::default(),
            process: String::new(),
            asked: None,
            pane_id: remote_id,
            name: None,
            color: None,
            remote_allowed: false,
            _subscriptions: [subscription, clipboard_subscription, input_subscription],
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
    pub(super) fn ring_bell(
        &mut self,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bell = &Settings::get(cx).bell;
        let flash = bell.visual.enable;
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
        // the flash of a background tab is drawn by the workspace only
        if flash {
            cx.notify();
        }
    }

    /// the shell of a tab printed something, wait for it to quiet down before calling it ready
    pub(super) fn schedule_ready(
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
    pub(super) fn mark_ready(&mut self, view: &Entity<TerminalView>, cx: &mut Context<Self>) {
        let Some(tab) = self.tab_mut(view) else {
            return;
        };
        tab.ready = true;
        if tab.pending_input.is_empty() {
            return;
        }
        let input = std::mem::take(&mut tab.pending_input);
        let source = tab.pending_source;
        let terminal = tab.view.read(cx).terminal().clone();
        terminal.update(cx, |terminal, _| terminal.input(source, input));
    }

    /// write input to the tab at `ix` in pane `pane_id`, holding it back until a fresh shell is ready
    pub(super) fn input_to(
        &mut self,
        pane_id: usize,
        ix: usize,
        source: InputSource,
        input: Vec<u8>,
        cx: &mut Context<Self>,
    ) {
        let tab = &mut self.pane_mut(pane_id).expect("pane from caller").tabs[ix];
        if tab.ready {
            let terminal = tab.view.read(cx).terminal().clone();
            if !terminal.update(cx, |terminal, _| terminal.input(source, input)) {
                self.input_refused(source, cx);
            }
        } else {
            // a shell still starting echoes raw input from the tty, so wait for its first output
            tab.pending_input.extend_from_slice(&input);
            tab.pending_source = source;
        }
    }

    /// say why input from `source` went nowhere
    pub(super) fn input_refused(&mut self, source: InputSource, cx: &mut Context<Self>) {
        let text = format!(
            "{} input was not typed: a program asks for a password",
            source.label()
        );
        self.show_notification(text, None, cx);
    }

    pub(super) fn activate_tab(
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
        EventBus::publish(BusEvent::FocusChanged {
            pane: view.read(cx).terminal().read(cx).pane_id,
        });
        view.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// move a tab within pane `pane_id`, keeping the active tab on its view
    pub(super) fn move_tab(
        &mut self,
        pane_id: usize,
        from: usize,
        to: usize,
        cx: &mut Context<Self>,
    ) {
        {
            let Some(pane) = self.pane_mut(pane_id) else {
                return;
            };
            if from >= pane.tabs.len() || to >= pane.tabs.len() {
                return;
            }
            // pinned tabs stay in front of the others
            let pinned = pane.tabs.iter().filter(|tab| tab.pinned).count();
            let to = if pane.tabs[from].pinned {
                to.min(pinned - 1)
            } else {
                to.max(pinned)
            };
            if from == to {
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
        // a group's tabs stay together, a tab dragged out of its run goes back to it
        self.regroup(pane_id);
        // titles show the tab number, which changed for the shifted tabs
        self.refresh_titles();
        cx.notify();
    }

    /// take the tab at `ix` out of pane `pane_id`, the bool is true when the pane emptied
    pub(super) fn take_tab(&mut self, pane_id: usize, ix: usize) -> Option<(Tab, bool)> {
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
        let window_id = window.window_handle().window_id().as_u64();
        let builder = spawn_profile(&profile, TerminalBounds::default(), window_id, cx)
            .map_err(|error| format!("failed to spawn terminal: {error:#}"))?;
        self.add_builder_tab(self.focused, builder, &profile, true, window, cx);
        Ok(())
    }

    /// type `text` of a run link into the active tab
    pub fn send_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let (pane, ix) = (self.focused, self.focused_pane().active);
        self.input_to(
            pane,
            ix,
            InputSource::Automation,
            text.as_bytes().to_vec(),
            cx,
        );
    }

    /// write `text` to a temp file and open it with `scrollback_pager` in a new tab
    pub(super) fn open_scrollback(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    /// open a tab with the named profile in pane `pane_id`, notifying when there is none
    pub(super) fn add_named_profile_tab(
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

    /// switch to the tab with this view, wherever it lives
    pub(super) fn activate_id(
        &mut self,
        id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((pane, ix)) = self.find_id(id) {
            self.activate_tab(pane, ix, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::test_support::*;

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

    #[gpui::test]
    fn disabled_next_tab_ignores_ctrl_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open_with(cx, 3, r#"{"next_tab": null}"#);
        assert_eq!(keys(&ws, cx, "ctrl-tab"), 2);
        assert_eq!(keys(&ws, cx, "alt-1"), 0);
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

    #[gpui::test]
    fn close_tab_after_background_tab_closes_it(cx: &mut TestAppContext) {
        let (ws, cx) = open(cx, 1);
        let before = views(&ws, cx);
        run_actions(&ws, cx, r#""new_background_tab", "close_tab""#);
        assert_eq!(views(&ws, cx), before);
        assert_eq!(current(&ws, cx), 0);
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
    fn profile_whose_program_fails_to_start_opens_no_tab(cx: &mut TestAppContext) {
        let (ws, cx) = open_profiles(
            cx,
            r#"{"name": "default", "default": true, "command": "system"},
               {"name": "broken", "command": {"program": "/nonexistent/kuterm-no-such-shell"}}"#,
        );
        let before = views(&ws, cx);
        run_actions(&ws, cx, r#"{"new_tab_with_profile": "broken"}"#);
        assert_eq!(views(&ws, cx), before);
        assert_eq!(current(&ws, cx), 0);
    }
}
