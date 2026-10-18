//! the unix socket of a running kuterm: one json line in and one json line out per
//! connection, or a stream of events for `subscribe`

use std::path::PathBuf;

use super::protocol::{Reply, Request};

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

#[cfg(unix)]
mod unix {
    use std::{
        io::{BufRead, BufReader, Read, Write},
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

    const NO_KUTERM: &str = "no running kuterm with \"remote_control\": \"socket\" found";

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
        Err(NO_KUTERM.into())
    }

    /// subscribe to events and give each json line to `line` until the connection ends or
    /// `line` returns false
    pub fn subscribe(request: &Request, mut line: impl FnMut(&str) -> bool) -> Result<(), String> {
        for path in candidates() {
            let Ok((reply, reader)) = connect(&path, request) else {
                continue;
            };
            if !reply.ok {
                return Err(reply.error.unwrap_or_else(|| "failed".into()));
            }
            for text in reader.lines().map_while(Result::ok) {
                if !line(text.trim_end()) {
                    break;
                }
            }
            return Ok(());
        }
        Err(NO_KUTERM.into())
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
        let events = super::super::events::EventBus::subscribe();
        if writer.write_all(b"{\"ok\":true}\n").is_err() {
            return;
        }
        for event in futures::executor::block_on_stream(events) {
            // only a write tells a client went away, and filtered events write nothing
            if client_gone(&writer) {
                return;
            }
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

    // true when the client closed its end, it sends nothing after its request
    fn client_gone(stream: &UnixStream) -> bool {
        if stream.set_nonblocking(true).is_err() {
            return true;
        }
        let gone = match (&*stream).read(&mut [0u8; 1]) {
            Ok(read) => read == 0,
            Err(error) => !matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ),
        };
        stream.set_nonblocking(false).is_err() || gone
    }
}

#[cfg(unix)]
pub use unix::{listen, send, subscribe};

#[cfg(not(unix))]
pub fn send(_: &Request) -> Result<Reply, String> {
    Err("remote control needs unix sockets".into())
}

#[cfg(not(unix))]
pub fn subscribe(_: &Request, _: impl FnMut(&str) -> bool) -> Result<(), String> {
    Err("remote control needs unix sockets".into())
}
