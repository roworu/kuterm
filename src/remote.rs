//! remote control: other processes drive a running kuterm through a unix socket, one json
//! line in and one json line out per connection. `kuterm msg ...` is the client

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::Values;

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
        /// name `--pane` finds the tab by
        #[serde(default)]
        name: Option<String>,
    },
    /// type text into a pane, the active tab without one
    SendText {
        text: String,
        #[serde(default)]
        pane: Option<String>,
    },
    ListTabs,
    /// what people see in a pane, the screen or the whole history
    GetText {
        #[serde(default)]
        pane: Option<String>,
        #[serde(default)]
        scrollback: bool,
    },
    FocusTab {
        #[serde(default)]
        pane: Option<String>,
    },
    CloseTab {
        #[serde(default)]
        pane: Option<String>,
    },
    /// split a pane, "right", "left", "up" or "down"
    Split {
        #[serde(default)]
        pane: Option<String>,
        direction: String,
    },
    SetTabTitle {
        #[serde(default)]
        pane: Option<String>,
        title: String,
    },
    /// draw a tab in a "#rrggbb" color, none takes it back
    SetTabColor {
        #[serde(default)]
        pane: Option<String>,
        #[serde(default)]
        color: Option<String>,
    },
    /// run a command palette command by name on a pane, with values for its inputs
    RunCommand {
        #[serde(default)]
        pane: Option<String>,
        name: String,
        #[serde(default)]
        inputs: Values,
    },
    /// the commands a pane ran: the last finished one, or one counted like kout does
    Query {
        #[serde(default)]
        pane: Option<String>,
        /// 1 based block number, negative counts back from the last finished one
        #[serde(default)]
        block: Option<i64>,
        /// every block instead of one
        #[serde(default)]
        all: bool,
    },
    /// keep the connection and get one json line per event of these names, all without any
    Subscribe {
        #[serde(default)]
        events: Vec<String>,
    },
    /// type a command into a pane and run it, after the user allowed it in the window
    Run {
        #[serde(default)]
        pane: Option<String>,
        command: String,
        /// "mcp" when an ai agent asks through `kuterm mcp`, so its commands are marked
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client: Option<String>,
    },
    /// type a command into a pane once its prompt comes back, after the ones queued before.
    /// "@18:00 cmd" waits for that time too
    Queue {
        #[serde(default)]
        pane: Option<String>,
        command: String,
    },
    /// open a new window, used by `--single-instance`
    NewWindow {
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    /// open a `kuterm://run` link, used when one is opened while kuterm runs
    OpenUrl {
        url: String,
    },
}

impl Request {
    /// the `--pane` the request acts on, none for the active tab or requests about no pane
    pub fn pane(&self) -> Option<&str> {
        match self {
            Request::SendText { pane, .. }
            | Request::GetText { pane, .. }
            | Request::FocusTab { pane }
            | Request::CloseTab { pane }
            | Request::Split { pane, .. }
            | Request::SetTabTitle { pane, .. }
            | Request::SetTabColor { pane, .. }
            | Request::RunCommand { pane, .. }
            | Request::Query { pane, .. }
            | Request::Run { pane, .. }
            | Request::Queue { pane, .. } => pane.as_deref(),
            _ => None,
        }
    }
}

/// one command a pane ran, as `query` tells it
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BlockInfo {
    pub id: u64,
    /// 1 based number among the blocks of its pane still in history
    pub number: usize,
    pub command: String,
    pub exit: Option<i32>,
    pub cwd: Option<PathBuf>,
    /// unix time it started, in seconds
    pub started: f64,
    /// seconds it ran, none while it runs
    pub seconds: Option<f64>,
    /// what people see of its output
    pub output: Option<String>,
    /// the output held text people don't see, like concealed text, which was left out
    pub hidden: bool,
    /// `kuterm://block/<id>`, a click on it in a pane jumps to the block
    pub link: String,
    /// what typed its command, like ["keyboard"] or ["remote"]
    #[serde(default)]
    pub source: Vec<crate::terminal::InputSource>,
}

