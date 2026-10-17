//! copy, paste and dropped files

use super::*;

/// selected text as it goes into the clipboard, none when nothing is selected
pub fn copied_text(terminal: &Terminal, cx: &App) -> Option<String> {
    Some(trim_copied(terminal.selection_text()?, cx))
}

// trailing whitespace goes when settings say so
pub(super) fn trim_copied(text: String, cx: &App) -> String {
    if !Settings::get(cx).terminal.copy_trim_trailing_whitespace {
        return text;
    }
    trim_trailing_whitespace(&text)
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

impl TerminalView {
    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    pub(super) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = copied_text(self.terminal.read(cx), cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            cx.emit(ViewEvent::Copied(text));
        }
    }

    pub(super) fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.paste_text(text, cx);
        }
    }

    /// paste `text`, asking first through the workspace when paste protection finds it unsafe
    pub fn paste_text(&mut self, text: String, cx: &mut Context<Self>) {
        let settings = &Settings::get(cx).terminal;
        let unsafe_keys =
            settings.paste_protection && self.terminal.read(cx).paste_is_unsafe(&text);
        let hides = settings.paste_check_invisible && !paste_check::flagged(&text).is_empty();
        if unsafe_keys || hides {
            cx.emit(ViewEvent::ConfirmPaste(text));
            return;
        }
        self.force_paste(&text, cx);
    }

    /// paste `text` without asking
    pub fn force_paste(&mut self, text: &str, cx: &mut Context<Self>) {
        self.input_happened(cx);
        self.terminal
            .update(cx, |term, _| term.paste(InputSource::Paste, text));
        cx.emit(ViewEvent::Pasted);
        cx.emit(TypedInput::Paste(text.to_string()));
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
        self.terminal
            .update(cx, |term, _| term.paste(InputSource::Paste, &text));
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::ui::terminal_view::tests::*;

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
            view.terminal
                .update(cx, |term, _| term.type_text(b"printf 'padded \\t\\n'\r"))
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

    #[gpui::test]
    fn dropped_paths_are_typed_quoted_with_a_space_after(cx: &mut TestAppContext) {
        let (view, cx) = focused_view("{}", cx);
        // a comment keeps the shell from running what is dropped
        view.update(cx, |view, cx| {
            view.terminal.update(cx, |term, _| term.type_text(b": "))
        });
        let paths = ExternalPaths(
            vec![
                std::path::PathBuf::from("/tmp/plain.txt"),
                std::path::PathBuf::from("/tmp/my dir"),
            ]
            .into(),
        );
        view.update(cx, |view, cx| view.drop_paths(&paths, cx));
        view.update(cx, |view, cx| {
            view.terminal.update(cx, |term, _| term.type_text(b"|end"))
        });
        wait_for(cx, "dropped paths never echoed", |cx| {
            screen(&view, cx).contains(": /tmp/plain.txt '/tmp/my dir' |end")
        });
    }
}
