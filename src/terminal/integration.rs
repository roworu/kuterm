//! shell integration: prompts and command output marked by the scanner, found in the grid

use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use alacritty_terminal::{
    Term,
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as AlacPoint, Side},
    selection::{Selection, SelectionType},
};

use crate::settings::{Profile, Shell, state_dir};

use super::{
    Terminal,
    builder::ZedListener,
    scanner::{OUTPUT_MARK, PROMPT_MARK},
};

/// first line of every marked prompt, top to bottom
fn prompt_lines(term: &Term<ZedListener>) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut last_id: Option<String> = None;
    for line in term.topmost_line().0..=term.bottommost_line().0 {
        let row = &term.grid()[Line(line)];
        let id = (0..term.columns()).find_map(|column| {
            let link = row[Column(column)].hyperlink()?;
            link.id()
                .starts_with(PROMPT_MARK)
                .then(|| link.id().to_string())
        });
        if let Some(id) = id
            && last_id.as_ref() != Some(&id)
        {
            lines.push(Line(line));
            last_id = Some(id);
        }
    }
    lines
}

/// first and last cell of the newest marked command output
fn last_output(term: &Term<ZedListener>) -> Option<(AlacPoint, AlacPoint)> {
    let mut newest: Option<(usize, AlacPoint, AlacPoint)> = None;
    for line in term.topmost_line().0..=term.bottommost_line().0 {
        let row = &term.grid()[Line(line)];
        for column in 0..term.columns() {
            let Some(link) = row[Column(column)].hyperlink() else {
                continue;
            };
            let Some(number) = link
                .id()
                .strip_prefix(OUTPUT_MARK)
                .and_then(|n| n.parse::<usize>().ok())
            else {
                continue;
            };
            let point = AlacPoint::new(Line(line), Column(column));
            newest = match newest {
                Some((n, start, _)) if n == number => Some((n, start, point)),
                Some((n, _, _)) if n > number => newest,
                _ => Some((number, point, point)),
            };
        }
    }
    newest.map(|(_, start, end)| (start, end))
}

impl Terminal {
    /// scroll so the prompt above the top of the view, or below it, is the first line shown.
    /// false when there is no such prompt
    pub fn jump_to_prompt(&mut self, up: bool) -> bool {
        let mut term = self.term.lock();
        let top = Line(-(term.grid().display_offset() as i32));
        let prompts = prompt_lines(&term);
        let target = if up {
            prompts.iter().rev().find(|line| **line < top)
        } else {
            prompts.iter().find(|line| **line > top)
        };
        let Some(target) = target else {
            return false;
        };
        let offset = -target.0;
        let delta = offset - term.grid().display_offset() as i32;
        term.scroll_display(Scroll::Delta(delta));
        drop(term);
        self.dirty.store(true, Ordering::Release);
        true
    }

    /// select the whole lines of the output of the last command, false when none is marked
    pub fn select_last_output(&mut self) -> bool {
        let mut term = self.term.lock();
        let Some((start, end)) = last_output(&term) else {
            return false;
        };
        let mut selection = Selection::new(SelectionType::Lines, start, Side::Left);
        selection.update(end, Side::Right);
        term.selection = Some(selection);
        term.scroll_to_point(start);
        drop(term);
        self.dirty.store(true, Ordering::Release);
        true
    }

    /// text of the output of the last command, none when none is marked
    pub fn last_output_text(&self) -> Option<String> {
        let term = self.term.lock();
        let (start, end) = last_output(&term)?;
        let text = term.bounds_to_string(
            AlacPoint::new(start.line, Column(0)),
            AlacPoint::new(end.line, term.last_column()),
        );
        Some(text.trim_end().to_string())
    }
}

const BASH_SCRIPT: &str = include_str!("../../assets/shell/kuterm.bash");
const ZSH_SCRIPT: &str = include_str!("../../assets/shell/zsh/.zshenv");
const FISH_SCRIPT: &str = include_str!("../../assets/shell/fish/kuterm.fish");

/// folder the integration scripts are written to, none when it can't be written
fn scripts_dir() -> Option<PathBuf> {
    let dir = state_dir()?.join("shell-integration");
    for (path, contents) in [
        ("kuterm.bash", BASH_SCRIPT),
        ("zsh/.zshenv", ZSH_SCRIPT),
        ("fish/fish/vendor_conf.d/kuterm.fish", FISH_SCRIPT),
    ] {
        let path = dir.join(path);
        // rewritten only when kuterm brings a new version
        if std::fs::read_to_string(&path).ok().as_deref() == Some(contents) {
            continue;
        }
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, contents));
        if let Err(error) = written {
            eprintln!("failed to write {}: {error}", path.display());
            return None;
        }
    }
    Some(dir)
}

