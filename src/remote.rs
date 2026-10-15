//! remote control: other processes drive a running kuterm through a unix socket, one json
//! line in and one json line out per connection. `kuterm msg ...` is the client

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// what a client asks for
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// open a tab in the active window
    NewTab {
        #[serde(default)]
        cwd: Option<PathBuf>,
        #[serde(default)]
        profile: Option<String>,
    },
    /// type text into the active tab
    SendText {
        text: String,
    },
    ListTabs,
    /// open a new window, used by `--single-instance`
    NewWindow {
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
}

/// one open tab, as `list-tabs` tells it
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TabInfo {
    pub window: u64,
    pub pane: usize,
    /// 1 based position in its pane
    pub number: usize,
    pub title: String,
    /// the tab in front of the focused pane of its window
    pub active: bool,
    pub cwd: Option<PathBuf>,
}

/// answer to a request
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<TabInfo>,
}

impl Reply {
    /// a reply telling it worked
    pub fn done() -> Self {
        Self {
            ok: true,
            ..Self::default()
        }
    }

    /// a reply telling why it failed
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(error.into()),
            ..Self::default()
        }
    }
}

/// folder sockets go in: `$XDG_RUNTIME_DIR`, the temp folder without it
pub(crate) fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(std::env::temp_dir)
}

/// socket of this process
pub fn socket_path() -> PathBuf {
    runtime_dir().join(format!("kuterm-{}.sock", std::process::id()))
}

/// usage of `kuterm msg`
pub const MSG_USAGE: &str = "usage: kuterm msg <command>

commands:
  new-tab [--cwd <dir>] [--profile <name>]   open a tab in the active window
  send-text <text>                           type text into the active tab
  list-tabs [--json]                         print the open tabs
  new-window [--cwd <dir>]                   open a window

kuterm needs \"remote_control\": \"socket\" in its settings. $KUTERM_SOCKET picks the
socket, otherwise the newest running kuterm is used";

/// the request `kuterm msg` arguments ask for, and whether to print json
pub fn parse_msg(args: &[String]) -> Result<(Request, bool), String> {
    let (command, rest) = args
        .split_first()
        .ok_or_else(|| "msg needs a command".to_string())?;
    let mut cwd = None;
    let mut profile = None;
    let mut json = false;
    let mut words = Vec::new();
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        let mut value = || {
            rest.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--cwd" => {
                let dir = value()?;
                cwd = Some(std::path::absolute(&dir).map_err(|error| format!("{dir}: {error}"))?);
            }
            "--profile" => profile = Some(value()?),
            "--json" => json = true,
            _ => words.push(arg.clone()),
        }
    }
    let request = match command.as_str() {
        "new-tab" => Request::NewTab { cwd, profile },
        "send-text" => Request::SendText {
            text: words.join(" "),
        },
        "list-tabs" => Request::ListTabs,
        "new-window" => Request::NewWindow { cwd },
        other => return Err(format!("unknown msg command {other:?}")),
    };
    Ok((request, json))
}

