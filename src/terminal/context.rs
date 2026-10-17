//! pane context: where a pane runs (an ssh host, a container, sudo) and what it works with
//! (kube context, aws profile, python venv, git repository), read from the process tree,
//! the environment and the folder of the program in front, with no prompt plugin

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use super::{ForegroundProcess, children, process_info};

// programs nested deeper than this under the one in front are not looked at
const MAX_DEPTH: usize = 4;

/// where the program in front of a pane runs
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ContextKind {
    #[default]
    Local,
    Ssh {
        host: String,
    },
    Container {
        runtime: String,
        name: String,
    },
    /// runs as another user through sudo, run0 or doas
    Elevated {
        tool: String,
    },
}

/// where a pane runs and what it works with, empty fields when unknown
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaneContext {
    pub kind: ContextKind,
    pub kube_context: Option<String>,
    pub aws_profile: Option<String>,
    pub venv: Option<String>,
    pub git_root: Option<PathBuf>,
}

impl PaneContext {
    /// context of a pane whose program in front is `process`
    pub fn read(process: Option<&ForegroundProcess>) -> Self {
        let Some(process) = process else {
            return Self::default();
        };
        let env = environ(process.pid);
        let home = env
            .get("HOME")
            .cloned()
            .or_else(|| std::env::var("HOME").ok());
        Self {
            kind: tree_kind(process),
            kube_context: kube_context(&env, home.as_deref().map(Path::new)),
            aws_profile: aws_profile(&env),
            venv: venv(&env),
            git_root: process.cwd.as_deref().and_then(git_root),
        }
    }

    /// short text for the event bus, like "ssh web1 kube prod", empty when local with nothing
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        match &self.kind {
            ContextKind::Local => {}
            ContextKind::Ssh { host } => parts.push(format!("ssh {host}")),
            ContextKind::Container { runtime, name } => parts.push(format!("{runtime} {name}")),
            ContextKind::Elevated { tool } => parts.push(tool.clone()),
        }
        let named = [
            ("kube", &self.kube_context),
            ("aws", &self.aws_profile),
            ("venv", &self.venv),
        ];
        for (label, value) in named {
            if let Some(value) = value {
                parts.push(format!("{label} {value}"));
            }
        }
        parts.join(" ")
    }
}

/// kind of the program in front or of one it started, the innermost remote one winning, so
/// `sudo ssh host` is on the host
fn tree_kind(process: &ForegroundProcess) -> ContextKind {
    let mut found = ContextKind::Local;
    let mut current = Some((process.pid, process.name.clone(), process.args.clone()));
    for _ in 0..MAX_DEPTH {
        let Some((pid, name, args)) = current.take() else {
            break;
        };
        if let Some(kind) = kind_of(&name, &args) {
            let remote = !matches!(kind, ContextKind::Elevated { .. });
            if remote || found == ContextKind::Local {
                found = kind;
            }
        }
        current = children(pid)
            .first()
            .and_then(|&child| process_info(child))
            .map(|info| (info.pid, info.name, info.args));
    }
    found
}

