//! what a plugin may do, asked once per plugin and kept with its hash, so a changed plugin
//! asks again

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::remote::Request;

/// what a plugin may do, each off unless its entry turns it on
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct Permissions {
    /// see the panes: their events, text and commands, and change tabs
    pub panes: bool,
    /// type into panes, which runs commands in their shells
    pub pty_write: bool,
    /// start programs, like its panel in a tab of its own
    pub run: bool,
    /// read and write the clipboard
    pub clipboard: bool,
}

impl Permissions {
    /// the permission `request` needs, none when it needs none
    fn needed(request: &Request) -> Option<(&'static str, bool)> {
        Some(match request {
            Request::SendText { .. }
            | Request::Run { .. }
            | Request::Queue { .. }
            | Request::RunCommand { .. } => ("pty_write", false),
            Request::NewTab { .. } | Request::NewWindow { .. } | Request::OpenUrl { .. } => {
                ("run", false)
            }
            // a plugin gets events as notifications, the socket's long stream is not for it
            Request::Subscribe { .. } => ("subscribe", true),
            _ => ("panes", false),
        })
    }

    /// why `request` is refused, none when it may go
    pub fn refuse(&self, request: &Request) -> Option<String> {
        let (needed, never) = Self::needed(request)?;
        if never {
            return Some(format!(
                "plugins get events as notifications, not by {needed}"
            ));
        }
        let allowed = match needed {
            "pty_write" => self.pty_write,
            "run" => self.run,
            _ => self.panes,
        };
        (!allowed).then(|| format!("the plugin has no \"{needed}\" permission"))
    }

    /// what the permissions allow, for the question asked before a plugin first runs
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.panes {
            parts.push("see your panes");
        }
        if self.pty_write {
            parts.push("type into them");
        }
        if self.run {
            parts.push("start programs");
        }
        if self.clipboard {
            parts.push("use the clipboard");
        }
        if parts.is_empty() {
            "nothing beyond its own commands".into()
        } else {
            parts.join(", ")
        }
    }
}

/// plugins the user allowed, by name, with the hash they were allowed at
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrustStore {
    trusted: BTreeMap<String, String>,
}

fn store_path() -> Option<PathBuf> {
    Some(crate::settings::state_dir()?.join("plugins-trust.json"))
}

impl TrustStore {
    /// the stored trust, empty when there is none
    pub fn load() -> Self {
        store_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json_lenient::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = store_path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let json = serde_json_lenient::to_string_pretty(self).unwrap_or_default();
        if let Err(error) = std::fs::write(&path, json) {
            eprintln!("failed to write {}: {error}", path.display());
        }
    }

    /// true when plugin `name` was allowed as it is now
    pub fn trusts(&self, name: &str, hash: &str) -> bool {
        self.trusted.get(name).is_some_and(|known| known == hash)
    }

    pub fn trust(&mut self, name: &str, hash: &str) {
        self.trusted.insert(name.to_string(), hash.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::{Permissions, TrustStore};
    use crate::remote::Request;

    #[test]
    fn requests_need_their_permission() {
        let none = Permissions::default();
        let send = Request::SendText {
            text: "x".into(),
            pane: None,
        };
        let read = Request::GetText {
            pane: None,
            scrollback: false,
        };
        let open = Request::NewTab {
            cwd: None,
            profile: None,
            name: None,
        };
        for request in [&send, &read, &open] {
            assert!(none.refuse(request).is_some(), "{request:?}");
        }
        let all = Permissions {
            panes: true,
            pty_write: true,
            run: true,
            clipboard: true,
        };
        for request in [&send, &read, &open] {
            assert_eq!(all.refuse(request), None, "{request:?}");
        }
        let subscribe = Request::Subscribe { events: Vec::new() };
        assert!(all.refuse(&subscribe).is_some());
        let readers = Permissions {
            panes: true,
            ..Permissions::default()
        };
        assert_eq!(readers.refuse(&read), None);
        assert!(readers.refuse(&send).unwrap().contains("pty_write"));
        assert_eq!(none.describe(), "nothing beyond its own commands");
        assert_eq!(readers.describe(), "see your panes");
    }

    #[test]
    fn trust_holds_for_one_hash() {
        let mut store = TrustStore::default();
        assert!(!store.trusts("p", "1"));
        store.trust("p", "1");
        assert!(store.trusts("p", "1"));
        assert!(!store.trusts("p", "2"));
        let json = serde_json_lenient::to_string(&store).unwrap();
        let back: TrustStore = serde_json_lenient::from_str(&json).unwrap();
        assert_eq!(back, store);
    }
}
