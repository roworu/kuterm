//! user keybindings, read from a jsonc file next to settings

use std::{collections::BTreeMap, path::PathBuf};

use gpui::{KeyBinding, Keystroke};
use serde::de::Error;

use super::{config_dir, load_file, parse_over};
use crate::cli::Cli;
use crate::ui::{
    terminal_view::{Copy, Paste},
    workspace::{ActivateTab, CloseTab, NewTab, NextTab, ToggleCommandPalette, ToggleFullscreen},
};

/// action name to its keys, `None` when the action is disabled
#[derive(Clone, Debug, PartialEq)]
pub struct Keybindings(BTreeMap<String, Option<String>>);

impl Default for Keybindings {
    fn default() -> Self {
        Self::parse("{}").expect("bundled default keybindings are invalid")
    }
}

const LINUX_KEYBINDINGS: &str = include_str!("../../assets/default_keybindings.jsonc");
const MACOS_KEYBINDINGS: &str = include_str!("../../assets/default_keybindings_macos.jsonc");

/// commented keybindings file written on first launch
pub const DEFAULT_KEYBINDINGS: &str = if cfg!(target_os = "macos") {
    MACOS_KEYBINDINGS
} else {
    LINUX_KEYBINDINGS
};

// keys are checked before this runs, so `KeyBinding::new` won't panic
fn binding(action: &str, keys: &str) -> Option<KeyBinding> {
    Some(match action {
        "new_tab" => KeyBinding::new(keys, NewTab, None),
        "close_tab" => KeyBinding::new(keys, CloseTab, None),
        "next_tab" => KeyBinding::new(keys, NextTab, None),
        "copy" => KeyBinding::new(keys, Copy, Some("Terminal")),
        "paste" => KeyBinding::new(keys, Paste, Some("Terminal")),
        "command_palette" => KeyBinding::new(keys, ToggleCommandPalette, None),
        "toggle_fullscreen" => KeyBinding::new(keys, ToggleFullscreen, None),
        _ => {
            let number: usize = action.strip_prefix("activate_tab_")?.parse().ok()?;
            KeyBinding::new(keys, ActivateTab(number.checked_sub(1)?), None)
        }
    })
}

impl Keybindings {
    /// `--keybindings-file`, or `keybindings.jsonc` in the config dir
    pub fn path() -> Option<PathBuf> {
        Cli::get()
            .keybindings_file
            .clone()
            .or_else(|| Some(config_dir()?.join("keybindings.jsonc")))
    }

    /// parse keybindings over the bundled ones, rejecting unknown actions and bad keys
    pub fn parse(json: &str) -> serde_json_lenient::Result<Self> {
        let bindings: BTreeMap<String, Option<String>> = parse_over(DEFAULT_KEYBINDINGS, json)?;
        for (action, keys) in &bindings {
            let Some(keys) = keys else { continue };
            if keys.trim().is_empty() {
                return Err(Error::custom(format!(
                    "empty keys for {action:?}, use null to disable it"
                )));
            }
            for key in keys.split_whitespace() {
                Keystroke::parse(key)
                    .map_err(|error| Error::custom(format!("{action:?}: {error}")))?;
            }
            if binding(action, keys).is_none() {
                return Err(Error::custom(format!("unknown action {action:?}")));
            }
        }
        Ok(Self(bindings))
    }

    /// load keybindings from the keybindings file, using defaults when it is missing or invalid
    pub fn load() -> Self {
        load_file(
            Self::path(),
            DEFAULT_KEYBINDINGS,
            "keybindings",
            Self::parse,
        )
    }