/// what program `name` run with `args` says about where it runs, none for an ordinary one
pub fn kind_of(name: &str, args: &[String]) -> Option<ContextKind> {
    let container = |runtime: &str, name: String| ContextKind::Container {
        runtime: runtime.to_string(),
        name,
    };
    match name {
        "ssh" => {
            // options that take a value, from ssh(1)
            let host = positional(args, "BbcDEeFIiJLlmOopQRSWw", &[])?;
            // user@host shows as host, the user is rarely what tells panes apart
            let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
            Some(ContextKind::Ssh {
                host: host.to_string(),
            })
        }
        "docker" | "podman" => {
            let args = match args.first().map(String::as_str) {
                Some("container") => &args[1..],
                _ => args,
            };
            if args.first().map(String::as_str) != Some("exec") {
                return None;
            }
            let long = [
                "--env",
                "--env-file",
                "--user",
                "--workdir",
                "--detach-keys",
            ];
            let target = positional(&args[1..], "euw", &long)?;
            Some(container(name, target.to_string()))
        }
        "kubectl" | "oc" => {
            if args.first().map(String::as_str) != Some("exec") {
                return None;
            }
            let long = ["--container", "--namespace", "--context", "--kubeconfig"];
            let pod = positional(&args[1..], "cn", &long)?;
            let pod = pod.strip_prefix("pod/").unwrap_or(pod);
            Some(container("kubectl", pod.to_string()))
        }
        "distrobox" | "toolbox" => {
            if args.first().map(String::as_str) != Some("enter") {
                return None;
            }
            let rest = &args[1..];
            let named = rest
                .iter()
                .position(|arg| arg == "--name" || arg == "-n" || arg == "--container")
                .and_then(|at| rest.get(at + 1));
            let target = named
                .map(String::as_str)
                .or_else(|| positional(rest, "", &[]))
                .unwrap_or(if name == "distrobox" {
                    "my-distrobox"
                } else {
                    "toolbox"
                });
            Some(container(name, target.to_string()))
        }
        "sudo" | "run0" | "doas" | "su" => Some(ContextKind::Elevated {
            tool: name.to_string(),
        }),
        _ => None,
    }
}

/// first argument that is no option and no option's value. `short` lists the one letter
/// options taking a value, `long` the long ones written apart from it
fn positional<'a>(args: &'a [String], short: &str, long: &[&str]) -> Option<&'a str> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            return args.next().map(String::as_str);
        }
        if let Some(name) = arg.strip_prefix("--") {
            if !name.contains('=') && long.contains(&arg.as_str()) {
                args.next();
            }
            continue;
        }
        if let Some(flags) = arg.strip_prefix('-') {
            // in "-it" or "-p22" the value, if any, follows the letter taking it
            let taking = flags.char_indices().find(|(_, c)| short.contains(*c));
            if let Some((at, c)) = taking
                && at + c.len_utf8() == flags.len()
            {
                args.next();
            }
            continue;
        }
        return Some(arg);
    }
    None
}

/// environment of process `pid`, empty where it can't be read
fn environ(pid: u32) -> HashMap<String, String> {
    #[cfg(target_os = "linux")]
    if let Ok(bytes) = std::fs::read(format!("/proc/{pid}/environ")) {
        return parse_environ(&bytes);
    }
    let _ = pid;
    HashMap::new()
}