#[cfg(unix)]
mod unix {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
        path::{Path, PathBuf},
        time::Duration,
    };

    use futures::{
        StreamExt,
        channel::{mpsc, oneshot},
    };
    use gpui::App;

    use super::{Reply, Request, runtime_dir, socket_path};

    // a client that sends nothing must not hold the socket forever
    const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);

    /// sockets of running kuterms, newest first
    fn candidates() -> Vec<PathBuf> {
        if let Some(path) = std::env::var_os("KUTERM_SOCKET") {
            return vec![PathBuf::from(path)];
        }
        let Ok(entries) = std::fs::read_dir(runtime_dir()) else {
            return Vec::new();
        };
        let mut sockets: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("kuterm-") && name.ends_with(".sock")
            })
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .collect();
        sockets.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        sockets.into_iter().map(|(_, path)| path).collect()
    }

    fn ask(path: &Path, request: &Request) -> std::io::Result<Reply> {
        let mut stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(CLIENT_TIMEOUT))?;
        let mut line = serde_json_lenient::to_string(request).map_err(std::io::Error::other)?;
        line.push('\n');
        stream.write_all(line.as_bytes())?;
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply)?;
        serde_json_lenient::from_str(&reply).map_err(std::io::Error::other)
    }

    /// send `request` to the newest running kuterm that answers
    pub fn send(request: &Request) -> Result<Reply, String> {
        for path in candidates() {
            if let Ok(reply) = ask(&path, request) {
                return Ok(reply);
            }
        }
        Err("no running kuterm with \"remote_control\": \"socket\" found".into())
    }

    /// listen on this process's socket, `handle` answers each request on the main thread.
    /// the socket file is removed when the app quits
    pub fn listen(cx: &mut App, handle: fn(Request, &mut App) -> Reply) -> Option<PathBuf> {
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("failed to listen on {}: {error}", path.display());
                return None;
            }
        };
        // only this user may drive the terminals
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
        let (tx, mut rx) = mpsc::unbounded::<(Request, oneshot::Sender<Reply>)>();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let tx = tx.clone();
                std::thread::spawn(move || serve(stream, tx));
            }
        });
        cx.spawn(async move |cx| {
            while let Some((request, reply)) = rx.next().await {
                let answer = cx.update(|cx| handle(request, cx));
                reply.send(answer).ok();
            }
        })
        .detach();
        let removed = path.clone();
        cx.on_app_quit(move |_| {
            std::fs::remove_file(&removed).ok();
            async {}
        })
        .detach();
        Some(path)
    }

    fn serve(stream: UnixStream, tx: mpsc::UnboundedSender<(Request, oneshot::Sender<Reply>)>) {
        stream.set_read_timeout(Some(CLIENT_TIMEOUT)).ok();
        let mut line = String::new();
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        if BufReader::new(stream).read_line(&mut line).is_err() {
            return;
        }
        let reply = match serde_json_lenient::from_str::<Request>(&line) {
            Err(error) => Reply::failed(format!("bad request: {error}")),
            Ok(request) => {
                let (reply_tx, reply_rx) = oneshot::channel();
                if tx.unbounded_send((request, reply_tx)).is_err() {
                    return;
                }
                futures::executor::block_on(reply_rx)
                    .unwrap_or_else(|_| Reply::failed("kuterm is quitting"))
            }
        };
        if let Ok(mut json) = serde_json_lenient::to_string(&reply) {
            json.push('\n');
            writer.write_all(json.as_bytes()).ok();
        }
    }
}

#[cfg(unix)]
pub use unix::{listen, send};

#[cfg(not(unix))]
pub fn send(_: &Request) -> Result<Reply, String> {
    Err("remote control needs unix sockets".into())
}

/// run `kuterm msg` with its arguments, the exit code says if it worked
pub fn run_msg(args: &[String]) -> i32 {
    let (request, json) = match parse_msg(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{error}\n\n{MSG_USAGE}");
            return 2;
        }
    };
    let reply = match send(&request) {
        Ok(reply) => reply,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    if !reply.ok {
        eprintln!("{}", reply.error.unwrap_or_else(|| "failed".into()));
        return 1;
    }
    if request == Request::ListTabs {
        if json {
            println!(
                "{}",
                serde_json_lenient::to_string_pretty(&reply.tabs).unwrap_or_default()
            );
        } else {
            for tab in &reply.tabs {
                let mark = if tab.active { "*" } else { " " };
                let cwd = tab.cwd.as_deref().map(Path::display);
                let cwd = cwd.map(|cwd| format!("  {cwd}")).unwrap_or_default();
                println!(
                    "{mark} {}:{}:{} {}{cwd}",
                    tab.window, tab.pane, tab.number, tab.title
                );
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Request, parse_msg};

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn msg_arguments_become_requests() {
        assert_eq!(
            parse_msg(&args("new-tab --cwd /tmp --profile dev")).unwrap(),
            (
                Request::NewTab {
                    cwd: Some(PathBuf::from("/tmp")),
                    profile: Some("dev".into())
                },
                false
            )
        );
        assert_eq!(
            parse_msg(&args("send-text echo hi")).unwrap().0,
            Request::SendText {
                text: "echo hi".into()
            }
        );
        assert_eq!(
            parse_msg(&args("list-tabs --json")).unwrap(),
            (Request::ListTabs, true)
        );
        assert!(parse_msg(&args("fly")).is_err());
        assert!(parse_msg(&args("new-tab --cwd")).is_err());
        assert!(parse_msg(&[]).is_err());
    }

    #[test]
    fn requests_are_one_json_object() {
        let json = serde_json_lenient::to_string(&Request::NewWindow { cwd: None }).unwrap();
        assert_eq!(json, r#"{"cmd":"new_window","cwd":null}"#);
    }
}
