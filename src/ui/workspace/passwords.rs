//! filling a password from a password manager into the active tab: its entries are listed in
//! the palette, the picked one's password is typed. it never goes to the clipboard,
//! notifications or logs

use gpui::{AppContext, Context, Entity, Window};

use super::Workspace;
use crate::{
    settings::{Command, Inputs, Settings, with_name},
    terminal::{Terminal, tty_echoes},
    ui::command_palette::CommandPalette,
};

/// stdout of `command`, the error tells how it failed without its output
fn run(command: &[String]) -> Result<String, String> {
    let (program, args) = command
        .split_first()
        .ok_or("the command is empty".to_string())?;
    let output = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!("{program} failed with {}", output.status));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("{program} printed no text"))
}

impl Workspace {
    /// list the password manager's entries in the palette, the picked one's password is
    /// typed into the active tab
    pub(super) fn fill_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = Settings::get(cx).password_manager.clone();
        let (list, get) = settings.commands();
        let pane = self.focused_pane();
        let Some(tab) = pane.tabs.get(pane.active) else {
            return;
        };
        let terminal = tab.view.read(cx).terminal().clone();
        let shell_pid = terminal.read(cx).shell_pid;
        cx.spawn_in(window, async move |this, cx| {
            let (names, echo) = cx
                .background_spawn(async move { (run(&list), tty_echoes(shell_pid)) })
                .await;
            this.update_in(cx, |this, window, cx| match names {
                Ok(names) => {
                    let names = names
                        .lines()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                        .collect();
                    // echo on means nothing asks for a secret, the password would show
                    let warn = echo == Some(true);
                    this.open_password_picker(
                        names,
                        warn,
                        get,
                        settings.enter,
                        terminal,
                        window,
                        cx,
                    )
                }
                Err(error) => this.show_error(format!("password manager: {error}"), cx),
            })
            .ok();
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn open_password_picker(
        &mut self,
        names: Vec<String>,
        warn: bool,
        get: Vec<String>,
        enter: bool,
        terminal: Entity<Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = names
            .into_iter()
            .map(|name| Command {
                name,
                category: None,
                pinned: false,
                inputs: Inputs::new(),
                actions: Vec::new(),
            })
            .collect();
        let placeholder = if warn {
            "fill password... the program shows what is typed, it doesn't ask for one"
        } else {
            "fill password..."
        };
        let palette = cx.new(|cx| CommandPalette::picker(items, placeholder, cx));
        self.open_overlay(&palette, window, cx);
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            move |this, _, picked: &Command, window, cx| {
                this.close_overlay(window, cx);
                let command = with_name(&get, &picked.name);
                let terminal = terminal.clone();
                cx.spawn(async move |this, cx| {
                    let password = cx.background_spawn(async move { run(&command) }).await;
                    this.update(cx, |this, cx| match password {
                        Ok(output) => {
                            let mut typed = output.lines().next().unwrap_or_default().to_string();
                            if enter {
                                typed.push('\r');
                            }
                            terminal.update(cx, |terminal, _| terminal.input(typed.into_bytes()));
                        }
                        Err(error) => this.show_error(format!("password manager: {error}"), cx),
                    })
                    .ok();
                })
                .detach();
            },
        ));
        self.palette = Some(palette);
    }
}

#[cfg(test)]
mod tests {
    use crate::settings::{Settings, with_name};

    fn commands(json: &str) -> (Vec<String>, Vec<String>) {
        Settings::parse(&format!(r#"{{"password_manager": {json}}}"#))
            .unwrap()
            .password_manager
            .commands()
    }

    #[test]
    fn presets_build_their_commands() {
        let (list, get) = commands(r#"{"preset": "pass"}"#);
        assert_eq!(list[0], "sh");
        assert!(list[2].contains("password-store"));
        assert_eq!(get, ["pass", "show", "{name}"]);
        let (_, get) = commands(r#"{"preset": "secret-tool"}"#);
        assert_eq!(get, ["secret-tool", "lookup", "service", "{name}"]);
        let (list, get) = commands(r#"{"preset": "bw"}"#);
        assert!(list[2].starts_with("bw list items"));
        assert_eq!(get, ["bw", "get", "password", "{name}"]);
        let (_, get) = commands(r#"{"preset": "op"}"#);
        assert_eq!(&get[..4], ["op", "item", "get", "{name}"]);
    }

    #[test]
    fn own_commands_replace_the_preset() {
        let (list, get) = commands(r#"{"preset": "bw", "list": ["ls"], "get": ["cat", "{name}"]}"#);
        assert_eq!(list, ["ls"]);
        assert_eq!(get, ["cat", "{name}"]);
    }

    #[test]
    fn names_stay_one_word() {
        let get = ["pass".to_string(), "show".into(), "{name}".into()];
        assert_eq!(
            with_name(&get, "web/a b; rm -rf x"),
            ["pass", "show", "web/a b; rm -rf x"]
        );
    }
}
