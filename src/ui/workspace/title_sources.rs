//! what tab title blocks read from the system: user, host, folder, git, clock and commands

use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

// a hung command would freeze every tab title, so it is abandoned after this
pub(super) const EXEC_TIMEOUT: Duration = Duration::from_secs(2);

// git status walks the whole work tree, too slow to run on every title refresh
pub(super) const GIT_CACHE_FOR: Duration = Duration::from_secs(2);

// nerd font branch
pub(super) const GIT_ICON: &str = "\u{e0a0}";

pub(super) fn prompt() -> String {
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
pub(super) fn system_host() -> Option<String> {
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
pub(super) fn system_host() -> Option<String> {
    None
}

// USER is often unset in containers and services, so fall back to other vars and /etc/passwd
pub(super) fn user_name() -> String {
    ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .find_map(|var| std::env::var(var).ok().filter(|name| !name.is_empty()))
        .or_else(passwd_user)
        .unwrap_or_default()
}

pub(super) fn passwd_user() -> Option<String> {
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

pub(super) fn folder_name(cwd: &Path) -> String {
    if std::env::var_os("HOME").is_some_and(|home| cwd == Path::new(&home)) {
        return "~".to_string();
    }
    cwd.file_name().map_or_else(
        || cwd.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// branch like " main* ↑1 ↓2", none outside a repository. cached for a moment per folder
pub(super) fn git(cwd: &Path) -> Option<String> {
    // most folders are no repository, finding that out needs no git process
    crate::terminal::git_root(cwd)?;
    // when each folder was read, and what it gave
    type Cache = HashMap<PathBuf, (Instant, Option<String>)>;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    if let Some((at, text)) = CACHE.lock().unwrap().get_or_insert_default().get(cwd)
        && at.elapsed() < GIT_CACHE_FOR
    {
        return text.clone();
    }
    // no locks, so a refresh never gets in the way of git commands typed in the tab
    let output = run(
        "git --no-optional-locks status --porcelain=v2 --branch 2>/dev/null",
        Some(cwd),
    );
    let text = output.as_deref().and_then(git_status);
    let mut cache = CACHE.lock().unwrap();
    let cache = cache.get_or_insert_default();
    // folders of closed tabs would pile up
    if cache.len() > 64 {
        cache.clear();
    }
    cache.insert(cwd.to_path_buf(), (Instant::now(), text.clone()));
    text
}

/// branch text from `git status --porcelain=v2 --branch` output
pub(super) fn git_status(output: &str) -> Option<String> {
    let mut oid = "";
    let mut head = None;
    let (mut ahead, mut behind) = (0, 0);
    let mut dirty = false;
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("# branch.oid ") {
            oid = value;
        } else if let Some(value) = line.strip_prefix("# branch.head ") {
            head = Some(value);
        } else if let Some(value) = line.strip_prefix("# branch.ab ") {
            let mut counts = value
                .split_whitespace()
                .map(|n| n.parse::<i32>().unwrap_or(0));
            ahead = counts.next().unwrap_or(0);
            behind = counts.next().unwrap_or(0);
        } else if !line.starts_with('#') && !line.is_empty() {
            dirty = true;
        }
    }
    let head = match head? {
        // a detached head shows its commit, like git's own prompt does
        "(detached)" => oid.get(..7).unwrap_or(oid),
        head => head,
    };
    let mut text = format!("{GIT_ICON} {head}");
    if dirty {
        text.push('*');
    }
    if ahead > 0 {
        text.push_str(&format!(" ↑{ahead}"));
    }
    if behind < 0 {
        text.push_str(&format!(" ↓{}", -behind));
    }
    Some(text)
}

/// time of day now in strftime `format`
pub(super) fn clock(format: &str) -> String {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let Ok(format) = std::ffi::CString::new(format) else {
            return String::new();
        };
        let mut buf = [0u8; 128];
        let written = unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                return String::new();
            }
            libc::strftime(buf.as_mut_ptr().cast(), buf.len(), format.as_ptr(), &tm)
        };
        String::from_utf8_lossy(&buf[..written]).into_owned()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = format;
        String::new()
    }
}

pub(super) fn exec(command: &str, cwd: Option<&Path>) -> Option<String> {
    Some(run(command, cwd)?.lines().next()?.trim().to_string())
}

/// whole output of a shell command, none when it failed to start or ran too long
pub(super) fn run(command: &str, cwd: Option<&Path>) -> Option<String> {
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
    Some(String::from_utf8_lossy(&output).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_is_user_at_host() {
        let prompt = prompt();
        let (user, host) = prompt.split_once('@').unwrap();
        assert_eq!(user, user_name());
        assert!(!user.is_empty());
        assert!(!host.is_empty());
    }

    #[test]
    fn folder_name_shortens_home_and_root() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(folder_name(Path::new(&home)), "~");
        assert_eq!(folder_name(Path::new("/")), "/");
        assert_eq!(folder_name(Path::new("/usr/lib")), "lib");
    }

    #[test]
    fn failing_and_hung_commands_are_empty() {
        assert_eq!(exec("exit 1", None), None);
        // output printed before the hang is dropped too, and a background child holding the
        // pipe open is not waited for
        for command in ["echo early; sleep 10", "echo hi; sleep 8 &"] {
            let start = Instant::now();
            assert_eq!(exec(command, None), None, "{command}");
            assert!(
                start.elapsed() < Duration::from_secs(4),
                "{command} took {:?}",
                start.elapsed()
            );
        }
    }

    #[test]
    fn git_status_shows_branch_changes_and_upstream() {
        let clean = "# branch.oid 1234567890abcdef\n# branch.head main\n";
        assert_eq!(git_status(clean).unwrap(), format!("{GIT_ICON} main"));
        let busy = "# branch.oid 1234567890abcdef\n# branch.head dev\n\
            # branch.upstream origin/dev\n# branch.ab +2 -1\n1 .M N... 1 1 1 a b src/x.rs\n";
        assert_eq!(git_status(busy).unwrap(), format!("{GIT_ICON} dev* ↑2 ↓1"));
        let detached = "# branch.oid 1234567890abcdef\n# branch.head (detached)\n? new\n";
        assert_eq!(
            git_status(detached).unwrap(),
            format!("{GIT_ICON} 1234567*")
        );
        assert_eq!(git_status(""), None);
    }

    #[test]
    fn git_block_reads_the_repository_of_the_folder() {
        let dir = std::env::temp_dir().join(format!("kuterm-git-block-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(git(&dir), None);
        let ok = Command::new("git")
            .args(["init", "-q", "-b", "trunk"])
            .current_dir(&dir)
            .status()
            .is_ok_and(|status| status.success());
        // git is in the dev image, a host without it has nothing to check
        if ok {
            // the "no repository" answer is cached for a moment
            std::thread::sleep(GIT_CACHE_FOR);
            assert_eq!(git(&dir), Some(format!("{GIT_ICON} trunk")));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clock_follows_its_format() {
        let time = clock("%H:%M");
        assert_eq!(time.len(), 5, "{time}");
        assert_eq!(&time[2..3], ":");
        assert_eq!(clock("text"), "text");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn passwd_user_names_the_user_this_process_runs_as() {
        // SAFETY: getpwuid returns a static entry or null, read before any other call
        let expected = unsafe {
            let entry = libc::getpwuid(libc::getuid());
            (!entry.is_null()).then(|| {
                std::ffi::CStr::from_ptr((*entry).pw_name)
                    .to_string_lossy()
                    .into_owned()
            })
        };
        // users from ldap and the like are only known to getpwuid
        let uid = unsafe { libc::getuid() }.to_string();
        let listed = std::fs::read_to_string("/etc/passwd").is_ok_and(|passwd| {
            passwd
                .lines()
                .any(|line| line.split(':').nth(2) == Some(uid.as_str()))
        });
        assert_eq!(passwd_user(), expected.filter(|_| listed));
    }

    #[test]
    fn clock_with_a_nul_in_its_format_is_empty() {
        assert_eq!(clock("%H\0%M"), "");
    }
}
