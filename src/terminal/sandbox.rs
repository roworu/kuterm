//! sandboxed profiles: the shell starts with no network, writes only to its folder and a
//! private /tmp. bubblewrap on linux, sandbox-exec on macos, refused when neither exists

use std::path::{Path, PathBuf};

use crate::settings::{Sandbox, SandboxTmp, Shell};

/// the program and arguments that run `program args` in `sandbox`, writing to `cwd` and
/// reading `keep` (shell integration, kuterm itself) even when /tmp is private
pub fn bwrap_command(
    sandbox: &Sandbox,
    program: &str,
    args: &[String],
    cwd: &Path,
    home: Option<&Path>,
    keep: &[PathBuf],
) -> (String, Vec<String>) {
    let mut out: Vec<String> = ["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc"]
        .map(str::to_string)
        .to_vec();
    if sandbox.tmp == SandboxTmp::Private {
        out.extend(["--tmpfs".into(), "/tmp".into()]);
    }
    for path in write_paths(sandbox, cwd, home) {
        let path = path.display().to_string();
        out.extend(["--bind".into(), path.clone(), path]);
    }
    // a private /tmp hides anything kuterm keeps there, put it back read only
    for path in keep.iter().filter(|path| path.exists()) {
        let path = path.display().to_string();
        out.extend(["--ro-bind".into(), path.clone(), path]);
    }
    if !sandbox.network {
        out.push("--unshare-net".into());
    }
    out.extend([
        "--die-with-parent".into(),
        "--chdir".into(),
        cwd.display().to_string(),
        "--".into(),
        program.to_string(),
    ]);
    out.extend(args.iter().cloned());
    ("bwrap".into(), out)
}

/// folders the sandbox may write, "cwd" standing for the folder it starts in
fn write_paths(sandbox: &Sandbox, cwd: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    sandbox
        .write
        .iter()
        .filter_map(|entry| match entry.as_str() {
            "cwd" => Some(cwd.to_path_buf()),
            path => match (path.strip_prefix("~/"), home) {
                (Some(rest), Some(home)) => Some(home.join(rest)),
                _ => Some(PathBuf::from(path)).filter(|path| path.is_absolute()),
            },
        })
        .collect()
}

/// sbpl profile for macos `sandbox-exec -p`, the same rules as the bwrap ones
pub fn sbpl_profile(sandbox: &Sandbox, cwd: &Path, home: Option<&Path>) -> String {
    let quote = |path: &Path| format!("{:?}", path.display().to_string());
    let mut profile = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
    let mut writes: Vec<PathBuf> = write_paths(sandbox, cwd, home);
    if sandbox.tmp == SandboxTmp::Private {
        // macos has no private mounts, writes go to the per user temp folder only
        writes.push(PathBuf::from("/private/var/folders"));
    } else {
        writes.push(PathBuf::from("/private/tmp"));
    }
    for path in writes {
        profile.push_str(&format!("(allow file-write* (subpath {}))\n", quote(&path)));
    }
    // ttys and /dev/null stay writable, or the shell can't even print
    profile.push_str("(allow file-write* (regex #\"^/dev/\"))\n");
    if !sandbox.network {
        profile.push_str("(deny network*)\n(allow network* (local unix))\n");
    }
    profile
}

