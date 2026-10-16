//! programs waiting for the user: questions and password prompts mark their tab and notify
//! once, and banners at the bottom of a pane explain output paused with ctrl+s and programs
//! stopped with ctrl+z

use std::time::Instant;

use gpui::{AnyElement, Context, Entity, Window, div, prelude::*};
use regex::Regex;

use super::{Pane, Workspace, notifications::NotificationButton};
use crate::{
    events_bus::{BusEvent, EventBus},
    settings::Settings,
    terminal::{InputSource, Question, StoppedJob},
    theme::Theme,
    ui::terminal_view::TerminalView,
};

// nerd font lock, shown as the tab icon while a program reads a secret
pub(super) const LOCK_ICON: &str = "\u{f023}";

/// `question_patterns` as typed and compiled, broken ones left out with an error on stderr
#[derive(Default)]
pub(super) struct QuestionPatterns {
    source: Vec<String>,
    compiled: Vec<Regex>,
}

impl QuestionPatterns {
    /// the compiled `patterns`, compiled again only when they changed
    fn get(&mut self, patterns: &[String]) -> &[Regex] {
        if self.source != patterns {
            self.source = patterns.to_vec();
            self.compiled = patterns
                .iter()
                .filter_map(|pattern| {
                    Regex::new(pattern)
                        .inspect_err(|error| eprintln!("question pattern {pattern:?}: {error}"))
                        .ok()
                })
                .collect();
        }
        &self.compiled
    }
}

/// what a notification about `question` in tab `number` says
fn question_text(question: &Question, number: usize) -> String {
    if question.secret {
        format!("tab {number} asks for a password")
    } else {
        format!("tab {number} waits for an answer: {}", question.line)
    }
}

/// text of the banner for a pane: paused output, then stopped jobs, none when all is well
pub(super) fn banner_text(paused: bool, stopped: &[StoppedJob]) -> Option<String> {
    if paused {
        return Some("output paused by ctrl+s, ctrl+q resumes".into());
    }
    if stopped.is_empty() {
        return None;
    }
    let names: Vec<String> = stopped
        .iter()
        .map(|job| format!("{}, stopped", job.name))
        .collect();
    Some(names.join(" · "))
}

impl Workspace {
    /// mark and tell about programs that started waiting for the user since the last refresh
    pub(super) fn check_waiting(&mut self, window: &Window, cx: &mut Context<Self>) {
        let settings = Settings::get(cx);
        if !settings.notifications.program_waiting {
            return;
        }
        let patterns = settings.notifications.question_patterns.clone();
        let patterns = self.question_patterns.get(&patterns).to_vec();
        let now = Instant::now();
        let mut asked = Vec::new();
        for pane in self.layout.panes() {
            for (ix, tab) in pane.tabs.iter().enumerate() {
                let terminal = tab.view.read(cx).terminal().read(cx);
                let question = terminal.question(&patterns, now);
                let known = tab.asked.as_ref().map(|(line, _)| line);
                let told = tab.asked.as_ref().is_some_and(|(_, told)| *told);
                // a new question, or one that waits untold, like one asked in a tab in front
                // that the user then left
                if question.as_ref().map(|q| &q.line) != known || (question.is_some() && !told) {
                    asked.push((tab.view.clone(), ix, question, terminal.inputs()));
                }
            }
        }
        for (view, ix, question, inputs) in asked {
            let seen = self.tab_seen(&view, window);
            let Some(tab) = self.tab_mut(&view) else {
                continue;
            };
            let Some(question) = question else {
                tab.asked = None;
                continue;
            };
            if tab.asked.as_ref().map(|(line, _)| line) != Some(&question.line) {
                // the same mark and "next waiting tab" as an agent waiting
                tab.attention.wait(inputs, Instant::now());
                cx.notify();
            }
            // told once per question, when it is out of sight
            tab.asked = Some((question.line.clone(), !seen));
            if seen {
                continue;
            }
            let text = question_text(&question, ix + 1);
            if !window.is_window_active() {
                super::notifications::system_notification("kuterm", &text);
            }
            let buttons = self.question_buttons(&view, &question);
            // the typed answer never shows, only the question
            if buttons.is_empty() {
                self.show_notification(text, Some(view.clone()), cx);
            } else {
                self.ask(text, None, Some(view.clone()), buttons, cx);
            }
        }
    }

