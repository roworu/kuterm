//! looks at pty output before alacritty parses it, for escape sequences alacritty skips:
//! shell integration marks (osc 133), the folder of the shell (osc 7) and notifications
//! (osc 9, osc 777), and images of kitty's graphics protocol (apc G). prompts, command output
//! and images get marked cells, so they can be found again wherever they scroll

use std::{
    fs::File,
    io::{self, Read},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

#[cfg(unix)]
use alacritty_terminal::{
    event::{OnResize, WindowSize},
    tty::{ChildEvent, EventedPty, EventedReadWrite, Pty},
};

use super::{graphics::GraphicsParser, links::MARKER_PREFIX};

// longest osc payload looked at, longer ones pass through untouched
const MAX_PAYLOAD: usize = 4096;
// longest graphics command, base64 of the biggest image sent in one piece
const MAX_APC: usize = 96 << 20;

/// something the program told the terminal through an escape sequence alacritty skips
#[derive(Clone, Debug, PartialEq)]
pub enum ScanEvent {
    /// the shell is about to draw its prompt
    PromptStart,
    /// a command started running, its output follows
    CommandStart,
    /// the command finished, with its exit code when the shell told it
    CommandEnd(Option<i32>),
    /// the shell is in this folder
    Cwd(PathBuf),
    Notify {
        title: String,
        body: String,
    },
    /// bytes to answer the program with, like the result of a graphics command
    Reply(Vec<u8>),
}

/// hyperlink ids of marked prompts and command output, numbered in the order they came
pub(crate) const PROMPT_MARK: &str = "kuterm-prompt-";
pub(crate) const OUTPUT_MARK: &str = "kuterm-output-";

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Ground,
    Escape,
    Osc,
    OscEscape,
    Apc,
    ApcEscape,
}

/// state between reads, a sequence may be split over several
pub struct Scanner {
    state: State,
    payload: Vec<u8>,
    /// payload was too long to look at
    overflow: bool,
    /// marked prompt or output cells are being printed
    open_mark: bool,
    next_mark: usize,
    /// graphics command being read, none while it is too long to keep
    apc: Option<Vec<u8>>,
    graphics: Option<GraphicsParser>,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new(None)
    }
}

const CLOSE_MARK: &[u8] = b"\x1b]8;;\x1b\\";

impl Scanner {
    /// scanner that runs graphics commands with `graphics`, or leaves them to alacritty,
    /// which skips them
    pub fn new(graphics: Option<GraphicsParser>) -> Self {
        Self {
            state: State::Ground,
            payload: Vec::new(),
            overflow: false,
            open_mark: false,
            next_mark: 0,
            apc: None,
            graphics,
        }
    }