/// true when `program` is found in PATH
fn in_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// `command` wrapped to run in `sandbox` from `cwd`, or why it can't be
pub fn sandboxed(
    command: &Shell,
    sandbox: &Sandbox,
    cwd: &Path,
    keep: &[PathBuf],
) -> Result<Shell, String> {
    let (program, args) = match command {
        // the sandbox runs a program by name, so the login shell is named
        Shell::System => (
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
            Vec::new(),
        ),
        Shell::Program(program) => (program.clone(), Vec::new()),
        Shell::WithArguments { program, args } => (program.clone(), args.clone()),
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        let profile = sbpl_profile(sandbox, cwd, home.as_deref());
        let mut wrapped = vec!["-p".to_string(), profile, program];
        wrapped.extend(args);
        return Ok(Shell::WithArguments {
            program: "/usr/bin/sandbox-exec".into(),
            args: wrapped,
        });
    }
    if !in_path("bwrap") {
        return Err("the sandbox needs bubblewrap (bwrap), it is not installed".into());
    }
    let (program, args) = bwrap_command(sandbox, &program, &args, cwd, home.as_deref(), keep);
    Ok(Shell::WithArguments { program, args })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{bwrap_command, sbpl_profile};
    use crate::settings::{Sandbox, SandboxTmp};

    fn sandbox(network: bool, write: &[&str], tmp: SandboxTmp) -> Sandbox {
        Sandbox {
            network,
            write: write.iter().map(|path| path.to_string()).collect(),
            tmp,
        }
    }

    #[test]
    fn bwrap_gets_the_rules_of_the_profile() {
        let cwd = Path::new("/work/project");
        let home = Path::new("/home/me");
        let keep = [PathBuf::from("/")];
        let (program, args) = bwrap_command(
            &sandbox(false, &["cwd", "~/.cache", "relative"], SandboxTmp::Private),
            "/bin/bash",
            &["--rcfile".into(), "/state/kuterm.bash".into()],
            cwd,
            Some(home),
            &keep,
        );
        assert_eq!(program, "bwrap");
        let line = args.join(" ");
        assert!(line.starts_with("--ro-bind / / --dev /dev --proc /proc --tmpfs /tmp "));
        assert!(line.contains("--bind /work/project /work/project"));
        assert!(line.contains("--bind /home/me/.cache /home/me/.cache"));
        assert!(!line.contains("relative"));
        assert!(line.contains("--unshare-net"));
        assert!(line.contains("--die-with-parent --chdir /work/project"));
        assert!(line.ends_with("-- /bin/bash --rcfile /state/kuterm.bash"));

        let (_, args) = bwrap_command(
            &sandbox(true, &[], SandboxTmp::Shared),
            "sh",
            &[],
            cwd,
            None,
            &[],
        );
        let line = args.join(" ");
        assert!(!line.contains("--unshare-net"));
        assert!(!line.contains("--tmpfs"));
        assert!(!line.contains("--bind /work"));
    }

    #[test]
    fn macos_profile_denies_writes_and_network() {
        let profile = sbpl_profile(
            &sandbox(false, &["cwd"], SandboxTmp::Private),
            Path::new("/Users/me/p"),
            None,
        );
        assert!(profile.contains("(deny file-write*)"));
        assert!(profile.contains("(allow file-write* (subpath \"/Users/me/p\"))"));
        assert!(profile.contains("(deny network*)"));
        let open = sbpl_profile(
            &sandbox(true, &[], SandboxTmp::Shared),
            Path::new("/p"),
            None,
        );
        assert!(!open.contains("network"));
        assert!(open.contains("/private/tmp"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shells_are_named_for_bwrap() {
        use crate::settings::Shell;

        let cwd = Path::new("/work");
        let rules = sandbox(false, &[], SandboxTmp::Shared);
        let wrapped = |command: &Shell| super::sandboxed(command, &rules, cwd, &[]);
        if !super::in_path("bwrap") {
            let error = wrapped(&Shell::System).unwrap_err();
            assert!(error.contains("bubblewrap"), "{error}");
            return;
        }
        let args = |shell: Shell| match shell {
            Shell::WithArguments { program, args } => {
                assert_eq!(program, "bwrap");
                args
            }
            other => panic!("not wrapped: {other:?}"),
        };
        // a program alone runs with no arguments after "--"
        let line = args(wrapped(&Shell::Program("/bin/zsh".into())).unwrap());
        assert!(
            line.ends_with(&["--".to_string(), "/bin/zsh".to_string()]),
            "{line:?}"
        );
        // the login shell is named, bwrap can't start "the system shell"
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let line = args(wrapped(&Shell::System).unwrap());
        assert!(line.ends_with(&["--".to_string(), shell]), "{line:?}");
    }
}