    fn question_buttons(
        &self,
        view: &Entity<TerminalView>,
        question: &Question,
    ) -> Vec<NotificationButton> {
        let answer = |label: &'static str, text: &'static str| {
            let view = view.clone();
            NotificationButton::new(label, move |this, _, cx| {
                let terminal = view.read(cx).terminal().clone();
                // the user clicked it, so it is their own answer
                terminal.update(cx, |terminal, _| {
                    terminal.input(InputSource::Keyboard, text.as_bytes().to_vec())
                });
                this.attention_ended(&view, cx);
            })
        };
        if question.secret {
            let view = view.clone();
            return vec![NotificationButton::new(
                "fill password",
                move |this, window, cx| {
                    if let Some((pane, ix)) = this.find_view(&view) {
                        this.activate_tab(pane, ix, window, cx);
                        this.fill_password(window, cx);
                    }
                },
            )];
        }
        if question.yes_no {
            return vec![answer("y", "y\r"), answer("n", "n\r")];
        }
        Vec::new()
    }

    /// tell the bus about tty settings that changed in the tab of `pane`
    pub(super) fn publish_pty_modes(&self, pane: u64, modes: Option<crate::terminal::PtyModes>) {
        let modes = modes.unwrap_or_default();
        EventBus::publish(BusEvent::PtyModesChanged {
            pane,
            echo: modes.echo,
            icanon: modes.icanon,
            ixon: modes.ixon,
        });
    }

    /// banner at the bottom of the pane's active tab: output paused by ctrl+s, or the
    /// programs stopped with ctrl+z, a click on one brings it back
    pub(super) fn render_pty_banner(&self, pane: &Pane, cx: &Context<Self>) -> Option<AnyElement> {
        let tab = pane.tabs.get(pane.active)?;
        let terminal = tab.view.read(cx).terminal().read(cx);
        let paused = terminal.xoff_paused();
        let stopped = terminal.stopped_jobs().to_vec();
        let text = banner_text(paused, &stopped)?;
        let theme = Theme::get(cx);
        let pane_id = pane.id;
        let view = tab.view.clone();
        let jobs = if paused { Vec::new() } else { stopped };
        Some(
            div()
                .id(("pane-pty-banner", pane_id))
                .debug_selector(move || format!("pane-pty-banner-{pane_id}"))
                .absolute()
                .bottom_0()
                .right_0()
                .px_2()
                .rounded_tl_md()
                .bg(theme.attention)
                .text_color(theme.terminal_background)
                .text_sm()
                .when(jobs.is_empty(), |banner| banner.child(text))
                .children(jobs.into_iter().enumerate().map(move |(ix, job)| {
                    let view = view.clone();
                    div()
                        .id(("stopped-job", ix))
                        .debug_selector(move || format!("stopped-job-{ix}"))
                        .child(format!("{}, stopped", job.name))
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            let terminal = view.read(cx).terminal().clone();
                            terminal.update(cx, |terminal, _| {
                                // only the shell knows `fg`, a running program would get it
                                if terminal.at_prompt() || !terminal.command_running() {
                                    let fg = format!("fg %{}\r", job.job);
                                    terminal.input(InputSource::Keyboard, fg.into_bytes());
                                }
                            });
                        })
                }))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{QuestionPatterns, banner_text, question_text};
    use crate::terminal::{Question, StoppedJob};

    #[test]
    fn banner_says_paused_first_then_stopped_jobs() {
        let vim = StoppedJob {
            pid: 1,
            name: "vim".into(),
            job: 1,
        };
        assert_eq!(banner_text(false, &[]), None);
        assert_eq!(
            banner_text(true, std::slice::from_ref(&vim)).as_deref(),
            Some("output paused by ctrl+s, ctrl+q resumes")
        );
        assert_eq!(
            banner_text(false, &[vim.clone(), vim]).as_deref(),
            Some("vim, stopped · vim, stopped")
        );
    }

    #[test]
    fn notifications_never_show_what_is_typed() {
        let secret = Question {
            line: "[sudo] password for me:".into(),
            secret: true,
            yes_no: false,
        };
        assert_eq!(question_text(&secret, 2), "tab 2 asks for a password");
        let ask = Question {
            line: "continue? [y/N]".into(),
            secret: false,
            yes_no: true,
        };
        assert_eq!(
            question_text(&ask, 1),
            "tab 1 waits for an answer: continue? [y/N]"
        );
    }

    #[test]
    fn broken_patterns_are_left_out() {
        let mut patterns = QuestionPatterns::default();
        let compiled = patterns.get(&[r"\?$".into(), "(".into()]);
        assert_eq!(compiled.len(), 1);
        assert!(compiled[0].is_match("ok?"));
    }
}