    /// copy `input` to `out`, adding marks after the sequences they belong to and taking out
    /// graphics commands, and collect what the program told. an escape at the very end is
    /// held back until the next read tells what it starts
    pub fn feed(&mut self, input: &[u8], out: &mut Vec<u8>, events: &mut Vec<ScanEvent>) {
        let mut ix = 0;
        while ix < input.len() {
            let byte = input[ix];
            ix += 1;
            self.state = match (self.state, byte) {
                (State::Ground, 0x1b) => State::Escape,
                (State::Ground, _) => {
                    out.push(byte);
                    State::Ground
                }
                (State::Escape, b'_')
                    if self.graphics.as_ref().is_some_and(GraphicsParser::enabled) =>
                {
                    self.apc = Some(Vec::new());
                    State::Apc
                }
                (State::Escape, b']') => {
                    out.extend_from_slice(b"\x1b]");
                    self.payload.clear();
                    self.overflow = false;
                    State::Osc
                }
                (State::Escape, 0x1b) => {
                    out.push(0x1b);
                    State::Escape
                }
                (State::Escape, _) => {
                    out.extend_from_slice(&[0x1b, byte]);
                    State::Ground
                }
                (State::Osc, 0x07) => {
                    out.push(byte);
                    self.finish(out, events);
                    State::Ground
                }
                (State::Osc, 0x1b) => {
                    out.push(byte);
                    State::OscEscape
                }
                // cancel and substitute end a sequence without running it
                (State::Osc, 0x18 | 0x1a) => {
                    out.push(byte);
                    State::Ground
                }
                (State::Osc, _) => {
                    out.push(byte);
                    if self.payload.len() < MAX_PAYLOAD {
                        self.payload.push(byte);
                    } else {
                        self.overflow = true;
                    }
                    State::Osc
                }
                (State::OscEscape, b'\\') => {
                    out.push(byte);
                    self.finish(out, events);
                    State::Ground
                }
                // another escape starts a new sequence and drops this one
                (State::OscEscape, b']') => {
                    out.push(byte);
                    self.payload.clear();
                    self.overflow = false;
                    State::Osc
                }
                (State::OscEscape, _) => {
                    out.push(byte);
                    State::Ground
                }
                (State::Apc, 0x1b) => State::ApcEscape,
                (State::Apc, 0x07) => {
                    self.finish_apc(out, events);
                    State::Ground
                }
                (State::Apc, 0x18 | 0x1a) => State::Ground,
                (State::Apc, _) => {
                    if let Some(apc) = &mut self.apc {
                        apc.push(byte);
                        if apc.len() > MAX_APC {
                            self.apc = None;
                        }
                    }
                    State::Apc
                }
                (State::ApcEscape, b'\\') => {
                    self.finish_apc(out, events);
                    State::Ground
                }
                // an escape that ends nothing starts something new, the command is dropped
                (State::ApcEscape, _) => {
                    ix -= 1;
                    State::Escape
                }
            };
        }
    }

    fn finish_apc(&mut self, out: &mut Vec<u8>, events: &mut Vec<ScanEvent>) {
        let (Some(apc), Some(graphics)) = (self.apc.take(), &mut self.graphics) else {
            return;
        };
        let Some(body) = apc.strip_prefix(b"G") else {
            return;
        };
        let outcome = graphics.command(body);
        out.extend_from_slice(&outcome.output);
        if let Some(reply) = outcome.reply {
            events.push(ScanEvent::Reply(reply));
        }
    }

    fn finish(&mut self, out: &mut Vec<u8>, events: &mut Vec<ScanEvent>) {
        if self.overflow {
            return;
        }
        let payload = String::from_utf8_lossy(&self.payload).into_owned();
        let Some((code, rest)) = payload.split_once(';') else {
            return;
        };
        match code {
            "133" => {
                let mut parts = rest.split(';');
                match parts.next() {
                    Some("A") => {
                        self.close_mark(out);
                        self.open_mark(PROMPT_MARK, out);
                        events.push(ScanEvent::PromptStart);
                    }
                    // the prompt is drawn, what is typed next is no part of it
                    Some("B") => self.close_mark(out),
                    Some("C") => {
                        self.close_mark(out);
                        self.open_mark(OUTPUT_MARK, out);
                        events.push(ScanEvent::CommandStart);
                    }
                    Some("D") => {
                        self.close_mark(out);
                        let exit = parts.next().and_then(|code| code.parse().ok());
                        events.push(ScanEvent::CommandEnd(exit));
                    }
                    _ => {}
                }
            }
            "7" => {
                if let Some(path) = file_url_path(rest) {
                    events.push(ScanEvent::Cwd(path));
                }
            }
            // 9;4 is a progress report, not a notification
            "9" if !rest.starts_with("4;") => events.push(ScanEvent::Notify {
                title: String::new(),
                body: rest.to_string(),
            }),
            "777" => {
                let mut parts = rest.splitn(3, ';');
                if parts.next() == Some("notify") {
                    let title = parts.next().unwrap_or_default().to_string();
                    let body = parts.next().unwrap_or_default().to_string();
                    events.push(ScanEvent::Notify { title, body });
                }
            }
            _ => {}
        }
    }

