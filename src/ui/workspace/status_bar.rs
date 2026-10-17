//! clicks on the status bar: ports open in the browser, memory switches its unit, totals list
//! every tab

use super::*;

use gpui::{Context, Div, Stateful, Window, div, prelude::*, px, relative, rems};

use super::{
    Workspace,
    tab_bar::*,
    tab_title::{self, Piece},
};
use crate::{
    settings::Settings, terminal::CPU_HISTORY, theme::Theme, ui::terminal_view::open_link,
};

impl Workspace {
    /// a click on a status bar piece: ports open in the browser, totals list every tab
    pub(super) fn status_clicked(
        &mut self,
        click: Click,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match click {
            Click::Port(port) => open_link(format!("http://localhost:{port}"), cx),
            Click::Memory => {
                let default = Settings::get(cx).status_bar.memory_unit;
                let state = cx.default_global::<StatusBarState>();
                state.next_memory_unit(default);
                state.save();
                self.refresh_titles();
            }
            Click::TotalCpu | Click::TotalMemory | Click::TotalNetwork => {
                self.open_usage_picker(click, window, cx)
            }
            Click::Queue => self.open_queue_editor(window, cx),
        }
    }

    /// palette listing cpu, memory or traffic of every tab, most first, picking one switches
    /// to it
    pub(super) fn open_usage_picker(
        &mut self,
        click: Click,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let unit = StatusBarState::get(cx).memory_unit(Settings::get(cx).status_bar.memory_unit);
        let mut rows = Vec::new();
        for pane in self.layout.panes() {
            for (ix, tab) in pane.tabs.iter().enumerate() {
                let pid = tab.view.read(cx).terminal().read(cx).shell_pid;
                let sample = ProcessSampler::shared().sample(pid).unwrap_or_default();
                let (amount, text) = match click {
                    Click::TotalCpu => (
                        f64::from(sample.cpu_percent),
                        tab_title::percent(sample.cpu_percent),
                    ),
                    Click::TotalNetwork => (
                        (sample.received_per_sec + sample.sent_per_sec) as f64,
                        tab_title::network(&sample),
                    ),
                    _ => (
                        sample.rss_bytes as f64,
                        tab_title::bytes(sample.rss_bytes, unit),
                    ),
                };
                rows.push((amount, text, pane.id, ix));
            }
        }
        rows.sort_by(|a, b| b.0.total_cmp(&a.0));
        let rows = rows.into_iter().map(|(_, text, pane, ix)| (text, pane, ix));
        self.open_tab_rows(rows.collect(), window, cx);
    }
}

// status bar text size in rems, smaller than the tabs so the bar stays quiet
pub(super) const STATUS_TEXT_SIZE: f32 = 0.75;

/// cpu graph as thin bars, newest on the right. older bars fade, busier ones turn from green
/// to yellow to red, and slots without samples yet are a flat line
pub(super) fn graph_bars(graph: &[f32], theme: &Theme) -> Div {
    let empty = CPU_HISTORY.saturating_sub(graph.len());
    let newest = &graph[graph.len().saturating_sub(CPU_HISTORY)..];
    let bar = || div().flex_none().w(px(3.)).rounded_t(px(1.));
    let bars = newest.iter().enumerate().map(|(ix, &value)| {
        let color = match value {
            v if v < 0.5 => theme.ansi[2],
            v if v < 0.8 => theme.ansi[3],
            _ => theme.ansi[1],
        };
        let age = (empty + ix) as f32 / (CPU_HISTORY - 1).max(1) as f32;
        bar()
            .h(relative(value.max(0.08)))
            .bg(color.opacity(0.35 + 0.65 * age))
    });
    div()
        .flex()
        .items_end()
        .gap(px(1.))
        .h(rems(0.9))
        .px(px(2.))
        .pt(px(2.))
        .rounded_sm()
        // a faint track, so the graph reads as one widget even while idle
        .bg(theme.text_muted.opacity(0.1))
        .children((0..empty).map(|_| bar().h(px(1.)).bg(theme.border)))
        .children(bars)
}

