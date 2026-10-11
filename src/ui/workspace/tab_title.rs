//! building tab titles from the blocks set in settings

use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};

use crate::{settings::TabTitleBlock, terminal::ForegroundProcess};

// a hung command would freeze every tab title, so it is abandoned after this
const EXEC_TIMEOUT: Duration = Duration::from_secs(2);

/// what a title is built from, collected on the main thread
pub(super) struct TitleInputs {
    pub number: usize,
    pub shell_pid: u32,
    pub title: String,
    pub profile_icon: Option<String>,
}

/// join blocks into a title, may block on exec commands
pub(super) fn build_title(
    blocks: &[TabTitleBlock],
    inputs: &TitleInputs,
    process: Option<&ForegroundProcess>,
) -> String {
    let cwd = process.and_then(|process| process.cwd.as_deref());
    let mut title = String::new();
    for block in blocks {
        match block {
            TabTitleBlock::Number => title.push_str(&inputs.number.to_string()),
            TabTitleBlock::Prompt => title.push_str(&prompt()),
            TabTitleBlock::Folder => title.push_str(&cwd.map(folder_name).unwrap_or_default()),
            TabTitleBlock::Command => title.push_str(process.map_or("", |process| &process.name)),
            TabTitleBlock::Title => title.push_str(&inputs.title),
            TabTitleBlock::Text(text) => title.push_str(text),
            TabTitleBlock::Exec(command) => title.push_str(&exec(command, cwd).unwrap_or_default()),
        }
    }
    title
}

fn prompt() -> String {
    // titles rebuild for every tab several times a second, and the user never changes
    static USER: OnceLock<String> = OnceLock::new();
    let user = USER.get_or_init(user_name);
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .ok()
        .or_else(system_host)
        .unwrap_or_default();
    format!("{user}@{}", host.trim())
}

// macos has neither hostname file
#[cfg(target_os = "macos")]
fn system_host() -> Option<String> {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let name = &buf[..buf.iter().position(|b| *b == 0)?];
    // "name.local" is shown as "name", like the \h of shell prompts
    let name = String::from_utf8_lossy(name);
    Some(name.split('.').next()?.to_string())
}

#[cfg(not(target_os = "macos"))]
fn system_host() -> Option<String> {
    None
}

// USER is often unset in containers and services, so fall back to other vars and /etc/passwd
fn user_name() -> String {
    ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .find_map(|var| std::env::var(var).ok().filter(|name| !name.is_empty()))
        .or_else(passwd_user)
        .unwrap_or_default()
}

fn passwd_user() -> Option<String> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .to_owned();
    std::fs::read_to_string("/etc/passwd")
        .ok()?
        .lines()
        .find_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            (fields.nth(1)? == uid).then(|| name.to_owned())
        })
}

fn folder_name(cwd: &Path) -> String {
    if std::env::var_os("HOME").is_some_and(|home| cwd == Path::new(&home)) {
        return "~".to_string();
    }
    cwd.file_name().map_or_else(
        || cwd.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn exec(command: &str, cwd: Option<&Path>) -> Option<String> {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    // read on another thread, so a full pipe can't stall the child until the timeout
    let (output_tx, output_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).ok();
        output_tx.send(output).ok();
    });
    let deadline = Instant::now() + EXEC_TIMEOUT;
    while let Ok(None) = child.try_wait() {
        if Instant::now() > deadline {
            child.kill().ok();
            child.wait().ok();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // background jobs of sh may still hold the pipe, so the reader is left to finish alone
    let output = output_rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()?;
    Some(
        String::from_utf8_lossy(&output)
            .lines()
            .next()?
            .trim()
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::foreground_process;

    fn build(blocks: &[TabTitleBlock]) -> String {
        let inputs = TitleInputs {
            number: 3,
            shell_pid: std::process::id(),
            title: "vim".into(),
            profile_icon: None,
        };
        build_title(
            blocks,
            &inputs,
            foreground_process(inputs.shell_pid).as_ref(),
        )
    }

    #[test]
    fn joins_blocks_in_order() {
        let title = build(&[
            TabTitleBlock::Number,
            TabTitleBlock::Text(": ".into()),
            TabTitleBlock::Title,
            TabTitleBlock::Text(" - ".into()),
            TabTitleBlock::Exec("echo first; echo second".into()),
        ]);
        assert_eq!(title, "3: vim - first");
    }

    #[test]
    fn folder_and_exec_use_process_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let folder = cwd.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(build(&[TabTitleBlock::Folder]), folder);
        assert_eq!(
            build(&[TabTitleBlock::Exec("pwd".into())]),
            cwd.display().to_string()
        );
    }

    #[test]
    fn folder_name_shortens_home_and_root() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(folder_name(Path::new(&home)), "~");
        assert_eq!(folder_name(Path::new("/")), "/");
        assert_eq!(folder_name(Path::new("/usr/lib")), "lib");
    }

    #[test]
    fn prompt_is_user_at_host() {
        let prompt = build(&[TabTitleBlock::Prompt]);
        let (user, host) = prompt.split_once('@').unwrap();
        assert_eq!(user, user_name());
        assert!(!user.is_empty());
        assert!(!host.is_empty());
    }

    #[test]
    fn failing_and_hung_commands_are_empty() {
        assert_eq!(build(&[TabTitleBlock::Exec("exit 1".into())]), "");
        let start = Instant::now();
        assert_eq!(build(&[TabTitleBlock::Exec("sleep 10".into())]), "");
        assert_eq!(
            build(&[TabTitleBlock::Exec("echo hi; sleep 10 &".into())]),
            ""
        );
        assert!(start.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn exec_timeout_is_bounded() {
        // output printed before the hang is dropped too
        let start = Instant::now();
        assert_eq!(
            build(&[TabTitleBlock::Exec("echo early; sleep 10".into())]),
            ""
        );
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn exec_background_child_holding_stdout_is_bounded() {
        // sh exits right away but a background child keeps the pipe open, the title must not wait for it
        let start = Instant::now();
        let _ = build(&[TabTitleBlock::Exec("echo hi; sleep 8 &".into())]);
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "took {:?}",
            start.elapsed()
        );
    }
}