    fn open_mark(&mut self, kind: &str, out: &mut Vec<u8>) {
        debug_assert!(kind.starts_with(MARKER_PREFIX));
        out.extend_from_slice(
            format!("\x1b]8;id={kind}{};kuterm:\x1b\\", self.next_mark).as_bytes(),
        );
        self.next_mark += 1;
        self.open_mark = true;
    }

    fn close_mark(&mut self, out: &mut Vec<u8>) {
        if std::mem::take(&mut self.open_mark) {
            out.extend_from_slice(CLOSE_MARK);
        }
    }
}

/// local path of a `file://host/path` url, percent escapes decoded
fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let mut bytes = Vec::with_capacity(path.len());
    let mut iter = path.bytes();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let hex = [iter.next()?, iter.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

/// events found since they were last taken with when they were read, shared with the
/// terminal
pub type ScanEvents = Arc<Mutex<Vec<(Instant, ScanEvent)>>>;

/// reads the pty through the scanner
pub struct ScanReader {
    file: File,
    scanner: Scanner,
    /// scanned bytes that did not fit the last read
    pending: Vec<u8>,
    raw: Vec<u8>,
    events: ScanEvents,
}

impl Read for ScanReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            // half the room, so the marks added to it nearly always fit
            let want = (buf.len() / 2).max(1);
            self.raw.resize(want, 0);
            let got = self.file.read(&mut self.raw)?;
            if got == 0 {
                return Ok(0);
            }
            let mut events = Vec::new();
            self.scanner
                .feed(&self.raw[..got], &mut self.pending, &mut events);
            if !events.is_empty() {
                let now = Instant::now();
                let mut shared = self.events.lock().unwrap();
                shared.extend(events.into_iter().map(|event| (now, event)));
            }
        }
        let n = self.pending.len().min(buf.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

/// alacritty's pty with its output going through the scanner
#[cfg(unix)]
pub struct ScannedPty {
    pty: Pty,
    reader: ScanReader,
}

#[cfg(unix)]
impl ScannedPty {
    /// wrap `pty`, events go to `events`, images to `graphics`
    pub fn new(pty: Pty, events: ScanEvents, graphics: GraphicsParser) -> io::Result<Self> {
        // a second handle on the same pty, polling still watches the original
        let file = pty.file().try_clone()?;
        Ok(Self {
            pty,
            reader: ScanReader {
                file,
                scanner: Scanner::new(Some(graphics)),
                pending: Vec::new(),
                raw: Vec::new(),
                events,
            },
        })
    }

    pub fn child_id(&self) -> u32 {
        self.pty.child().id()
    }
}

#[cfg(unix)]
impl EventedReadWrite for ScannedPty {
    type Reader = ScanReader;
    type Writer = File;

    unsafe fn register(
        &mut self,
        poll: &Arc<polling::Poller>,
        interest: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        unsafe { self.pty.register(poll, interest, mode) }
    }

    fn reregister(
        &mut self,
        poll: &Arc<polling::Poller>,
        interest: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        self.pty.reregister(poll, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<polling::Poller>) -> io::Result<()> {
        self.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut ScanReader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut File {
        self.pty.writer()
    }
}

#[cfg(unix)]
impl EventedPty for ScannedPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}

#[cfg(unix)]
impl OnResize for ScannedPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.pty.on_resize(window_size)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{ScanEvent, Scanner, file_url_path};

    fn scan(chunks: &[&[u8]]) -> (Vec<u8>, Vec<ScanEvent>) {
        let mut scanner = Scanner::default();
        let (mut out, mut events) = (Vec::new(), Vec::new());
        for chunk in chunks {
            scanner.feed(chunk, &mut out, &mut events);
        }
        (out, events)
    }

    #[test]
    fn shell_marks_become_events_and_marked_cells() {
        let (out, events) = scan(&[
            b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133",
            b";C\x07file\r\n\x1b]133;D;2\x1b\\",
        ]);
        assert_eq!(
            events,
            [
                ScanEvent::PromptStart,
                ScanEvent::CommandStart,
                ScanEvent::CommandEnd(Some(2))
            ]
        );
        let out = String::from_utf8(out).unwrap();
        assert_eq!(
            out,
            "\x1b]133;A\x07\x1b]8;id=kuterm-prompt-0;kuterm:\x1b\\$ \x1b]133;B\x07\x1b]8;;\x1b\\ls\r\n\
             \x1b]133;C\x07\x1b]8;id=kuterm-output-1;kuterm:\x1b\\file\r\n\x1b]133;D;2\x1b\\\x1b]8;;\x1b\\"
        );
    }

    #[test]
    fn cwd_and_notifications_are_reported() {
        let (out, events) = scan(&[
            b"\x1b]7;file://host/tmp/a%20b\x07",
            b"\x1b]9;build done\x07\x1b]9;4;1;50\x07",
            b"\x1b]777;notify;title;body; with semicolon\x1b\\",
        ]);
        assert_eq!(
            events,
            [
                ScanEvent::Cwd(PathBuf::from("/tmp/a b")),
                ScanEvent::Notify {
                    title: String::new(),
                    body: "build done".into()
                },
                ScanEvent::Notify {
                    title: "title".into(),
                    body: "body; with semicolon".into()
                },
            ]
        );
        // nothing is added or dropped
        assert!(out.starts_with(b"\x1b]7;file://host/tmp/a%20b\x07\x1b]9;"));
        assert!(file_url_path("http://x/y").is_none());
    }

    #[test]
    fn long_or_cancelled_sequences_are_skipped() {
        let mut long = b"\x1b]9;".to_vec();
        long.extend(std::iter::repeat_n(b'x', super::MAX_PAYLOAD + 10));
        long.push(0x07);
        let (out, events) = scan(&[&long, b"\x1b]9;gone\x18\x1b]133;D\x07"]);
        assert_eq!(events, [ScanEvent::CommandEnd(None)]);
        assert_eq!(
            out.len(),
            long.len() + "\x1b]9;gone\x18\x1b]133;D\x07".len()
        );
    }

    #[test]
    fn graphics_commands_are_taken_out_and_answered() {
        use std::sync::{Arc, Mutex};

        use crate::terminal::graphics::{CellSize, Graphics, GraphicsParser};

        let shared = Arc::new(Mutex::new(Graphics::new(true, 10)));
        let parser = GraphicsParser::new(shared.clone(), Arc::new(CellSize::default()));
        let mut scanner = Scanner::new(Some(parser));
        let (mut out, mut events) = (Vec::new(), Vec::new());
        // split over reads, even between the escape and its underscore
        for chunk in [
            &b"a\x1b"[..],
            b"_Ga=q,i=4,s=1,v=1,f=24;AAA",
            b"A\x1b",
            b"\\b",
        ] {
            scanner.feed(chunk, &mut out, &mut events);
        }
        assert_eq!(out, b"ab");
        assert_eq!(events, [ScanEvent::Reply(b"\x1b_Gi=4;OK\x1b\\".to_vec())]);

        // other apc sequences are dropped, an escape that ends nothing starts a sequence
        let (mut out, mut events) = (Vec::new(), Vec::new());
        scanner.feed(b"\x1b_Xjunk\x1b\\c\x1b_Gq\x1b[1m", &mut out, &mut events);
        assert_eq!(out, b"c\x1b[1m");

        // turned off, they go on to alacritty
        shared.lock().unwrap().configure(false, 10);
        let (mut out, mut events) = (Vec::new(), Vec::new());
        scanner.feed(b"\x1b_Ga=q\x1b\\", &mut out, &mut events);
        assert_eq!(out, b"\x1b_Ga=q\x1b\\");
        assert!(events.is_empty());
    }
}