/// `profile` changed to load kuterm's shell integration into bash, zsh or fish. a command
/// with its own arguments is left alone, it may not be a shell at all
pub fn with_shell_integration(profile: &Profile) -> Profile {
    let mut profile = profile.clone();
    let program = match &profile.command {
        Shell::System => std::env::var("SHELL").ok(),
        Shell::Program(program) => Some(program.clone()),
        Shell::WithArguments { .. } => None,
    };
    let Some(program) = program else {
        return profile;
    };
    let name = Path::new(&program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    if !matches!(name.as_str(), "bash" | "zsh" | "fish") {
        return profile;
    }
    let Some(dir) = scripts_dir() else {
        return profile;
    };
    let inherited = |key: &str| {
        profile
            .env
            .get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
    };
    match name.as_str() {
        // macos starts the system shell through login, which only keeps the environment
        "bash" if !(cfg!(target_os = "macos") && profile.command == Shell::System) => {
            let rcfile = dir.join("kuterm.bash").to_string_lossy().into_owned();
            profile.command = Shell::WithArguments {
                program,
                args: vec!["--rcfile".into(), rcfile],
            };
        }
        "zsh" => {
            let original = inherited("ZDOTDIR").unwrap_or_default();
            profile.env.insert("KUTERM_ZDOTDIR".into(), original);
            let zdotdir = dir.join("zsh").to_string_lossy().into_owned();
            profile.env.insert("ZDOTDIR".into(), zdotdir);
        }
        "fish" => {
            let data = inherited("XDG_DATA_DIRS")
                .filter(|dirs| !dirs.is_empty())
                .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
            let ours = dir.join("fish").to_string_lossy().into_owned();
            profile
                .env
                .insert("XDG_DATA_DIRS".into(), format!("{ours}:{data}"));
        }
        _ => return profile,
    }
    profile
        .env
        .insert("KUTERM_SHELL_INTEGRATION".into(), "1".into());
    profile
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::with_shell_integration;
    use crate::{
        settings::{Shell, TerminalSettings},
        terminal::{
            Event,
            tests::{profile, screen_text, spawn_with, wait_for_text},
        },
    };

    #[test]
    fn marks_find_prompts_and_the_last_output() {
        // three prompts with a command each, printed the way an integrated shell does
        let script = concat!(
            "i=0; while [ $i -lt 40 ]; do ",
            "printf '\\033]133;A\\007$ \\033]133;B\\007cmd_%s\\n\\033]133;C\\007' $i; ",
            "printf 'out_%s\\nmore_%s\\n\\033]133;D;0\\007' $i $i; i=$((i+1)); done; ",
            "printf '\\033]133;A\\007$ \\033]133;B\\007\\033]7;file://h/tmp\\007'; echo; echo marks_ready; sleep 5"
        );
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "marks_ready");
        terminal.handle_scan_events();
        assert_eq!(terminal.reported_cwd(), Some(std::path::Path::new("/tmp")));
        assert_eq!(
            terminal.last_output_text().as_deref(),
            Some("out_39\nmore_39")
        );
        assert!(terminal.select_last_output());
        assert_eq!(
            terminal.selection_text().as_deref(),
            Some("out_39\nmore_39\n")
        );

        // up from the bottom, each jump puts an older prompt at the top of the view
        assert!(terminal.jump_to_prompt(true));
        terminal.sync();
        let first = screen_text(terminal).lines().next().unwrap().to_string();
        assert!(first.starts_with("$ cmd_"), "{first:?}");
        assert!(terminal.jump_to_prompt(true));
        terminal.sync();
        let second = screen_text(terminal).lines().next().unwrap().to_string();
        assert!(
            second.starts_with("$ cmd_") && second < first,
            "{second:?} {first:?}"
        );
        assert!(terminal.jump_to_prompt(false));
        terminal.sync();
        assert_eq!(screen_text(terminal).lines().next().unwrap(), first);
    }

    #[test]
    fn command_end_after_a_start_reports_its_duration() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf '\\033]133;D;0\\007\\033]133;C\\007'; sleep 0.3; printf '\\033]133;D;3\\007\\033]9;all done\\007'; echo end_ready; sleep 5".into(),
            ],
        });
        let started = Instant::now();
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "end_ready");
        let events = terminal.handle_scan_events();
        // the first end has no start, so it ended nothing
        let [
            Event::CommandFinished { duration, exit },
            Event::Notify { title, body },
        ] = events.as_slice()
        else {
            panic!("{events:?}");
        };
        assert_eq!(*exit, Some(3));
        assert!(*duration >= Duration::from_millis(250) && *duration <= started.elapsed());
        assert_eq!((title.as_str(), body.as_str()), ("", "all done"));
    }

    #[test]
    fn bash_integration_marks_commands_and_reports_the_folder() {
        let bash = profile(Shell::Program("/bin/bash".into()));
        let integrated = with_shell_integration(&bash);
        let Shell::WithArguments { args, .. } = &integrated.command else {
            panic!("bash keeps no arguments: {:?}", integrated.command);
        };
        assert_eq!(args[0], "--rcfile");
        let mut builder = spawn_with(&TerminalSettings::default(), &integrated);
        let terminal = &mut builder.terminal;
        terminal.input(b"cd /tmp && echo integ_$((40+2))\r".to_vec());
        wait_for_text(terminal, "integ_42");
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut finished = false;
        while !(finished && terminal.reported_cwd() == Some(std::path::Path::new("/tmp"))) {
            terminal.sync();
            finished |= terminal
                .handle_scan_events()
                .iter()
                .any(|event| matches!(event, Event::CommandFinished { exit: Some(0), .. }));
            assert!(Instant::now() < deadline, "no marks from bash");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(terminal.last_output_text().as_deref(), Some("integ_42"));
    }

    /// next message from a shell `matches` takes, skipping the others
    #[cfg(unix)]
    fn wait_shell(
        events: &mut futures::channel::mpsc::UnboundedReceiver<crate::completion::ShellEvent>,
        what: &str,
        matches: impl Fn(&crate::completion::ShellMessage) -> bool,
    ) -> crate::completion::ShellEvent {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match events.try_recv() {
                Ok(event) if matches(&event.message) => return event,
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
            assert!(Instant::now() < deadline, "no {what} from zsh");
        }
    }

    #[cfg(unix)]
    #[test]
    fn zsh_reports_its_line_and_takes_completions() {
        use crate::completion::{Bridge, SOCKET_ENV, ShellMessage, protocol};

        let Some(zsh) = ["/usr/bin/zsh", "/bin/zsh"]
            .into_iter()
            .find(|path| std::path::Path::new(path).exists())
        else {
            eprintln!("no zsh, skipped");
            return;
        };
        // a home of its own: no user files, and an empty .zshrc skips zsh's first run wizard
        let home = std::env::temp_dir().join(format!("kuterm-zsh-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".zshrc"), "PS1='$ '\n").unwrap();
        let mut zsh = profile(Shell::Program(zsh.into()));
        let home_str = home.to_string_lossy().into_owned();
        zsh.env.insert("HOME".into(), home_str.clone());
        zsh.env.insert("ZDOTDIR".into(), home_str);
        let mut zsh = with_shell_integration(&zsh);
        let mut bridge = Bridge::new().unwrap();
        let mut events = bridge.take_events().unwrap();
        zsh.env
            .insert(SOCKET_ENV.into(), bridge.path().to_string_lossy().into());
        let mut builder = spawn_with(&TerminalSettings::default(), &zsh);
        let terminal = &mut builder.terminal;
        terminal.set_completion_bridge(bridge);

        wait_shell(
            &mut events,
            "hello",
            |m| matches!(m, ShellMessage::Hello { shell } if shell == "zsh"),
        );
        wait_shell(&mut events, "PATH", |m| {
            matches!(m, ShellMessage::Env { .. })
        });
        terminal.input(b"echo comp_".to_vec());
        let typed = wait_shell(
            &mut events,
            "the typed line",
            |m| matches!(m, ShellMessage::Line { buffer, cursor: 10, .. } if buffer == "echo comp_"),
        );
        let ShellMessage::Line { seq, cwd, .. } = &typed.message else {
            unreachable!()
        };
        assert!(cwd.is_absolute());

        // an answer to an older line is dropped
        let stale = protocol::accept(seq - 1, 5, 10, "wrong ").unwrap();
        assert!(terminal.send_to_shell(typed.conn, &stale));
        let fresh = protocol::accept(*seq, 5, 10, "comp_done ").unwrap();
        assert!(terminal.send_to_shell(typed.conn, &fresh));
        wait_shell(
            &mut events,
            "the completed line",
            |m| matches!(m, ShellMessage::Line { buffer, cursor: 15, .. } if buffer == "echo comp_done "),
        );

        // the completed line runs as typed
        terminal.input(b"\r".to_vec());
        wait_shell(&mut events, "the end of the line", |m| {
            *m == ShellMessage::End
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        while !screen_text(terminal)
            .lines()
            .any(|line| line.trim_end() == "comp_done")
        {
            assert!(Instant::now() < deadline, "{}", screen_text(terminal));
            std::thread::sleep(Duration::from_millis(50));
            terminal.sync();
        }
        assert!(!screen_text(terminal).contains("wrong"));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn zsh_and_fish_are_pointed_at_the_scripts() {
        let zsh = with_shell_integration(&profile(Shell::Program("/usr/bin/zsh".into())));
        assert!(zsh.env["ZDOTDIR"].ends_with("shell-integration/zsh"));
        assert!(zsh.env.contains_key("KUTERM_ZDOTDIR"));
        let fish = with_shell_integration(&profile(Shell::Program("fish".into())));
        assert!(fish.env["XDG_DATA_DIRS"].contains("shell-integration/fish:"));
        // commands with arguments and other programs are left alone
        let other = profile(Shell::Program("/bin/sh".into()));
        assert_eq!(with_shell_integration(&other), other);
        let args = profile(Shell::WithArguments {
            program: "bash".into(),
            args: vec!["-l".into()],
        });
        assert_eq!(with_shell_integration(&args), args);
    }
}
