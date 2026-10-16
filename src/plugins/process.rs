//! a plugin's process: started with pipes, read and written on threads of its own, so a
//! plugin that hangs or floods never holds up the ui

use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use futures::channel::mpsc::UnboundedSender;
use serde_json_lenient::Value;

use super::{
    framing::{Decoded, Decoder, encode},
    spec::PluginSpec,
};

// first restart waits this long, each next one twice as long up to the cap
const BACKOFF_START: Duration = Duration::from_millis(500);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

/// what came from a plugin, tagged with the generation of the process that sent it
#[derive(Debug, PartialEq)]
pub enum FromPlugin {
    Message(Value),
    /// bytes that were no message, with why
    Broken(String),
    /// its output closed, it exited or crashed
    Exited,
}

/// the wait before restart number `attempt`, 0 being the first
pub fn backoff(attempt: u32) -> Duration {
    BACKOFF_START
        .saturating_mul(2u32.saturating_pow(attempt.min(16)))
        .min(BACKOFF_CAP)
}

/// a running plugin, killed when dropped
pub struct PluginProcess {
    child: Child,
    writer: mpsc::Sender<Vec<u8>>,
}

impl PluginProcess {
    /// start `spec`, its messages go to `tx` tagged with `generation`
    pub fn spawn(
        spec: &PluginSpec,
        generation: u64,
        tx: UnboundedSender<(u64, FromPlugin)>,
    ) -> std::io::Result<Self> {
        let (program, args) = spec
            .command
            .split_first()
            .ok_or_else(|| std::io::Error::other("the plugin has no command"))?;
        // settings name scripts like "~/plugins/x.py", no shell expands that here
        let home = std::env::var("HOME").ok();
        let expand = |arg: &String| match (arg.strip_prefix("~/"), &home) {
            (Some(rest), Some(home)) => format!("{home}/{rest}"),
            _ => arg.clone(),
        };
        let mut command = Command::new(expand(program));
        command
            .args(args.iter().map(expand))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // its logs go where kuterm's go
            .stderr(Stdio::inherit())
            .env("KUTERM_PLUGIN", &spec.name);
        if let Some(dir) = &spec.dir {
            command.current_dir(dir);
        }
        let mut child = command.spawn()?;
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let mut stdin = child.stdin.take().expect("stdin is piped");
        std::thread::Builder::new()
            .name(format!("plugin {} reader", spec.name))
            .spawn(move || {
                let mut decoder = Decoder::default();
                let mut buf = [0u8; 8192];
                loop {
                    let read = match stdout.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => read,
                    };
                    for decoded in decoder.feed(&buf[..read]) {
                        let message = match decoded {
                            Decoded::Message(value) => FromPlugin::Message(value),
                            Decoded::Broken(why) => FromPlugin::Broken(why),
                        };
                        if tx.unbounded_send((generation, message)).is_err() {
                            return;
                        }
                    }
                }
                tx.unbounded_send((generation, FromPlugin::Exited)).ok();
            })?;
        let (writer, writes) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name(format!("plugin {} writer", spec.name))
            .spawn(move || {
                for bytes in writes {
                    if stdin
                        .write_all(&bytes)
                        .and_then(|()| stdin.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Self { child, writer })
    }

    /// queue `message` for the plugin, never waiting for it to read
    pub fn send(&self, message: &Value) {
        self.writer.send(encode(message)).ok();
    }
}

impl Drop for PluginProcess {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use futures::channel::mpsc::unbounded;
    use serde_json_lenient::json;

    use super::{FromPlugin, PluginProcess, backoff};
    use crate::plugins::{permissions::Permissions, spec::PluginSpec};

    #[test]
    fn restarts_back_off_up_to_the_cap() {
        assert_eq!(backoff(0), Duration::from_millis(500));
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(3), Duration::from_secs(4));
        assert_eq!(backoff(10), Duration::from_secs(30));
        assert_eq!(backoff(u32::MAX), Duration::from_secs(30));
    }

    fn sh(script: &str) -> PluginSpec {
        PluginSpec {
            name: "test".into(),
            command: vec!["sh".into(), "-c".into(), script.into()],
            permissions: Permissions::default(),
            dir: None,
        }
    }

    fn next(
        rx: &mut futures::channel::mpsc::UnboundedReceiver<(u64, FromPlugin)>,
    ) -> (u64, FromPlugin) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(got) = rx.try_recv() {
                return got;
            }
            assert!(Instant::now() < deadline, "nothing from the plugin");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn messages_go_both_ways_and_an_exit_is_told() {
        let (tx, mut rx) = unbounded();
        // cat sends back whatever it gets, framed as it came
        let plugin = PluginProcess::spawn(&sh("cat"), 7, tx).unwrap();
        let message = json!({"jsonrpc": "2.0", "method": "ping"});
        plugin.send(&message);
        assert_eq!(next(&mut rx), (7, FromPlugin::Message(message)));
        drop(plugin);
        assert_eq!(next(&mut rx), (7, FromPlugin::Exited));

        let (tx, mut rx) = unbounded();
        let _crashing =
            PluginProcess::spawn(&sh("printf 'garbage\\r\\n\\r\\n'; exit 3"), 8, tx).unwrap();
        assert!(matches!(next(&mut rx), (8, FromPlugin::Broken(_))));
        assert_eq!(next(&mut rx), (8, FromPlugin::Exited));
    }
}
