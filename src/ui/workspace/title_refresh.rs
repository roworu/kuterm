//! tab titles, the window title and the status bar, built off the main thread from the blocks
//! settings name, on a poll and after output

use super::*;

// programs, folders and command output change without events, so titles are polled
pub(super) const TITLE_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

// output also refreshes titles, so short commands like "sudo dnf check-update" show up
// between polls. busy output asks many times a second, so its refreshes are spaced by this
pub(super) const TITLE_REFRESH_MIN_GAP: Duration = Duration::from_millis(200);

/// the blocks settings name and what every tab shows, read on the main thread for a build
pub(super) struct TitleSnapshot {
    blocks: Vec<TabTitleBlock>,
    window_blocks: Vec<TabTitleBlock>,
    /// left and right of the status bar, none while it is off
    status_blocks: Option<[Vec<TabTitleBlock>; 2]>,
    icon_settings: TabIconSettings,
    icons: TabIcons,
    /// index into `tabs` of the active tab of the focused pane
    active: usize,
    tabs: Vec<TabSnapshot>,
}

/// one tab of a `TitleSnapshot`
struct TabSnapshot {
    id: EntityId,
    inputs: TitleInputs,
    /// folder the shell reported, read only while tabs are grouped by project
    cwd: Option<Option<PathBuf>>,
    /// in front of its pane. only these get their context and pty state read again, the
    /// others keep what was read while they were shown
    visible: bool,
    /// a program asked for a secret when the pty state was last read
    secret: bool,
}

/// title of one tab as the background build made it
pub(super) struct TabTitle {
    title: String,
    icon_at: Vec<usize>,
    ports_at: Vec<(Range<usize>, u16)>,
    icon: String,
    /// an ai agent runs in front of the shell
    agent: bool,
    /// git project the tab is in, found while tabs are grouped by project
    project: Option<PathBuf>,
    /// where the tab runs, read for visible tabs only
    context: Option<PaneContext>,
    /// name of the program in front
    process: String,
    /// tty settings and stopped jobs, read for visible tabs only
    pty: Option<PtyState>,
}

/// what one build gives: every tab's title, and the window title and status bar of the
/// active tab
struct Built {
    titles: Vec<(EntityId, TabTitle)>,
    window_title: String,
    status: [Vec<Piece>; 2],
}