/// `NAME=value` entries split by nul bytes, as /proc keeps them
fn parse_environ(bytes: &[u8]) -> HashMap<String, String> {
    bytes
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (name, value) = entry.split_once('=')?;
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

fn non_empty(env: &HashMap<String, String>, name: &str) -> Option<String> {
    env.get(name).filter(|value| !value.is_empty()).cloned()
}

fn aws_profile(env: &HashMap<String, String>) -> Option<String> {
    non_empty(env, "AWS_PROFILE").or_else(|| non_empty(env, "AWS_DEFAULT_PROFILE"))
}

/// name of the active python environment, like the prompt of `activate` shows it
fn venv(env: &HashMap<String, String>) -> Option<String> {
    if let Some(path) = non_empty(env, "VIRTUAL_ENV") {
        // newer venvs say how they want to be called, like "(project) "
        if let Some(prompt) = non_empty(env, "VIRTUAL_ENV_PROMPT") {
            let prompt = prompt.trim().trim_start_matches('(').trim_end_matches(')');
            if !prompt.is_empty() {
                return Some(prompt.to_string());
            }
        }
        return Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
    }
    non_empty(env, "CONDA_DEFAULT_ENV")
}

/// current context of the first kubeconfig, like `kubectl config current-context` reads it
fn kube_context(env: &HashMap<String, String>, home: Option<&Path>) -> Option<String> {
    let file = match non_empty(env, "KUBECONFIG") {
        Some(list) => PathBuf::from(list.split(':').next()?),
        None => home?.join(".kube/config"),
    };
    current_context(&std::fs::read_to_string(file).ok()?)
}

/// `current-context` of a kubeconfig, read without a yaml parser since it is a top level key
fn current_context(config: &str) -> Option<String> {
    config.lines().find_map(|line| {
        let value = line.strip_prefix("current-context:")?.trim();
        let value = value.trim_matches(|c| c == '"' || c == '\'');
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// the git repository `path` is in, found by walking up to a folder with `.git`
pub fn git_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn programs_tell_where_they_run() {
        let ssh = |host: &str| Some(ContextKind::Ssh { host: host.into() });
        assert_eq!(kind_of("ssh", &args("web1")), ssh("web1"));
        assert_eq!(
            kind_of("ssh", &args("-p 2222 -i key -A me@web2 uptime")),
            ssh("web2")
        );
        assert_eq!(kind_of("ssh", &args("-p2222 -v db")), ssh("db"));
        assert_eq!(kind_of("ssh", &args("-v")), None);
        let container = |runtime: &str, name: &str| {
            Some(ContextKind::Container {
                runtime: runtime.into(),
                name: name.into(),
            })
        };
        assert_eq!(
            kind_of("docker", &args("exec -it -e A=1 --user root app bash")),
            container("docker", "app")
        );
        assert_eq!(
            kind_of("podman", &args("container exec --workdir=/x -it db sh")),
            container("podman", "db")
        );
        assert_eq!(kind_of("docker", &args("run -it alpine")), None);
        assert_eq!(
            kind_of("kubectl", &args("exec -it -n prod pod/api-1 -c main -- sh")),
            container("kubectl", "api-1")
        );
        assert_eq!(
            kind_of("distrobox", &args("enter fedora")),
            container("distrobox", "fedora")
        );
        assert_eq!(
            kind_of("distrobox", &args("enter")),
            container("distrobox", "my-distrobox")
        );
        assert_eq!(
            kind_of("sudo", &args("-i")),
            Some(ContextKind::Elevated {
                tool: "sudo".into()
            })
        );
        assert_eq!(kind_of("vim", &args("ssh")), None);
    }

    #[test]
    fn environment_gives_venv_aws_and_kube() {
        let parsed = parse_environ(b"A=1\0VIRTUAL_ENV=/x/venv\0EMPTY=\0B=c=d\0");
        assert_eq!(parsed["B"], "c=d");
        assert_eq!(venv(&parsed).as_deref(), Some("venv"));
        let named = env(&[
            ("VIRTUAL_ENV", "/p/.venv"),
            ("VIRTUAL_ENV_PROMPT", "(proj) "),
        ]);
        assert_eq!(venv(&named).as_deref(), Some("proj"));
        assert_eq!(
            venv(&env(&[("CONDA_DEFAULT_ENV", "base")])).as_deref(),
            Some("base")
        );
        assert_eq!(venv(&env(&[])), None);

        assert_eq!(
            aws_profile(&env(&[("AWS_PROFILE", "dev")])).as_deref(),
            Some("dev")
        );
        assert_eq!(
            aws_profile(&env(&[("AWS_PROFILE", ""), ("AWS_DEFAULT_PROFILE", "ci")])).as_deref(),
            Some("ci")
        );

        let dir = std::env::temp_dir().join(format!("kuterm_ctx_{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".kube")).unwrap();
        std::fs::write(
            dir.join(".kube/config"),
            "apiVersion: v1\ncontexts:\n- name: a\ncurrent-context: \"prod\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("other"), "current-context: staging\n").unwrap();
        assert_eq!(kube_context(&env(&[]), Some(&dir)).as_deref(), Some("prod"));
        let list = format!("{}:/nowhere", dir.join("other").display());
        assert_eq!(
            kube_context(&env(&[("KUBECONFIG", &list)]), Some(&dir)).as_deref(),
            Some("staging")
        );
        assert_eq!(current_context("current-context: \n"), None);
        assert_eq!(kube_context(&env(&[]), None), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn summary_joins_what_is_known() {
        let context = PaneContext {
            kind: ContextKind::Ssh { host: "web".into() },
            kube_context: Some("prod".into()),
            venv: Some("venv".into()),
            ..PaneContext::default()
        };
        assert_eq!(context.summary(), "ssh web kube prod venv venv");
        assert_eq!(PaneContext::default().summary(), "");
    }

    #[test]
    fn git_roots_are_found_by_their_git_folder() {
        let root = std::env::temp_dir().join(format!("kuterm-git-root-{}", std::process::id()));
        let deep = root.join("src/a/b");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert_eq!(git_root(&deep), Some(root.clone()));
        assert_eq!(git_root(&root), Some(root.clone()));
        std::fs::remove_dir_all(&root).ok();
        // the temp folder itself is in no repository
        assert_eq!(
            git_root(&std::env::temp_dir().join("no-such-project")),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_tree_and_environ_of_a_child_are_read() {
        // a child named like ssh under a shell, with a venv in its environment
        let dir = std::env::temp_dir().join(format!("kuterm_ctx_ssh_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ssh = dir.join("ssh");
        std::fs::remove_file(&ssh).ok();
        // a shell started as "ssh", so argv[0] names it like the real one
        std::os::unix::fs::symlink("/bin/sh", &ssh).unwrap();
        let script = format!(
            "VIRTUAL_ENV=/x/venv {} -c 'sleep 10; :' web9; :",
            ssh.display()
        );
        let parent = crate::terminal::spawn(&script, 1);
        let shell = process_info(parent.0.id()).unwrap();
        let context = PaneContext::read(Some(&shell));
        assert_eq!(
            context.kind,
            ContextKind::Ssh {
                host: "web9".into()
            }
        );
        // the shell's own environment has no venv, the child's has
        let child = process_info(children(shell.pid)[0]).unwrap();
        assert_eq!(
            PaneContext::read(Some(&child)).venv.as_deref(),
            Some("venv")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn containers_need_their_enter_verb() {
        let container = |runtime: &str, name: &str| {
            Some(ContextKind::Container {
                runtime: runtime.into(),
                name: name.into(),
            })
        };
        // only exec and enter go into a container, other verbs run on this machine
        assert_eq!(kind_of("kubectl", &args("get pods")), None);
        assert_eq!(kind_of("oc", &args("logs api")), None);
        assert_eq!(kind_of("distrobox", &args("list")), None);
        assert_eq!(
            kind_of("toolbox", &args("enter")),
            container("toolbox", "toolbox")
        );
        assert_eq!(
            kind_of("toolbox", &args("enter --container dev")),
            container("toolbox", "dev")
        );
        // after "--" the next word is the target even when it looks like an option
        assert_eq!(
            kind_of("ssh", &args("-v -- -odd-host")),
            Some(ContextKind::Ssh {
                host: "-odd-host".into()
            })
        );
        assert_eq!(kind_of("ssh", &args("-v --")), None);
    }

    #[test]
    fn summary_names_containers_and_elevation() {
        let context = PaneContext {
            kind: ContextKind::Container {
                runtime: "podman".into(),
                name: "db".into(),
            },
            aws_profile: Some("dev".into()),
            ..PaneContext::default()
        };
        assert_eq!(context.summary(), "podman db aws dev");
        let context = PaneContext {
            kind: ContextKind::Elevated {
                tool: "sudo".into(),
            },
            ..PaneContext::default()
        };
        assert_eq!(context.summary(), "sudo");
        // nothing in front, like a pane whose shell is gone
        assert_eq!(PaneContext::read(None), PaneContext::default());
    }

    #[test]
    fn empty_venv_prompt_falls_back_to_the_folder_name() {
        let named = env(&[("VIRTUAL_ENV", "/p/.venv"), ("VIRTUAL_ENV_PROMPT", "() ")]);
        assert_eq!(venv(&named).as_deref(), Some(".venv"));
    }
}