    /// gpui bindings for every enabled action
    pub fn bindings(&self) -> Vec<KeyBinding> {
        self.0
            .iter()
            .filter_map(|(action, keys)| binding(action, keys.as_deref()?))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::tests::{temp_dir, with_config_home};

    #[test]
    fn defaults_bind_every_action() {
        let keys = Keybindings::default();
        assert_eq!(keys.0["new_tab"].as_deref(), Some("ctrl-shift-t"));
        assert_eq!(keys.0["close_tab"].as_deref(), Some("ctrl-shift-w"));
        assert_eq!(keys.0["next_tab"].as_deref(), Some("ctrl-tab"));
        assert_eq!(keys.0["copy"].as_deref(), Some("ctrl-shift-c"));
        assert_eq!(keys.0["activate_tab_9"].as_deref(), Some("alt-9"));
        assert_eq!(keys.0["toggle_fullscreen"].as_deref(), Some("f11"));
        assert_eq!(keys.bindings().len(), 16);
    }

    #[test]
    fn defaults_leave_plain_ctrl_letters_to_the_terminal() {
        // ctrl-<letter> is a control character that shells and vim use (ctrl-w deletes a word,
        // ctrl-t transposes), a binding would swallow it
        for (action, keys) in &Keybindings::default().0 {
            let Some(keys) = keys else { continue };
            for key in keys.split_whitespace() {
                let stroke = Keystroke::parse(key).unwrap();
                let m = stroke.modifiers;
                let plain_ctrl = m.control && !m.shift && !m.alt && !m.platform;
                assert!(
                    !(plain_ctrl && stroke.key.len() == 1),
                    "{action} is bound to {key}, which the terminal needs"
                );
            }
        }
    }

    #[test]
    fn partial_file_merges_and_null_disables() {
        let keys = Keybindings::parse(
            r#"{"new_tab": "ctrl-shift-n", "close_tab": null, "activate_tab_10": "alt-0",}"#,
        )
        .unwrap();
        assert_eq!(keys.0["new_tab"].as_deref(), Some("ctrl-shift-n"));
        assert_eq!(keys.0["close_tab"], None);
        assert_eq!(keys.0["paste"].as_deref(), Some("ctrl-shift-v"));
        assert_eq!(keys.bindings().len(), 16);
    }

    #[test]
    fn invalid_entries_are_rejected() {
        for json in [
            r#"{"open_window": "ctrl-n"}"#,
            r#"{"new_tab": "  "}"#,
            r#"{"new_tab": "foo-t"}"#,
            r#"{"activate_tab_0": "alt-0"}"#,
            r#"{"new_tab": 5}"#,
        ] {
            assert!(
                Keybindings::parse(json).is_err(),
                "{json} should be invalid"
            );
        }
    }

    #[test]
    fn load_creates_keeps_and_falls_back() {
        let dir = temp_dir("keybindings_load");
        with_config_home(&dir, || {
            let path = Keybindings::path().unwrap();
            assert_eq!(path, dir.join("kuterm/keybindings.jsonc"));
            assert_eq!(Keybindings::load(), Keybindings::default());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_KEYBINDINGS);

            std::fs::write(&path, r#"{"paste": null}"#).unwrap();
            assert_eq!(Keybindings::load().0["paste"], None);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                r#"{"paste": null}"#
            );

            std::fs::write(&path, r#"{"bogus": "ctrl-b"}"#).unwrap();
            assert_eq!(Keybindings::load(), Keybindings::default());
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn activate_tab_0_is_rejected() {
        let error = Keybindings::parse(r#"{"activate_tab_0": "alt-0"}"#).unwrap_err();
        assert!(error.to_string().contains("unknown action"), "{error}");
    }

    #[test]
    fn activate_tab_10_adds_a_binding_for_index_9() {
        let keys = Keybindings::parse(r#"{"activate_tab_10": "alt-0"}"#).unwrap();
        let bindings = keys.bindings();
        assert_eq!(bindings.len(), 17);
        let strokes = |ix: usize| -> Vec<Keystroke> {
            let found: Vec<_> = bindings
                .iter()
                .filter(|b| b.action().partial_eq(&ActivateTab(ix)))
                .collect();
            assert_eq!(found.len(), 1, "expected one binding for tab {ix}");
            found[0]
                .keystrokes()
                .iter()
                .map(|k| k.inner().clone())
                .collect()
        };
        assert_eq!(strokes(9), vec![Keystroke::parse("alt-0").unwrap()]);
        // defaults untouched
        assert_eq!(strokes(0), vec![Keystroke::parse("alt-1").unwrap()]);
    }

    #[test]
    fn macos_defaults_bind_the_same_actions() {
        let actions = |file: &str| -> Vec<String> {
            let bindings: BTreeMap<String, Option<String>> = parse_over(file, "{}").unwrap();
            for keys in bindings.values().flatten() {
                for key in keys.split_whitespace() {
                    Keystroke::parse(key).unwrap();
                }
            }
            bindings.into_keys().collect()
        };
        assert_eq!(actions(MACOS_KEYBINDINGS), actions(LINUX_KEYBINDINGS));
    }

    #[test]
    fn every_action_has_a_comment() {
        for action in [
            "new_tab",
            "close_tab",
            "next_tab",
            "copy",
            "paste",
            "command_palette",
            "toggle_fullscreen",
            "activate_tab_1",
        ] {
            let line = DEFAULT_KEYBINDINGS
                .lines()
                .position(|line| line.trim_start().starts_with(&format!("\"{action}\"")))
                .unwrap();
            let previous = DEFAULT_KEYBINDINGS.lines().nth(line - 1).unwrap();
            assert!(
                previous.trim_start().starts_with("//"),
                "{action} has no comment above it"
            );
        }
    }
}