/// the task building titles: after `TITLE_REFRESH_INTERVAL`, right away when `true` comes on
/// the returned sender, and soon, spaced by `TITLE_REFRESH_MIN_GAP`, when `false` comes
pub(super) fn spawn_title_refresh(
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> (UnboundedSender<bool>, Task<()>) {
    let (refresh, mut refresh_rx) = unbounded();
    let task = cx.spawn_in(window, async move |this, cx| {
        loop {
            let Ok(snapshot) = this.update(cx, |this, cx| this.title_snapshot(cx)) else {
                break;
            };
            let built_at = Instant::now();
            let built = build(snapshot, cx.background_executor()).await;
            let Ok(()) = this.update_in(cx, |this, window, cx| this.set_titles(built, window, cx))
            else {
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
    (refresh, task)
}

/// build every tab of `snapshot` off the main thread. /proc reads and exec blocks may block,
/// and every tab builds on its own, so a slow exec block holds back only its tab
async fn build(snapshot: TitleSnapshot, executor: &gpui::BackgroundExecutor) -> Built {
    let TitleSnapshot {
        blocks,
        window_blocks,
        status_blocks,
        icon_settings,
        icons,
        active,
        tabs,
    } = snapshot;
    let shared = Arc::new((blocks, window_blocks, status_blocks, icon_settings, icons));
    let builds = tabs.into_iter().enumerate().map(|(ix, tab)| {
        let shared = shared.clone();
        executor.spawn(async move { build_tab(&shared, tab, ix == active) })
    });
    let mut built = Built {
        titles: Vec::new(),
        window_title: String::new(),
        status: Default::default(),
    };
    for (id, title, window_title, status) in futures::future::join_all(builds).await {
        if let Some(window_title) = window_title {
            built.window_title = window_title;
        }
        if let Some(status) = status {
            built.status = status;
        }
        built.titles.push((id, title));
    }
    built
}

type SharedBlocks = (
    Vec<TabTitleBlock>,
    Vec<TabTitleBlock>,
    Option<[Vec<TabTitleBlock>; 2]>,
    TabIconSettings,
    TabIcons,
);

/// title of one tab, and the window title and status bar when it is the active one
fn build_tab(
    shared: &SharedBlocks,
    tab: TabSnapshot,
    active: bool,
) -> (EntityId, TabTitle, Option<String>, Option<[Vec<Piece>; 2]>) {
    let (blocks, window_blocks, status_blocks, icon_settings, icons) = shared;
    let TabSnapshot {
        id,
        mut inputs,
        cwd,
        visible,
        secret,
    } = tab;
    // read once, the window title, tab title and icon all need it
    let process = foreground_process(inputs.shell_pid);
    let pty = visible.then(|| {
        inputs.context = PaneContext::read(process.as_ref());
        let in_front = process.as_ref().map_or(inputs.shell_pid, |p| p.pid);
        PtyState {
            modes: pty_modes(in_front),
            shell_in_front: in_front == inputs.shell_pid,
            stopped: stopped_jobs(inputs.shell_pid),
        }
    });
    let process_name = process.as_ref().map(|p| p.name.clone()).unwrap_or_default();
    // the git project of the folder the shell reported, or of the program's folder, only
    // while tabs are grouped by project
    let project = cwd.and_then(|cwd| {
        cwd.or_else(|| process.as_ref()?.cwd.clone())
            .as_deref()
            .and_then(crate::terminal::git_root)
    });
    let window_title =
        active.then(|| tab_title::build_title(window_blocks, &inputs, process.as_ref()));
    let status = status_blocks.as_ref().filter(|_| active).map(|sides| {
        sides
            .clone()
            .map(|side| tab_title::build_pieces(&side, &inputs, process.as_ref()))
    });
    let (title, icon_at, ports_at) = tab_title::build_title(blocks, &inputs, process.as_ref());
    let secret = pty.as_ref().map_or(secret, PtyState::asking_secret);
    let (icon, agent) = tab_icon::tab_icon(
        icon_settings,
        icons,
        inputs.profile_icon.as_deref(),
        process,
    );
    // a lock while a password is asked, then the tab's own icon
    let icon = if secret {
        waiting::LOCK_ICON.to_string()
    } else {
        icon
    };
    // the window title and status bar are plain text, so their icons go in as glyphs
    let window_title =
        window_title.map(|(text, icon_at, _)| tab_title::insert_icons(text, &icon_at, &icon));
    let status = status.map(|sides| sides.map(|side| tab_title::icons_as_text(side, &icon)));
    let title = TabTitle {
        title,
        icon_at,
        ports_at,
        icon,
        agent,
        project,
        context: visible.then_some(inputs.context),
        process: process_name,
        pty,
    };
    (id, title, window_title, status)
}

/// true when some title or the status bar shows cpu, memory or ports. asking the sampler keeps
/// it running, so it is only asked then
pub(super) fn shows_samples(settings: &Settings) -> bool {
    let status_bar = &settings.status_bar;
    let status = [&status_bar.left, &status_bar.right]
        .into_iter()
        .filter(|_| status_bar.enable)
        .flatten();
    settings
        .tab_title
        .iter()
        .chain(&settings.window_title)
        .chain(status)
        .any(TabTitleBlock::sampled)
}

/// columns and rows of a terminal
pub(super) fn terminal_size(terminal: &Terminal) -> (usize, usize) {
    let bounds = terminal.last_content.terminal_bounds;
    (bounds.num_columns(), bounds.num_lines())
}

impl Workspace {
    pub(super) fn refresh_titles(&self) {
        self.refresh_titles.unbounded_send(true).ok();
    }

    pub(super) fn refresh_titles_later(&self) {
        self.refresh_titles.unbounded_send(false).ok();
    }

    pub(super) fn title_snapshot(&self, cx: &App) -> TitleSnapshot {
        let settings = Settings::get(cx);
        let status_bar = &settings.status_bar;
        let status_blocks = status_bar
            .enable
            .then(|| [status_bar.left.clone(), status_bar.right.clone()]);
        let sampled = shows_samples(settings);
        let memory_unit = StatusBarState::get(cx).memory_unit(status_bar.memory_unit);
        let clock = title_sources::clock(&status_bar.clock_format);
        let mut tabs = Vec::new();
        let mut active = 0;
        let by_project = settings.tab_groups.by_project;
        for pane in self.layout.panes() {
            for (ix, tab) in pane.tabs.iter().enumerate() {
                if pane.id == self.focused && ix == pane.active {
                    active = tabs.len();
                }
                let terminal = tab.view.read(cx).terminal().read(cx);
                tabs.push(TabSnapshot {
                    id: tab.view.entity_id(),
                    inputs: TitleInputs {
                        number: ix + 1,
                        shell_pid: terminal.shell_pid,
                        title: terminal.title(&settings.default_title),
                        profile_icon: tab.profile_icon.clone(),
                        progress: terminal.progress(),
                        sample: sampled
                            .then(|| ProcessSampler::shared().sample(terminal.shell_pid))
                            .flatten(),
                        total: None,
                        memory_unit,
                        clock: clock.clone(),
                        size: terminal_size(terminal),
                        context: tab.context.clone(),
                        queued: terminal.queue().entries().len(),
                        xoff: terminal.xoff_paused(),
                        stopped: terminal
                            .stopped_jobs()
                            .iter()
                            .map(|job| job.name.clone())
                            .collect(),
                    },
                    cwd: by_project.then(|| terminal.reported_cwd().map(Path::to_path_buf)),
                    visible: ix == pane.active,
                    secret: terminal.asking_secret(),
                });
            }
        }
        if sampled {
            let total = TreeSample::total(tabs.iter().filter_map(|tab| tab.inputs.sample.as_ref()));
            for tab in &mut tabs {
                tab.inputs.total = Some(total.clone());
            }
        }
        TitleSnapshot {
            blocks: settings.tab_title.clone(),
            window_blocks: settings.window_title.clone(),
            status_blocks,
            icon_settings: settings.icons.clone(),
            icons: TabIcons::get(cx).clone(),
            active,
            tabs,
        }
    }

    fn set_titles(&mut self, built: Built, window: &mut Window, cx: &mut Context<Self>) {
        let Built {
            titles,
            window_title,
            status,
        } = built;
        let window_title = if window_title.is_empty() {
            Settings::get(cx).default_title.clone()
        } else {
            tab_title::spin(&window_title, self.spinner_frame)
        };
        // linux can't read the title back, so the last one is kept here
        if self.window_title != window_title {
            window.set_window_title(&window_title);
            self.window_title = window_title;
        }
        let mut changed = self.status != status;
        self.status = status;
        for (id, built) in titles {
            // tabs closed while building are skipped
            let Some((pane_id, ix)) = self.find_id(id) else {
                continue;
            };
            let tab = &mut self
                .pane_mut(pane_id)
                .expect("pane id comes from the layout")
                .tabs[ix];
            tab.agent = built.agent;
            tab.project = built.project;
            if let Some(context) = built.context
                && tab.context != context
            {
                let summary = context.summary();
                if summary != tab.context.summary() {
                    EventBus::publish(BusEvent::ContextChanged {
                        pane: tab.pane_id,
                        context: summary,
                    });
                }
                tab.context = context;
            }
            let tab_pane_id = tab.pane_id;
            let terminal = tab.view.read(cx).terminal().clone();
            if let Some(pty) = built.pty {
                let modes = pty.modes;
                let modes_changed = terminal.read(cx).pty_modes() != modes;
                if terminal.update(cx, |terminal, _| terminal.set_pty_state(pty)) {
                    changed = true;
                    if modes_changed {
                        self.publish_pty_modes(tab_pane_id, modes);
                    }
                }
            }
            let tab = &mut self
                .pane_mut(pane_id)
                .expect("pane id comes from the layout")
                .tabs[ix];
            let icon = built.icon;
            if tab.process != built.process {
                tab.process = built.process;
                EventBus::publish(BusEvent::ProcessChanged {
                    pane: tab.pane_id,
                    name: tab.process.clone(),
                });
            }
            // input does not redraw the tab bar, so the mark of an answered wait goes here
            changed |= tab
                .attention
                .forget_answered(tab.view.read(cx).terminal().read(cx).inputs());
            if tab.title != built.title
                || tab.icon_at != built.icon_at
                || tab.ports_at != built.ports_at
                || tab.icon != icon
            {
                tab.title = built.title;
                tab.icon_at = built.icon_at;
                tab.ports_at = built.ports_at;
                tab.icon = icon;
                changed = true;
            }
        }
        changed |= self.group_by_project(cx);
        // timed entries come due without any output, the title refresh is the clock
        self.tick_queues(cx);
        self.check_waiting(window, cx);
        if changed {
            cx.notify();
        }
        self.start_spinner(window, cx);
    }

    /// turn busy progress spinners until no title has one
    pub(super) fn start_spinner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.spinning || !self.has_spinner() {
            return;
        }
        self.spinning = true;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(tab_title::SPINNER_INTERVAL)
                    .await;
                let Ok(true) = this.update_in(cx, |this, window, cx| {
                    if !this.has_spinner() {
                        this.spinning = false;
                        return false;
                    }
                    this.spinner_frame += 1;
                    this.window_title = tab_title::spin(&this.window_title, this.spinner_frame);
                    window.set_window_title(&this.window_title);
                    cx.notify();
                    true
                }) else {
                    break;
                };
            }
        })
        .detach();
    }

    pub(super) fn has_spinner(&self) -> bool {
        tab_title::has_spinner(&self.window_title)
            || self
                .status
                .iter()
                .flatten()
                .any(|piece| tab_title::has_spinner(&piece.text))
            || self.layout.panes().iter().any(|pane| {
                pane.tabs
                    .iter()
                    .any(|tab| tab_title::has_spinner(&tab.title))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::test_support::*;

    #[test]
    fn only_visible_tabs_read_their_context_and_pty_state() {
        let settings = Settings::default();
        let shared: SharedBlocks = (
            vec![TabTitleBlock::Number],
            Vec::new(),
            None,
            settings.icons.clone(),
            TabIcons::default(),
        );
        let tab = |visible, secret| TabSnapshot {
            id: EntityId::from(1u64),
            inputs: TitleInputs {
                number: 1,
                shell_pid: std::process::id(),
                ..TitleInputs::default()
            },
            cwd: None,
            visible,
            secret,
        };
        let (_, shown, ..) = build_tab(&shared, tab(true, true), false);
        assert!(shown.context.is_some() && shown.pty.is_some());
        // a fresh read wins over the old secret, this process asks for none
        assert_ne!(shown.icon, waiting::LOCK_ICON);
        let (_, hidden, ..) = build_tab(&shared, tab(false, true), false);
        assert!(hidden.context.is_none() && hidden.pty.is_none());
        // a hidden tab keeps the secret read while it was shown
        assert_eq!(hidden.icon, waiting::LOCK_ICON);
        assert_eq!(hidden.title, "1");
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
}