/// one open tab, as `list-tabs` tells it
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TabInfo {
    /// id `--pane` takes, the tab's shell has it in $KUTERM_PANE
    pub id: u64,
    /// name given to the tab, `--pane` takes it too
    pub name: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<BlockInfo>,
    /// the text held text people don't see, which was left out
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
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
  new-tab [--cwd <dir>] [--profile <name>] [--name <name>]   open a tab in the active window
  send-text <text>                    type text into a pane
  list-tabs [--json]                  print the open tabs
  new-window [--cwd <dir>]            open a window
  get-text [--scrollback]             print what is shown in a pane, or its whole history
  focus-tab                           switch to a pane
  close-tab                           close a pane, asking first like a click on its x
  split <right|left|up|down>          split a pane
  set-tab-title <title>               give a tab a title, empty takes it back
  set-tab-color [#rrggbb]             draw a tab in a color, none takes it back
  run-command <name> [--input k=v...] run a command palette command, like \"clear history\".
                                      --input fills one of its inputs, a command needing
                                      more than its inputs give fails
  query [--last-output] [--block <n>] [--all] [--json]
                                      the commands a pane ran: command, exit, folder, times
                                      and output. --block 3 is the third, --block -2 the one
                                      before the last finished one
  subscribe [<event>...] [--json]     print events as they happen, like command_finished or
                                      bell, all of them without names
  queue <command>                     run a command once the prompt is back, after the ones
                                      queued before. \"@18:00 <command>\" also waits for 18:00

commands acting on a pane take --pane <name, title or id>, the active tab without it. ids
are in list-tabs and in $KUTERM_PANE of the pane's shell. text people don't see, like
concealed text, is left out of get-text and query.

kuterm needs \"remote_control\": \"socket\" in its settings. $KUTERM_SOCKET picks the
socket, otherwise the newest running kuterm is used";

/// the request `kuterm msg` arguments ask for, and whether to print json
pub fn parse_msg(args: &[String]) -> Result<(Request, bool), String> {
    let (command, rest) = args
        .split_first()
        .ok_or_else(|| "msg needs a command".to_string())?;
    let mut cwd = None;
    let mut profile = None;
    let mut name = None;
    let mut pane = None;
    let mut json = false;
    let mut scrollback = false;
    let mut block = None;
    let mut all = false;
    let mut inputs = Values::new();
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
            "--name" => name = Some(value()?),
            "--pane" => pane = Some(value()?),
            "--json" => json = true,
            "--scrollback" => scrollback = true,
            "--all" => all = true,
            "--last-output" => block = Some(-1),
            "--input" => {
                let pair = value()?;
                let Some((name, value)) = pair.split_once('=') else {
                    return Err(format!("--input takes name=value, not {pair:?}"));
                };
                inputs.insert(name.to_string(), value.to_string());
            }
            "--block" => {
                let number = value()?;
                block = Some(
                    number
                        .parse()
                        .map_err(|_| format!("--block takes a number, not {number:?}"))?,
                );
            }
            _ => words.push(arg.clone()),
        }
    }
    let text = words.join(" ");
    let request = match command.as_str() {
        "new-tab" => Request::NewTab { cwd, profile, name },
        "send-text" => Request::SendText { text, pane },
        "list-tabs" => Request::ListTabs,
        "new-window" => Request::NewWindow { cwd },
        "get-text" => Request::GetText { pane, scrollback },
        "focus-tab" => Request::FocusTab { pane },
        "close-tab" => Request::CloseTab { pane },
        "split" => match text.as_str() {
            "right" | "left" | "up" | "down" => Request::Split {
                pane,
                direction: text,
            },
            _ => return Err("split takes right, left, up or down".into()),
        },
        "set-tab-title" => Request::SetTabTitle { pane, title: text },
        "set-tab-color" => Request::SetTabColor {
            pane,
            color: (!text.is_empty()).then_some(text),
        },
        "run-command" if !text.is_empty() => Request::RunCommand {
            pane,
            name: text,
            inputs,
        },
        "run-command" => return Err("run-command needs the name of a command".into()),
        "query" => Request::Query { pane, block, all },
        "queue" if !text.is_empty() => Request::Queue {
            pane,
            command: text,
        },
        "queue" => return Err("queue needs a command".into()),
        "subscribe" => Request::Subscribe { events: words },
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

    fn connect(path: &Path, request: &Request) -> std::io::Result<(Reply, BufReader<UnixStream>)> {
        let mut stream = UnixStream::connect(path)?;
        // a run waits for the user to allow it, events come whenever they happen
        let timeout = match request {
            Request::Run { .. } | Request::Subscribe { .. } => None,
            _ => Some(CLIENT_TIMEOUT),
        };
        stream.set_read_timeout(timeout)?;
        let mut line = serde_json_lenient::to_string(request).map_err(std::io::Error::other)?;
        line.push('\n');
        stream.write_all(line.as_bytes())?;
        let mut reader = BufReader::new(stream);
        let mut reply = String::new();
        reader.read_line(&mut reply)?;
        let reply = serde_json_lenient::from_str(&reply).map_err(std::io::Error::other)?;
        Ok((reply, reader))
    }

    /// send `request` to the newest running kuterm that answers
    pub fn send(request: &Request) -> Result<Reply, String> {
        for path in candidates() {
            if let Ok((reply, _)) = connect(&path, request) {
                return Ok(reply);
            }
        }
        Err("no running kuterm with \"remote_control\": \"socket\" found".into())
    }

    /// subscribe to events and give each json line to `line` until the connection ends or
    /// `line` returns false
    pub fn subscribe(request: &Request, mut line: impl FnMut(&str) -> bool) -> Result<(), String> {
        for path in candidates() {
            let Ok((reply, mut reader)) = connect(&path, request) else {
                continue;
            };
            if !reply.ok {
                return Err(reply.error.unwrap_or_else(|| "failed".into()));
            }
            let mut text = String::new();
            loop {
                text.clear();
                match reader.read_line(&mut text) {
                    Ok(0) | Err(_) => return Ok(()),
                    Ok(_) if !line(text.trim_end()) => return Ok(()),
                    Ok(_) => {}
                }
            }
        }
        Err("no running kuterm with \"remote_control\": \"socket\" found".into())
    }

    /// listen on this process's socket, `handle` answers each request on the main thread,
    /// right away or later through the sender, like after the user allowed it. the socket file
    /// is removed when the app quits
    pub fn listen(
        cx: &mut App,
        handle: fn(Request, oneshot::Sender<Reply>, &mut App),
    ) -> Option<PathBuf> {
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
                cx.update(|cx| handle(request, reply, cx));
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
            // events go out from here, the main thread only publishes them
            Ok(Request::Subscribe { events }) => {
                stream_events(writer, &events);
                return;
            }
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

    /// write one json line per published event named in `names`, any without names, until
    /// the client goes away
    fn stream_events(mut writer: UnixStream, names: &[String]) {
        let events = crate::events_bus::EventBus::subscribe();
        if writer.write_all(b"{\"ok\":true}\n").is_err() {
            return;
        }
        for event in futures::executor::block_on_stream(events) {
            if !names.is_empty() && !names.contains(&event.name()) {
                continue;
            }
            let Ok(mut json) = serde_json_lenient::to_string(&event) else {
                continue;
            };
            json.push('\n');
            if writer.write_all(json.as_bytes()).is_err() {
                return;
            }
        }
    }
}

#[cfg(unix)]
pub use unix::{listen, send, subscribe};

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
    if let Request::Subscribe { .. } = &request {
        #[cfg(unix)]
        let result = subscribe(&request, |line| {
            use std::io::Write;
            let mut stdout = std::io::stdout();
            writeln!(stdout, "{line}")
                .and_then(|()| stdout.flush())
                .is_ok()
        });
        #[cfg(not(unix))]
        let result: Result<(), String> = Err("remote control needs unix sockets".into());
        return match result {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{error}");
                1
            }
        };
    }
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
                    "{mark} {} {}:{}:{} {}{cwd}",
                    tab.id, tab.window, tab.pane, tab.number, tab.title
                );
            }
        }
    }
    if let Some(text) = &reply.text {
        println!("{text}");
    }
    if let Request::Query { all, .. } = &request {
        if json {
            let blocks = if *all {
                serde_json_lenient::to_string_pretty(&reply.blocks)
            } else {
                serde_json_lenient::to_string_pretty(&reply.blocks.first())
            };
            println!("{}", blocks.unwrap_or_default());
        } else {
            for block in &reply.blocks {
                if *all {
                    let exit = block.exit.map_or("-".into(), |exit| exit.to_string());
                    println!("{} [{exit}] {}", block.number, block.command);
                } else if let Some(output) = &block.output {
                    println!("{output}");
                }
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Request, parse_msg};
    use crate::settings::Values;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn msg_arguments_become_requests() {
        assert_eq!(
            parse_msg(&args("new-tab --cwd /tmp --profile dev --name build")).unwrap(),
            (
                Request::NewTab {
                    cwd: Some(PathBuf::from("/tmp")),
                    profile: Some("dev".into()),
                    name: Some("build".into()),
                },
                false
            )
        );
        assert_eq!(
            parse_msg(&args("send-text --pane build echo hi"))
                .unwrap()
                .0,
            Request::SendText {
                text: "echo hi".into(),
                pane: Some("build".into()),
            }
        );
        let pane = || Some("3".to_string());
        for (line, want) in [
            (
                "get-text --pane 3 --scrollback",
                Request::GetText {
                    pane: pane(),
                    scrollback: true,
                },
            ),
            ("focus-tab --pane 3", Request::FocusTab { pane: pane() }),
            ("close-tab --pane 3", Request::CloseTab { pane: pane() }),
            (
                "split down --pane 3",
                Request::Split {
                    pane: pane(),
                    direction: "down".into(),
                },
            ),
            (
                "set-tab-title --pane 3 my build",
                Request::SetTabTitle {
                    pane: pane(),
                    title: "my build".into(),
                },
            ),
            (
                "set-tab-color --pane 3 #ff0000",
                Request::SetTabColor {
                    pane: pane(),
                    color: Some("#ff0000".into()),
                },
            ),
            (
                "queue --pane 3 @18:00 make deploy",
                Request::Queue {
                    pane: pane(),
                    command: "@18:00 make deploy".into(),
                },
            ),
            (
                "set-tab-color --pane 3",
                Request::SetTabColor {
                    pane: pane(),
                    color: None,
                },
            ),
            (
                "run-command --pane 3 clear history",
                Request::RunCommand {
                    pane: pane(),
                    name: "clear history".into(),
                    inputs: Values::new(),
                },
            ),
            (
                "run-command checkout branch --input branch=main",
                Request::RunCommand {
                    pane: None,
                    name: "checkout branch".into(),
                    inputs: Values::from([("branch".to_string(), "main".to_string())]),
                },
            ),
            (
                // only the first "=" splits, the rest belongs to the value
                "run-command x --input msg=a=b --input n=1",
                Request::RunCommand {
                    pane: None,
                    name: "x".into(),
                    inputs: Values::from([
                        ("msg".to_string(), "a=b".to_string()),
                        ("n".to_string(), "1".to_string()),
                    ]),
                },
            ),
            (
                "query --pane 3 --last-output",
                Request::Query {
                    pane: pane(),
                    block: Some(-1),
                    all: false,
                },
            ),
            (
                "query --block -2",
                Request::Query {
                    pane: None,
                    block: Some(-2),
                    all: false,
                },
            ),
            (
                "subscribe command_finished bell",
                Request::Subscribe {
                    events: vec!["command_finished".into(), "bell".into()],
                },
            ),
        ] {
            assert_eq!(parse_msg(&args(line)).unwrap().0, want, "{line}");
        }
        assert!(parse_msg(&args("split sideways")).is_err());
        assert!(parse_msg(&args("run-command")).is_err());
        assert!(parse_msg(&args("run-command x --input novalue")).is_err());
        assert!(parse_msg(&args("query --block x")).is_err());
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
        let json = serde_json_lenient::to_string(&Request::Run {
            pane: Some("2".into()),
            command: "ls".into(),
            client: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"cmd":"run","pane":"2","command":"ls"}"#);
        // replies leave out what they don't carry
        assert_eq!(
            serde_json_lenient::to_string(&super::Reply::done()).unwrap(),
            r#"{"ok":true}"#
        );
    }
}
