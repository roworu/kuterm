//! `file:line` links open in an editor already running in another pane, at that line, and
//! that pane gets the focus. nvim gets it over its server socket, the others as keys

use std::path::{Path, PathBuf};

use gpui::{Context, Entity, Window};

use super::Workspace;
use crate::{
    terminal::{
        FileLine, ForegroundProcess, InputSource, Link, foreground_process, parse_file_line,
        resolve_file,
    },
    ui::terminal_view::TerminalView,
};

/// how a running editor is told to open a file
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum EditorOpen {
    /// keys typed into its pane
    Keys(String),
    /// `nvim --server <socket> --remote-send <keys>`
    NvimRemote { socket: PathBuf, keys: String },
}

/// nvim's server socket: `--listen <path>`, else the default one in the runtime dir
fn nvim_socket(process: &ForegroundProcess, runtime_dir: Option<&Path>) -> Option<PathBuf> {
    let listen = process
        .args
        .iter()
        .position(|arg| arg == "--listen")
        .and_then(|at| process.args.get(at + 1));
    if let Some(listen) = listen {
        return Some(PathBuf::from(listen));
    }
    let socket = runtime_dir?.join(format!("nvim.{}.0", process.pid));
    socket.exists().then_some(socket)
}

/// what makes the editor `process` open `path` at `at`, none for programs that are no
/// editor kuterm knows
pub(super) fn editor_open(
    process: &ForegroundProcess,
    path: &Path,
    at: &FileLine,
    runtime_dir: Option<&Path>,
) -> Option<EditorOpen> {
    let file = path.display().to_string();
    // vim's ex commands take spaces in names escaped
    let escaped = file.replace(' ', "\\ ");
    let line = at.line;
    let column = at.column.unwrap_or(1);
    Some(match process.name.as_str() {
        "nvim" => {
            let keys = format!("<C-\\><C-N>:e +{line} {escaped}<CR>{column}|");
            match nvim_socket(process, runtime_dir) {
                Some(socket) => EditorOpen::NvimRemote { socket, keys },
                None => EditorOpen::Keys(format!("\x1b:e +{line} {escaped}\r{column}|")),
            }
        }
        "vim" | "vi" => EditorOpen::Keys(format!("\x1b:e +{line} {escaped}\r{column}|")),
        "hx" | "helix" => EditorOpen::Keys(format!("\x1b:open {file}:{line}:{column}\r")),
        "kak" => EditorOpen::Keys(format!("\x1b:edit {escaped} {line} {column}\r")),
        // only the terminal emacs reads keys from the pty
        "emacs" if process.args.iter().any(|arg| arg == "-nw" || arg == "-t") => {
            EditorOpen::Keys(format!("\x07\x18\x06{file}\r\x1bgg{line}\r"))
        }
        _ => return None,
    })
}

impl Workspace {
    /// open the `file:line` of `link` in a running editor, else as the view opens links
    pub(super) fn open_file_line(
        &mut self,
        view: &Entity<TerminalView>,
        link: &Link,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(at) = parse_file_line(&link.target) {
            let cwd = view
                .read(cx)
                .terminal()
                .read(cx)
                .folder_at(link.range.start().line);
            let home = std::env::var_os("HOME").map(PathBuf::from);
            let path = resolve_file(&at.path, cwd.as_deref(), home.as_deref());
            if self.open_in_running_editor(&path, &at, window, cx) {
                return;
            }
        }
        view.update(cx, |view, cx| view.run_link_here(link, cx));
    }

    /// open `at`, resolved to `path`, in an editor running in a pane of this window and focus
    /// it, false when none runs
    pub(super) fn open_in_running_editor(
        &mut self,
        path: &Path,
        at: &FileLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        // panes shown now first, the focused one last, then tabs in the background
        let mut candidates = Vec::new();
        for pane in self.layout.panes() {
            for (ix, tab) in pane.tabs.iter().enumerate() {
                let order = match (pane.active == ix, pane.id == self.focused) {
                    (true, false) => 0,
                    (true, true) => 1,
                    (false, _) => 2,
                };
                let shell_pid = tab.view.read(cx).terminal().read(cx).shell_pid;
                candidates.push((order, pane.id, ix, shell_pid));
            }
        }
        candidates.sort_by_key(|(order, ..)| *order);
        for (_, pane, ix, shell_pid) in candidates {
            let Some(process) = foreground_process(shell_pid).filter(|p| p.pid != shell_pid) else {
                continue;
            };
            let Some(open) = editor_open(&process, path, at, runtime_dir.as_deref()) else {
                continue;
            };
            match open {
                EditorOpen::Keys(keys) => {
                    self.input_to(pane, ix, InputSource::Automation, keys.into_bytes(), cx);
                }
                EditorOpen::NvimRemote { socket, keys } => {
                    let sent = std::process::Command::new("nvim")
                        .arg("--server")
                        .arg(&socket)
                        .arg("--remote-send")
                        .arg(&keys)
                        .status();
                    if !sent.is_ok_and(|status| status.success()) {
                        continue;
                    }
                }
            }
            self.activate_tab(pane, ix, window, cx);
            self.focus_pane(pane, window, cx);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{EditorOpen, editor_open};
    use crate::terminal::{FileLine, ForegroundProcess};

    fn process(name: &str, args: &[&str]) -> ForegroundProcess {
        ForegroundProcess {
            pid: 4242,
            name: name.into(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            cwd: None,
        }
    }

    fn keys(open: Option<EditorOpen>) -> String {
        match open {
            Some(EditorOpen::Keys(keys)) => keys,
            other => panic!("no keys: {other:?}"),
        }
    }

    #[test]
    fn each_editor_gets_its_own_command() {
        let path = Path::new("/w/my file.rs");
        let at = FileLine {
            path: "my file.rs".into(),
            line: 42,
            column: Some(7),
        };
        assert_eq!(
            keys(editor_open(&process("vim", &[]), path, &at, None)),
            "\x1b:e +42 /w/my\\ file.rs\r7|"
        );
        assert_eq!(
            keys(editor_open(&process("hx", &[]), path, &at, None)),
            "\x1b:open /w/my file.rs:42:7\r"
        );
        assert_eq!(
            keys(editor_open(&process("kak", &[]), path, &at, None)),
            "\x1b:edit /w/my\\ file.rs 42 7\r"
        );
        assert!(
            keys(editor_open(&process("emacs", &["-nw"]), path, &at, None)).ends_with("\x1bgg42\r")
        );
        assert_eq!(editor_open(&process("emacs", &[]), path, &at, None), None);
        assert_eq!(editor_open(&process("less", &[]), path, &at, None), None);
        // nvim without a socket gets keys like vim
        assert!(
            keys(editor_open(&process("nvim", &[]), path, &at, None)).starts_with("\x1b:e +42")
        );
        assert_eq!(
            editor_open(
                &process("nvim", &["--listen", "/tmp/n.sock"]),
                path,
                &at,
                None
            ),
            Some(EditorOpen::NvimRemote {
                socket: PathBuf::from("/tmp/n.sock"),
                keys: "<C-\\><C-N>:e +42 /w/my\\ file.rs<CR>7|".into(),
            })
        );
        // the default socket in the runtime dir, named by pid
        let dir = std::env::temp_dir().join(format!("kuterm_nvim_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("nvim.4242.0"), "").unwrap();
        let open = editor_open(&process("nvim", &[]), path, &at, Some(&dir));
        assert!(
            matches!(open, Some(EditorOpen::NvimRemote { .. })),
            "{open:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
