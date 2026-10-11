//! command palette pins changed from the palette, remembered between launches

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{App, Global};
use serde::{Deserialize, Serialize};

use super::{Command, state_dir};

/// pin choices made in the palette, keyed by command label
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Pins {
    /// command label to the pinned state picked in the palette
    #[serde(default)]
    pub overrides: HashMap<String, bool>,
}

impl Global for Pins {}

impl Pins {
    /// `pinned_commands.json` in the state dir
    pub fn path() -> Option<PathBuf> {
        Some(state_dir()?.join("pinned_commands.json"))
    }

    /// load pins from the file, empty when it is missing or invalid
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(json) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json_lenient::from_str(&json).unwrap_or_else(|error| {
            eprintln!("invalid pins in {}: {error}", path.display());
            Self::default()
        })
    }

    /// write pins to the state dir
    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                serde_json_lenient::to_string_pretty(self).map_err(std::io::Error::other)
            })
            .and_then(|json| std::fs::write(&path, json));
        if let Err(error) = result {
            eprintln!("failed to write {}: {error}", path.display());
        }
    }

    /// pinned state of a command, the palette choice wins over the command default
    pub fn is_pinned(&self, command: &Command) -> bool {
        self.overrides
            .get(&command.label())
            .copied()
            .unwrap_or(command.pinned)
    }

    /// remember the pinned state picked for a command
    pub fn set(&mut self, command: &Command, pinned: bool) {
        let label = command.label();
        // an override equal to the command default is dropped, so editing that default works
        if pinned == command.pinned {
            self.overrides.remove(&label);
        } else {
            self.overrides.insert(label, pinned);
        }
    }

    /// pins loaded at startup
    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CommandAction;
    use crate::settings::tests::{temp_dir, with_config_home};

    fn command(pinned: bool) -> Command {
        Command {
            name: "reload".into(),
            category: Some("config".into()),
            pinned,
            actions: vec![CommandAction::ReloadConfig],
        }
    }

    #[test]
    fn save_reports_write_errors() {
        let dir = temp_dir("pins_save_error");
        std::fs::create_dir_all(dir.join("state")).unwrap();
        with_config_home(&dir, || {
            // a file where the state folder should be makes create_dir_all fail
            std::fs::write(dir.join("state/kuterm"), "not a dir").unwrap();
            Pins::default().save();
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn override_wins_over_the_command_default() {
        let command = command(true);
        let mut pins = Pins::default();
        assert!(pins.is_pinned(&command));
        pins.set(&command, false);
        assert!(!pins.is_pinned(&command));
        assert_eq!(pins.overrides.get("config: reload"), Some(&false));
        // setting it back to the default drops the override
        pins.set(&command, true);
        assert!(pins.overrides.is_empty());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = temp_dir("pins_round_trip");
        with_config_home(&dir, || {
            let path = Pins::path().unwrap();
            assert_eq!(path, dir.join("state/kuterm/pinned_commands.json"));
            // missing file loads as empty
            assert_eq!(Pins::load(), Pins::default());

            let mut pins = Pins::default();
            pins.set(&command(true), false);
            pins.save();
            assert!(path.exists());
            assert_eq!(Pins::load(), pins);

            // broken file falls back to empty
            std::fs::write(&path, "{ broken").unwrap();
            assert_eq!(Pins::load(), Pins::default());
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
