//! what clients send a running kuterm and what it answers, one json object each

use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::{Reply, Request};

    #[test]
    fn requests_are_one_json_object() {
        let json = serde_json_lenient::to_string(&Request::NewWindow { cwd: None }).unwrap();
        assert_eq!(json, r#"{"cmd":"new_window","cwd":null}"#);
        let json = serde_json_lenient::to_string(&Request::Run {
            pane: Some("2".into()),
            command: "ls".into(),
        })
        .unwrap();
        assert_eq!(json, r#"{"cmd":"run","pane":"2","command":"ls"}"#);
        // replies leave out what they don't carry
        assert_eq!(
            serde_json_lenient::to_string(&Reply::done()).unwrap(),
            r#"{"ok":true}"#
        );
    }

    #[test]
    fn pane_is_the_one_a_request_acts_on() {
        let pane = || Some("7".to_string());
        assert_eq!(
            Request::SetTabColor {
                pane: pane(),
                color: None
            }
            .pane(),
            Some("7")
        );
        assert_eq!(
            Request::Queue {
                pane: pane(),
                command: "ls".into()
            }
            .pane(),
            Some("7")
        );
        // requests about no pane, like opening a window, have none
        assert_eq!(Request::ListTabs.pane(), None);
        assert_eq!(Request::NewWindow { cwd: None }.pane(), None);
    }
}
