//! the private socket of one tab: its shells connect, report their command line, and get
//! picked completions back. every tab has its own socket, so a shell always talks to its tab

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use super::protocol::{self, MAX_LINE, ShellEvent, ShellMessage};

/// env var shells find the socket in
pub const SOCKET_ENV: &str = "KUTERM_COMPLETION_SOCKET";

// a shell that stops reading must never stall the ui
const WRITE_TIMEOUT: Duration = Duration::from_millis(100);

static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);

/// folder of this process's sockets, only this user can enter it
fn socket_dir() -> std::io::Result<PathBuf> {
    let dir = crate::automation::socket::runtime_dir()
        .join(format!("kuterm-complete-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    // fails when someone else made it first
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

struct Shared {
    closed: AtomicBool,
    /// write side of every connected shell
    writers: Mutex<HashMap<u64, UnixStream>>,
}

pub struct Bridge {
    path: PathBuf,
    shared: Arc<Shared>,
    events: Option<UnboundedReceiver<ShellEvent>>,
}

impl Bridge {
    /// listen on a new socket
    pub fn new() -> std::io::Result<Bridge> {
        let n = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        let path = socket_dir()?.join(format!("{n}.sock"));
        std::fs::remove_file(&path).ok();
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let shared = Arc::new(Shared {
            closed: AtomicBool::new(false),
            writers: Mutex::new(HashMap::new()),
        });
        let (tx, rx) = unbounded();
        let accepting = shared.clone();
        std::thread::Builder::new()
            .name("completion socket".into())
            .spawn(move || {
                let mut next_conn = 0;
                for stream in listener.incoming() {
                    if accepting.closed.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let conn = next_conn;
                    next_conn += 1;
                    let Ok(writer) = stream.try_clone() else {
                        continue;
                    };
                    writer.set_write_timeout(Some(WRITE_TIMEOUT)).ok();
                    accepting.writers.lock().unwrap().insert(conn, writer);
                    let (tx, shared) = (tx.clone(), accepting.clone());
                    std::thread::spawn(move || {
                        read_shell(stream, conn, &tx);
                        shared.writers.lock().unwrap().remove(&conn);
                        tx.unbounded_send(ShellEvent {
                            conn,
                            message: ShellMessage::Closed,
                        })
                        .ok();
                    });
                }
            })?;
        Ok(Bridge {
            path,
            shared,
            events: Some(rx),
        })
    }

    /// the socket shells connect to
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// what the shells say, can be taken once
    pub fn take_events(&mut self) -> Option<UnboundedReceiver<ShellEvent>> {
        self.events.take()
    }

    /// send a line to the shell of `conn`, false when it is gone or did not take it
    pub fn send(&self, conn: u64, line: &str) -> bool {
        let mut writers = self.shared.writers.lock().unwrap();
        writers
            .get_mut(&conn)
            .is_some_and(|writer| writer.write_all(line.as_bytes()).is_ok())
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        // wakes the accepting thread, which then sees it is closed
        UnixStream::connect(&self.path).ok();
        std::fs::remove_file(&self.path).ok();
    }
}

/// pass every valid line of a shell on until it hangs up. bad lines are skipped, the
/// connection stays, so a shell never writes into a closed socket
fn read_shell(stream: UnixStream, conn: u64, tx: &UnboundedSender<ShellEvent>) {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    loop {
        line.clear();
        let mut too_long = false;
        // reads up to the newline, keeping at most MAX_LINE bytes
        loop {
            let Ok(buf) = reader.fill_buf() else { return };
            if buf.is_empty() {
                return;
            }
            let newline = buf.iter().position(|&b| b == b'\n');
            let chunk = &buf[..newline.unwrap_or(buf.len())];
            if line.len() + chunk.len() > MAX_LINE {
                too_long = true;
            } else {
                line.extend_from_slice(chunk);
            }
            let used = newline.map_or(buf.len(), |ix| ix + 1);
            reader.consume(used);
            if newline.is_some() {
                break;
            }
        }
        if too_long {
            eprintln!("completion: dropped a line over {MAX_LINE} bytes from a shell");
            continue;
        }
        let Ok(text) = std::str::from_utf8(&line) else {
            continue;
        };
        match protocol::parse(text) {
            Ok(message) => {
                if tx.unbounded_send(ShellEvent { conn, message }).is_err() {
                    return;
                }
            }
            Err(error) => eprintln!("completion: bad message from a shell: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use futures::StreamExt;

    use super::*;

    fn next(events: &mut UnboundedReceiver<ShellEvent>) -> ShellEvent {
        futures::executor::block_on(events.next()).expect("bridge closed")
    }

    #[test]
    fn shells_report_and_get_answers() {
        let mut bridge = Bridge::new().unwrap();
        let mode = std::fs::metadata(bridge.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let mut events = bridge.take_events().unwrap();
        assert!(bridge.take_events().is_none());

        let mut shell = UnixStream::connect(bridge.path()).unwrap();
        shell
            .write_all(b"{\"v\":1,\"type\":\"hello\",\"shell\":\"zsh\"}\n")
            .unwrap();
        let hello = next(&mut events);
        assert_eq!(
            hello.message,
            ShellMessage::Hello {
                shell: "zsh".into()
            }
        );

        // bad, not utf-8, oversized and unknown lines are skipped, the connection stays
        shell.write_all(b"garbage\n").unwrap();
        shell
            .write_all(b"{\"v\":1,\"type\":\"hello\",\"shell\":\"z\xffsh\"}\n")
            .unwrap();
        shell.write_all(&vec![b'x'; MAX_LINE + 10]).unwrap();
        shell.write_all(b"\n{\"v\":9,\"type\":\"end\"}\n").unwrap();
        shell
            .write_all(b"{\"v\":1,\"type\":\"line\",\"seq\":1,\"cursor\":2,\"buffer\":\"gi\",\"cwd\":\"/\"}\n")
            .unwrap();
        let line = next(&mut events);
        assert_eq!(line.conn, hello.conn);
        assert!(matches!(
            line.message,
            ShellMessage::Line {
                seq: 1,
                cursor: 2,
                ..
            }
        ));

        // a second shell, like a nested one, is told apart
        let mut nested = UnixStream::connect(bridge.path()).unwrap();
        nested.write_all(b"{\"v\":1,\"type\":\"end\"}\n").unwrap();
        let end = next(&mut events);
        assert_ne!(end.conn, hello.conn);

        assert!(bridge.send(hello.conn, "1 accept 1 0 2 git \n"));
        let mut got = [0; 20];
        shell.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"1 accept 1 0 2 git \n");

        drop(nested);
        assert_eq!(
            next(&mut events),
            ShellEvent {
                conn: end.conn,
                message: ShellMessage::Closed
            }
        );
        assert!(!bridge.send(end.conn, "x\n"));

        let path = bridge.path().to_path_buf();
        drop(bridge);
        assert!(!path.exists());
    }
}
