//! links under the pointer: hover, peek at file contents and opening them

use super::*;

// the mouse rests on a `file:line` this long before its peek shows
const PEEK_DELAY: Duration = Duration::from_millis(500);

// lines a peek shows above and below the named one
const PEEK_AROUND: u32 = 5;

/// open `target` with `links.open_with`, or the system opener
pub fn open_link(target: String, cx: &App) {
    match Settings::get(cx).links.open_with.clone() {
        Some(program) => {
            cx.background_spawn(async move {
                if let Err(error) = std::process::Command::new(&program).arg(target).status() {
                    eprintln!("failed to run {program:?}: {error}");
                }
            })
            .detach();
        }
        None => cx.open_url(&target),
    }
}

impl TerminalView {
    /// link at a window position while the link modifier is held
    pub(super) fn link_at(
        &mut self,
        position: Point<Pixels>,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) -> Option<Link> {
        if !Settings::get(cx).links.modifier.held(modifiers) {
            return None;
        }
        cx.default_global::<LinkPatterns>();
        let terminal = self.terminal.clone();
        cx.update_global(|patterns: &mut LinkPatterns, cx| {
            patterns.0.update(&Settings::get(cx).links.patterns);
            terminal.update(cx, |terminal, _| {
                terminal.link_at(position, &mut patterns.0)
            })
        })
    }

    // the peek shows after the mouse rested on a `file:line`, and goes once it leaves it
    pub(super) fn update_peek(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !Settings::get(cx).links.peek {
            return;
        }
        let terminal = self.terminal.read(cx);
        let found = terminal.file_line_at(position);
        let cell = found
            .as_ref()
            .map(|(_, range, _)| (range.start().line.0, range.start().column.0));
        if cell == self.peek_cell {
            return;
        }
        self.peek_cell = cell;
        if terminal.decorations().popover.is_some() {
            self.terminal.update(cx, |terminal, _| {
                terminal.decorate(|decorations| decorations.popover = None)
            });
            cx.notify();
        }
        let Some((at, range, cwd)) = found else {
            self._peek = Task::ready(());
            return;
        };
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let path = resolve_file(&at.path, cwd.as_deref(), home.as_deref());
        self._peek = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PEEK_DELAY).await;
            let lines = cx
                .background_spawn(async move { snippet(&path, at.line, PEEK_AROUND) })
                .await;
            let Some(content) = lines else {
                return;
            };
            this.update(cx, |this, cx| {
                let popover = Popover {
                    range: (*range.start(), *range.end()),
                    content,
                };
                this.terminal.update(cx, |terminal, _| {
                    terminal.decorate(|decorations| decorations.popover = Some(popover))
                });
                cx.notify();
            })
            .ok();
        });
    }

    // underline follows the mouse, the modifier and output moving the text
    pub(super) fn update_hovered_link(&mut self, cx: &mut Context<Self>) {
        let link = self
            .last_mouse
            .and_then(|(position, modifiers)| self.link_at(position, &modifiers, cx));
        if link != self.hovered_link {
            self.hovered_link = link;
            cx.notify();
        }
    }

    pub(super) fn modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((_, modifiers)) = &mut self.last_mouse {
            *modifiers = event.modifiers;
            self.update_hovered_link(cx);
        }
    }

    /// run what a link or hint is for: open, copy, paste or a command
    pub fn run_link(&mut self, link: &Link, cx: &mut Context<Self>) {
        if link.action == PatternAction::Open
            && Settings::get(cx).links.open_in_running_editor
            && parse_file_line(&link.target).is_some()
        {
            cx.emit(ViewEvent::OpenFileLine(link.clone()));
            return;
        }
        self.run_link_here(link, cx);
    }

    /// run what a link is for in this view, never handing it to another pane
    pub fn run_link_here(&mut self, link: &Link, cx: &mut Context<Self>) {
        match &link.action {
            // kuterm's own links open here, after this view is done updating
            PatternAction::Open if link.target.starts_with("kuterm://") => {
                let target = link.target.clone();
                cx.defer(move |cx| open_urls(&[target], cx));
            }
            PatternAction::Open => open_link(link.target.clone(), cx),
            PatternAction::Copy => {
                cx.write_to_clipboard(ClipboardItem::new_string(link.target.clone()));
                cx.emit(ViewEvent::Copied(link.target.clone()));
            }
            PatternAction::Paste => {
                self.input_happened(cx);
                self.terminal.update(cx, |terminal, _| {
                    terminal.paste(InputSource::Paste, &link.target)
                });
            }
            PatternAction::Exec(command) => {
                let shell_pid = self.terminal.read(cx).shell_pid;
                let (command, target) = (command.clone(), link.target.clone());
                cx.background_spawn(async move {
                    let cwd = foreground_process(shell_pid).and_then(|process| process.cwd);
                    let mut child = std::process::Command::new("/bin/sh");
                    child.arg("-c").arg(&command).env("KUTERM_MATCH", target);
                    if let Some(cwd) = cwd {
                        child.current_dir(cwd);
                    }
                    if let Err(error) = child.status() {
                        eprintln!("failed to run {command:?}: {error}");
                    }
                })
                .detach();
            }
        }
    }

    /// link under the mouse, exposed for tests
    #[cfg(test)]
    pub(crate) fn hovered_link(&self) -> Option<&Link> {
        self.hovered_link.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::index::{Column, Line, Point as AlacPoint};
    use gpui::TestAppContext;

    use super::*;
    use crate::ui::terminal_view::tests::*;

    fn link(target: &str, action: PatternAction) -> Link {
        let at = AlacPoint::new(Line(0), Column(0));
        Link {
            range: at..=at,
            rest: Vec::new(),
            target: target.into(),
            action,
            name: Some("test".into()),
        }
    }

    #[gpui::test]
    fn paste_link_types_its_target(cx: &mut TestAppContext) {
        let (view, cx) = focused_view("{}", cx);
        view.update(cx, |view, cx| {
            view.terminal.update(cx, |term, _| term.type_text(b": "))
        });
        let link = link("pasted-target-1", PatternAction::Paste);
        view.update(cx, |view, cx| view.run_link_here(&link, cx));
        wait_for(cx, "target never typed", |cx| {
            screen(&view, cx).contains(": pasted-target-1")
        });
    }

    #[gpui::test]
    fn exec_link_runs_in_the_shell_folder_with_the_match(cx: &mut TestAppContext) {
        let (view, cx) = focused_view("{}", cx);
        let dir = std::env::temp_dir().join(format!("kuterm-exec-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out");
        let cd = format!("cd '{}'\r", dir.display());
        view.update(cx, |view, cx| {
            view.terminal
                .update(cx, |term, _| term.type_text(cd.as_bytes()))
        });
        let shell_pid = view.read_with(cx, |view, cx| view.terminal.read(cx).shell_pid);
        wait_for(cx, "shell never changed folder", |_| {
            foreground_process(shell_pid).and_then(|process| process.cwd) == Some(dir.clone())
        });
        let command = r#"printf '%s %s' "$KUTERM_MATCH" "$PWD" > out.tmp; mv out.tmp out"#;
        let link = link("ticket-7", PatternAction::Exec(command.into()));
        view.update(cx, |view, cx| view.run_link_here(&link, cx));
        wait_for(cx, "command never ran", |_| out.exists());
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            format!("ticket-7 {}", dir.display())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[gpui::test]
    fn open_link_without_open_with_uses_the_system_opener(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"links": {"open_with": null}}"#, cx);
        let link = link("https://example.com/x", PatternAction::Open);
        view.update(cx, |view, cx| view.run_link_here(&link, cx));
        assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/x"));
    }
}