impl Workspace {
    /// profile list opened with a right click on "+", picking one opens a tab with it
    /// bar under the panes with the status pieces of the active tab on both sides
    pub(super) fn render_status_bar(&self, cx: &Context<Self>) -> Stateful<Div> {
        let theme = Theme::get(cx);
        let side = |name: &'static str, pieces: &[Piece]| {
            let items = pieces
                .iter()
                .enumerate()
                // a space between two ports is a gap here
                .filter(|(_, piece)| !piece.text.trim().is_empty())
                .map(|(ix, piece)| {
                    let item = div().id((name, ix)).flex_none().whitespace_nowrap();
                    let item = match &piece.graph {
                        Some(graph) => item.child(graph_bars(graph, theme)),
                        None => item.child(tab_title::spin(&piece.text, self.spinner_frame)),
                    };
                    match piece.click {
                        Some(click) => item
                            .debug_selector(move || format!("status-{click:?}"))
                            .cursor_pointer()
                            .hover(|item| item.underline())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.status_clicked(click, window, cx)
                            })),
                        None => item,
                    }
                });
            div()
                .flex()
                .items_center()
                .gap_3()
                .min_w_0()
                .overflow_hidden()
                .children(items)
        };
        div()
            .id("status-bar")
            .debug_selector(|| "status-bar".into())
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap_6()
            .w_full()
            .h(rems(STATUS_BAR_HEIGHT))
            .px_3()
            .bg(theme.tab_bar_background)
            .border_t_1()
            .border_color(theme.border)
            .text_size(rems(STATUS_TEXT_SIZE))
            .text_color(theme.text_muted)
            .hover(|bar| bar.text_color(theme.text))
            .child(side("status-left", &self.status[0]))
            .child(side("status-right", &self.status[1]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn status_bar_sits_below_the_panes_only_when_enabled(cx: &mut TestAppContext) {
        let json = r#"{"status_bar": {"enable": false}}"#;
        let (_, cx) = open_with_settings(cx, 1, "{}", Settings::parse(json).unwrap());
        assert!(cx.debug_bounds("status-bar").is_none());
        let (ws, cx) = open_with_status(cx, "[]", r#"[{"text": "left"}]"#, r#"["number"]"#);
        let bar = cx.debug_bounds("status-bar").unwrap();
        assert!(cx.debug_bounds("pane-terminal-0").unwrap().bottom() <= bar.top());
        wait_until(cx, "status never built", |cx| {
            ws.update(cx, |ws, _| !ws.status[1].is_empty())
        });
        let texts = ws.update(cx, |ws, _| {
            ws.status
                .clone()
                .map(|side| side.into_iter().map(|piece| piece.text).collect::<Vec<_>>())
        });
        assert_eq!(texts, [vec!["left"], vec!["1"]]);
    }

    #[gpui::test]
    fn listening_port_shows_in_status_title_and_palette(cx: &mut TestAppContext) {
        let (ws, cx) = open_with_status(cx, r#"["ports"]"#, r#"["ports"]"#, r#"["total_cpu"]"#);
        send_to(
            &ws,
            cx,
            0,
            "exec python3 -m http.server 0 --bind 127.0.0.1\r",
        );
        let port = status_port(&ws, cx);
        // the tab title holds the same port, with its range for clicks
        let text = format!(":{port}");
        wait_until(cx, "port never showed in the tab title", |cx| {
            ws.update(cx, |ws, _| ws.focused_pane().tabs[0].title == text)
        });
        let ports_at = ws.update(cx, |ws, _| ws.focused_pane().tabs[0].ports_at.clone());
        assert_eq!(ports_at, [(0..text.len(), port)]);
        assert!(
            cx.debug_bounds(format!("status-Port({port})").leak())
                .is_some()
        );
        assert!(cx.debug_bounds("status-TotalCpu").is_some());
        // from another tab, the palette goes back to the one serving it
        ws.update_in(cx, |ws, window, cx| ws.add_tab(window, cx));
        cx.run_until_parked();
        assert_eq!(current(&ws, cx), 1);
        ws.update_in(cx, |ws, window, cx| {
            ws.toggle_command_palette(&ToggleCommandPalette, window, cx)
        });
        type_keys(cx, "s e r v i n g");
        assert_eq!(
            palette_names(&ws, cx),
            [format!("tabs: go to tab serving :{port}")]
        );
        type_keys(cx, "enter");
        assert!(!palette_open(&ws, cx));
        assert_eq!(current(&ws, cx), 0);
        assert!(ws.update(cx, |ws, _| ws.recent_commands.is_empty()));
    }

    #[gpui::test]
    fn total_click_lists_tabs_most_first(cx: &mut TestAppContext) {
        let (ws, cx) = open_with_status(cx, "[]", "[]", r#"["total_memory"]"#);
        ws.update_in(cx, |ws, window, cx| ws.add_tab(window, cx));
        // the first tab holds far more memory than a bare shell
        send_to(
            &ws,
            cx,
            0,
            "exec python3 -c 'b = b\"x\" * (64 << 20); input()'\r",
        );
        let first = terminal(&ws, cx, 0).update(cx, |t, _| t.shell_pid);
        wait_until(cx, "memory never measured", |cx| {
            cx.debug_bounds("status-TotalMemory").is_some()
                && ProcessSampler::shared()
                    .sample(first)
                    .is_some_and(|sample| sample.rss_bytes > 64 << 20)
        });
        assert_eq!(current(&ws, cx), 1);
        let total = cx.debug_bounds("status-TotalMemory").unwrap().center();
        click(cx, total, 1);
        release(cx, total);
        cx.run_until_parked();
        assert!(palette_open(&ws, cx));
        let names = palette_names(&ws, cx);
        assert_eq!(names.len(), 2);
        assert!(names[0].starts_with("1: "), "{names:?}");
        assert!(names[1].starts_with("2: "), "{names:?}");
        type_keys(cx, "enter");
        assert!(!palette_open(&ws, cx));
        assert_eq!(current(&ws, cx), 0);
    }

    #[cfg(not(target_os = "macos"))]
    #[gpui::test]
    fn memory_click_switches_unit_and_remembers_it(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("workspace_memory_unit");
        crate::settings::tests::with_config_home(&dir, || {
            let (ws, cx) = open_with_status(cx, "[]", r#"["memory"]"#, "[]");
            let memory = |cx: &mut VisualTestContext| {
                ws.update(cx, |ws, _| ws.status[0].first().map(|p| p.text.clone()))
            };
            wait_until(cx, "memory never measured", |cx| memory(cx).is_some());
            for unit in ["KB", "MB", "GB"] {
                let at = cx.debug_bounds("status-Memory").unwrap().center();
                click(cx, at, 1);
                release(cx, at);
                wait_until(cx, "memory unit never switched", |cx| {
                    memory(cx).is_some_and(|text| text.ends_with(unit))
                });
            }
            assert_eq!(
                StatusBarState::load().memory_unit(MemoryUnit::Auto),
                MemoryUnit::Gb
            );
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn cpu_and_network_totals_list_every_tab_and_the_queue_says_when_empty(
        cx: &mut TestAppContext,
    ) {
        let (ws, cx) = open(cx, 2);
        let clicked = |ws: &Entity<Workspace>, cx: &mut VisualTestContext, click: Click| {
            ws.update_in(cx, |ws, window, cx| ws.status_clicked(click, window, cx));
            cx.run_until_parked();
        };
        clicked(&ws, cx, Click::TotalCpu);
        let names = palette_names(&ws, cx);
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().all(|name| name.contains('%')), "{names:?}");
        type_keys(cx, "escape");
        assert!(!palette_open(&ws, cx));

        clicked(&ws, cx, Click::TotalNetwork);
        let names = palette_names(&ws, cx);
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(
            names
                .iter()
                .all(|name| name.contains("↓") && name.contains("/s")),
            "{names:?}"
        );
        type_keys(cx, "escape");

        clicked(&ws, cx, Click::Queue);
        assert!(!palette_open(&ws, cx));
        assert_eq!(notifications(&ws, cx), ["the queue is empty"]);
    }
}
