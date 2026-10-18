//! messages between the shell's line editor and kuterm. the shell sends one json object per
//! line, kuterm answers with plain text lines a shell script can split without a json parser

use std::path::PathBuf;

use serde::Deserialize;

pub const VERSION: u32 = 1;

/// longest line read from a shell, longer ones are dropped
pub const MAX_LINE: usize = 64 * 1024;

/// what a shell said, `conn` tells shells of the same tab apart, like a nested one
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellEvent {
    pub conn: u64,
    pub message: ShellMessage,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShellMessage {
    /// a shell connected
    Hello { shell: String },
    /// PATH of the shell changed
    Env { path: String },
    /// the command line being edited, `seq` grows with every change
    Line {
        seq: u64,
        buffer: String,
        /// in chars
        cursor: usize,
        cwd: PathBuf,
        /// the change came from moving through history, like the up arrow, so the popup
        /// stays out of the way of scrolling it
        #[serde(default)]
        history: bool,
    },
    /// what the shell did with the accept for line `seq`: not applied when its line had
    /// changed since
    Answered { seq: u64, applied: bool },
    /// the line was run or dropped, the editor is gone until the next prompt
    End,
    /// the shell went away, never sent by one
    #[serde(skip)]
    Closed,
}

#[derive(Deserialize)]
struct Envelope {
    v: u32,
    #[serde(flatten)]
    message: ShellMessage,
}

/// a line from a shell, checked: known version, cursor inside the buffer, absolute folder
pub fn parse(line: &str) -> Result<ShellMessage, String> {
    let envelope: Envelope = serde_json_lenient::from_str(line).map_err(|e| e.to_string())?;
    if envelope.v != VERSION {
        return Err(format!("unknown protocol version {}", envelope.v));
    }
    if let ShellMessage::Line {
        buffer,
        cursor,
        cwd,
        ..
    } = &envelope.message
    {
        let chars = buffer.chars().count();
        if *cursor > chars {
            return Err(format!(
                "cursor {cursor} is past the {chars} chars of the line"
            ));
        }
        if !cwd.is_absolute() {
            return Err(format!("folder {} is not absolute", cwd.display()));
        }
    }
    Ok(envelope.message)
}

/// tells the shell to put `text` in place of chars `start..end` of the line it sent as `seq`.
/// none when `text` has a control character, which would end the message or edit the line
pub fn accept(seq: u64, start: usize, end: usize, text: &str) -> Option<String> {
    if text.is_empty() || text.chars().any(char::is_control) || start > end {
        return None;
    }
    Some(format!("{VERSION} accept {seq} {start} {end} {text}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_messages_are_checked() {
        let line = r#"{"v":1,"type":"line","seq":3,"cursor":2,"buffer":"gé x","cwd":"/tmp"}"#;
        assert_eq!(
            parse(line),
            Ok(ShellMessage::Line {
                seq: 3,
                buffer: "gé x".into(),
                cursor: 2,
                cwd: "/tmp".into(),
                history: false,
            })
        );
        // a shell that moves through history says so, others leave it out
        assert!(matches!(
            parse(
                r#"{"v":1,"type":"line","seq":4,"cursor":6,"buffer":"cat al","cwd":"/","history":true}"#
            ),
            Ok(ShellMessage::Line { history: true, .. })
        ));
        // the cursor counts chars, so it can be at the end of a line with multibyte chars
        assert!(
            parse(r#"{"v":1,"type":"line","seq":1,"cursor":2,"buffer":"éé","cwd":"/"}"#).is_ok()
        );
        for bad in [
            r#"{"v":1,"type":"line","seq":1,"cursor":3,"buffer":"éé","cwd":"/"}"#,
            r#"{"v":1,"type":"line","seq":1,"cursor":0,"buffer":"","cwd":"rel"}"#,
            r#"{"v":1,"type":"line","seq":-1,"cursor":0,"buffer":"","cwd":"/"}"#,
            r#"{"v":2,"type":"end"}"#,
            r#"{"type":"end"}"#,
            r#"{"v":1,"type":"launch"}"#,
            r#"{"v":1,"type":"closed"}"#,
            "not json",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(parse(r#"{"v":1,"type":"end"}"#), Ok(ShellMessage::End));
        assert_eq!(
            parse(r#"{"v":1,"type":"answered","seq":4,"applied":false}"#),
            Ok(ShellMessage::Answered {
                seq: 4,
                applied: false
            })
        );
        assert_eq!(
            parse(r#"{"v":1,"type":"hello","shell":"zsh"}"#),
            Ok(ShellMessage::Hello {
                shell: "zsh".into()
            })
        );
    }

    #[test]
    fn accepted_text_is_one_line() {
        assert_eq!(
            accept(4, 0, 2, "git ").as_deref(),
            Some("1 accept 4 0 2 git \n")
        );
        assert_eq!(accept(4, 0, 2, "a\nrm -rf ~"), None);
        assert_eq!(accept(4, 0, 2, "a\x1b[A"), None);
        assert_eq!(accept(4, 3, 2, "a"), None);
    }
}
