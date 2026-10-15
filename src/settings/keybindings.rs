//! user keybindings, the "keybindings" section of settings

use std::collections::BTreeMap;

use gpui::{KeyBinding, Keystroke, NoAction};
use serde::Deserialize;
use serde::de::Error;
use serde_json_lenient::Value;

use super::{CommandAction, config};
use crate::ui::{
    terminal_view::{
        Complete, CompletionAccept, CompletionDismiss, CompletionNext, CompletionPrev, Copy, Paste,
        Search, ShowHints, ToggleViMode,
    },
    workspace::{
        ActivateTab, CloseTab, Hide, HideOthers, Minimize, NewTab, NextTab, Quit, RunAction,
        ToggleCommandPalette, ToggleFullscreen,
    },
};

/// action name to its keys, `None` when the action is disabled, and the extra bindings of
/// the "keymap" list
#[derive(Clone, Debug, PartialEq)]
pub struct Keybindings(BTreeMap<String, Option<String>>, Vec<KeymapEntry>);

/// one entry of the "keymap" list
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct KeymapEntry {
    pub keys: String,
    pub action: KeymapAction,
    /// where the keys work, everywhere when none
    #[serde(default)]
    pub when: Option<When>,
}

/// what keys of the "keymap" list do
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum KeymapAction {
    /// an action of the "keybindings" section, or "unbind"
    Named(String),
    /// a command palette action like { "send_text": "..." }
    Command(CommandAction),
}

/// where keys of the "keymap" list work
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum When {
    Terminal,
    Search,
    Palette,
    ViMode,
    AltScreen,
    /// while the completion popup shows
    Completion,
}

impl When {
    /// gpui key context this stands for
    fn context(self) -> &'static str {
        match self {
            When::Terminal => "Terminal",
            When::Search => "Search",
            When::Palette => "CommandPalette",
            When::ViMode => "Terminal && vi_mode",
            When::AltScreen => "Terminal && alt_screen",
            When::Completion => COMPLETION_CONTEXT,
        }
    }
}

impl Default for Keybindings {
    fn default() -> Self {
        Self::parse("{}").expect("bundled default keybindings are invalid")
    }
}

// the popup keys are plain keys the shell needs, so they only work while it shows
const COMPLETION_CONTEXT: &str = "Terminal && completion";

// keys are checked before this runs, so `KeyBinding::new` won't panic
fn binding(action: &str, keys: &str, when: Option<&str>) -> Option<KeyBinding> {
    Some(match action {
        "new_tab" => KeyBinding::new(keys, NewTab, when),
        "close_tab" => KeyBinding::new(keys, CloseTab, when),
        "next_tab" => KeyBinding::new(keys, NextTab, when),
        "copy" => KeyBinding::new(keys, Copy, when.or(Some("Terminal"))),
        "paste" => KeyBinding::new(keys, Paste, when.or(Some("Terminal"))),
        "search" => KeyBinding::new(keys, Search, when.or(Some("Terminal"))),
        "hints" => KeyBinding::new(keys, ShowHints, when.or(Some("Terminal"))),
        "vi_mode" => KeyBinding::new(keys, ToggleViMode, when.or(Some("Terminal"))),
        "complete" => KeyBinding::new(keys, Complete, when.or(Some("Terminal"))),
        "completion_next" => {
            KeyBinding::new(keys, CompletionNext, when.or(Some(COMPLETION_CONTEXT)))
        }
        "completion_prev" => {
            KeyBinding::new(keys, CompletionPrev, when.or(Some(COMPLETION_CONTEXT)))
        }
        "completion_accept" => {
            KeyBinding::new(keys, CompletionAccept, when.or(Some(COMPLETION_CONTEXT)))
        }
        "completion_dismiss" => {
            KeyBinding::new(keys, CompletionDismiss, when.or(Some(COMPLETION_CONTEXT)))
        }
        "command_palette" => KeyBinding::new(keys, ToggleCommandPalette, when),
        "toggle_fullscreen" => KeyBinding::new(keys, ToggleFullscreen, when),
        "quit" => KeyBinding::new(keys, Quit, when),
        "hide" => KeyBinding::new(keys, Hide, when),
        "hide_others" => KeyBinding::new(keys, HideOthers, when),
        "minimize" => KeyBinding::new(keys, Minimize, when),
        "prev_tab" => KeyBinding::new(keys, RunAction(CommandAction::PrevTab), when),
        "select_all" => KeyBinding::new(keys, RunAction(CommandAction::SelectAll), when),
        "clear" => KeyBinding::new(keys, RunAction(CommandAction::Clear), when),
        "increase_font_size" => {
            KeyBinding::new(keys, RunAction(CommandAction::IncreaseFontSize), when)
        }
        "decrease_font_size" => {
            KeyBinding::new(keys, RunAction(CommandAction::DecreaseFontSize), when)
        }
        "reset_font_size" => KeyBinding::new(keys, RunAction(CommandAction::ResetFontSize), when),
        "open_settings" => KeyBinding::new(keys, RunAction(CommandAction::OpenSettings), when),
        "reload_config" => KeyBinding::new(keys, RunAction(CommandAction::ReloadConfig), when),
        "split_left" => KeyBinding::new(keys, RunAction(CommandAction::SplitLeft), when),
        "split_right" => KeyBinding::new(keys, RunAction(CommandAction::SplitRight), when),
        "split_up" => KeyBinding::new(keys, RunAction(CommandAction::SplitUp), when),
        "split_down" => KeyBinding::new(keys, RunAction(CommandAction::SplitDown), when),
        "close_pane" => KeyBinding::new(keys, RunAction(CommandAction::ClosePane), when),
        "focus_pane_left" => KeyBinding::new(keys, RunAction(CommandAction::FocusPaneLeft), when),
        "focus_pane_right" => KeyBinding::new(keys, RunAction(CommandAction::FocusPaneRight), when),
        "focus_pane_up" => KeyBinding::new(keys, RunAction(CommandAction::FocusPaneUp), when),
        "focus_pane_down" => KeyBinding::new(keys, RunAction(CommandAction::FocusPaneDown), when),
        "resize_pane_left" => KeyBinding::new(keys, RunAction(CommandAction::ResizePaneLeft), when),
        "resize_pane_right" => {
            KeyBinding::new(keys, RunAction(CommandAction::ResizePaneRight), when)
        }
        "resize_pane_up" => KeyBinding::new(keys, RunAction(CommandAction::ResizePaneUp), when),
        "resize_pane_down" => KeyBinding::new(keys, RunAction(CommandAction::ResizePaneDown), when),
        "jump_to_prev_prompt" => {
            KeyBinding::new(keys, RunAction(CommandAction::JumpToPrevPrompt), when)
        }
        "jump_to_next_prompt" => {
            KeyBinding::new(keys, RunAction(CommandAction::JumpToNextPrompt), when)
        }
        "select_last_output" => {
            KeyBinding::new(keys, RunAction(CommandAction::SelectLastOutput), when)
        }
        "copy_last_output" => KeyBinding::new(keys, RunAction(CommandAction::CopyLastOutput), when),
        "open_scrollback" => KeyBinding::new(keys, RunAction(CommandAction::OpenScrollback), when),
        "zoom_pane" => KeyBinding::new(keys, RunAction(CommandAction::ZoomPane), when),
        "equalize_panes" => KeyBinding::new(keys, RunAction(CommandAction::EqualizePanes), when),
        "close_other_tabs" => KeyBinding::new(keys, RunAction(CommandAction::CloseOtherTabs), when),
        "rename_tab" => KeyBinding::new(keys, RunAction(CommandAction::RenameTab), when),
        "pin_tab" => KeyBinding::new(keys, RunAction(CommandAction::PinTab), when),
        "duplicate_tab" => KeyBinding::new(keys, RunAction(CommandAction::DuplicateTab), when),
        "next_attention_tab" => {
            KeyBinding::new(keys, RunAction(CommandAction::NextAttentionTab), when)
        }
        "pick_agent_tab" => KeyBinding::new(keys, RunAction(CommandAction::PickAgentTab), when),
        "new_group" => KeyBinding::new(keys, RunAction(CommandAction::NewGroup), when),
        "add_tab_to_group" => KeyBinding::new(keys, RunAction(CommandAction::AddTabToGroup), when),
        "remove_tab_from_group" => {
            KeyBinding::new(keys, RunAction(CommandAction::RemoveTabFromGroup), when)
        }
        "rename_pane" => KeyBinding::new(keys, RunAction(CommandAction::RenamePane), when),
        "show_hidden_text" => KeyBinding::new(keys, RunAction(CommandAction::ShowHiddenText), when),
        "fill_password" => KeyBinding::new(keys, RunAction(CommandAction::FillPassword), when),
        "close_group" => KeyBinding::new(keys, RunAction(CommandAction::CloseGroup), when),
        "tab_overview" => KeyBinding::new(keys, RunAction(CommandAction::TabOverview), when),
        "pick_theme" => KeyBinding::new(keys, RunAction(CommandAction::PickTheme), when),
        "find_in_all_tabs" => KeyBinding::new(keys, RunAction(CommandAction::FindInAllTabs), when),
        "toggle_broadcast_all" => {
            KeyBinding::new(keys, RunAction(CommandAction::ToggleBroadcastAll), when)
        }
        "toggle_broadcast_pane" => {
            KeyBinding::new(keys, RunAction(CommandAction::ToggleBroadcastPane), when)
        }
        "move_tab_to_new_window" => {
            KeyBinding::new(keys, RunAction(CommandAction::MoveTabToNewWindow), when)
        }
        _ => {
            let number: usize = action.strip_prefix("activate_tab_")?.parse().ok()?;
            KeyBinding::new(keys, ActivateTab(number.checked_sub(1)?), when)
        }
    })
}

impl Keybindings {
    /// parse a keybindings section over the bundled one
    pub fn parse(json: &str) -> serde_json_lenient::Result<Self> {
        let section = serde_json_lenient::from_str(json)?;
        Self::from_full(&config::combine(config::section("keybindings", section)))
    }

    /// keybindings section of settings merged over the bundled ones, rejecting unknown
    /// actions and bad keys
    pub fn from_full(full: &Value) -> serde_json_lenient::Result<Self> {
        let section = full.get("keybindings").unwrap_or(&Value::Null);
        let bindings = BTreeMap::<String, Option<String>>::deserialize(section)?;
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
            if binding(action, keys, None).is_none() {
                return Err(Error::custom(format!("unknown action {action:?}")));
            }
        }
        let keymap = match full.get("keymap") {
            None | Some(Value::Null) => Vec::new(),
            Some(list) => Vec::<KeymapEntry>::deserialize(list)
                .map_err(|error| Error::custom(format!("keymap: {error}")))?,
        };
        for entry in &keymap {
            if entry.keys.trim().is_empty() {
                return Err(Error::custom("keymap: empty keys"));
            }
            for key in entry.keys.split_whitespace() {
                Keystroke::parse(key)
                    .map_err(|error| Error::custom(format!("keymap {:?}: {error}", entry.keys)))?;
            }
            if let KeymapAction::Named(name) = &entry.action
                && name != "unbind"
                && binding(name, &entry.keys, None).is_none()
            {
                return Err(Error::custom(format!("keymap: unknown action {name:?}")));
            }
        }
        Ok(Self(bindings, keymap))
    }

    /// gpui bindings for every enabled action, the "keymap" list last so it wins
    pub fn bindings(&self) -> Vec<KeyBinding> {
        let named = self
            .0
            .iter()
            .filter_map(|(action, keys)| binding(action, keys.as_deref()?, None));
        let keymap = self.1.iter().filter_map(|entry| {
            let when = entry.when.map(When::context);
            match &entry.action {
                // the keys then match nothing and reach the terminal
                KeymapAction::Named(name) if name == "unbind" => {
                    Some(KeyBinding::new(&entry.keys, NoAction, when))
                }
                KeymapAction::Named(name) => binding(name, &entry.keys, when),
                KeymapAction::Command(action) => Some(KeyBinding::new(
                    &entry.keys,
                    RunAction(action.clone()),
                    when,
                )),
            }
        });
        named.chain(keymap).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DEFAULT_SETTINGS;

    #[test]
    fn defaults_bind_every_action() {
        let keys = Keybindings::default();
        assert_eq!(keys.0["new_tab"].as_deref(), Some("ctrl-shift-t"));
        assert_eq!(keys.0["close_tab"].as_deref(), Some("ctrl-shift-w"));
        assert_eq!(keys.0["next_tab"].as_deref(), Some("ctrl-tab"));
        assert_eq!(keys.0["copy"].as_deref(), Some("ctrl-shift-c"));
        assert_eq!(keys.0["activate_tab_9"].as_deref(), Some("alt-9"));
        assert_eq!(keys.0["toggle_fullscreen"].as_deref(), Some("f11"));
        assert_eq!(keys.bindings().len(), 30);
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
                let letter = stroke.key.len() == 1 && stroke.key.chars().all(char::is_alphabetic);
                assert!(
                    !(plain_ctrl && letter),
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
        assert_eq!(keys.bindings().len(), 30);
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
    fn activate_tab_0_is_rejected() {
        let error = Keybindings::parse(r#"{"activate_tab_0": "alt-0"}"#).unwrap_err();
        assert!(error.to_string().contains("unknown action"), "{error}");
    }

    #[test]
    fn activate_tab_10_adds_a_binding_for_index_9() {
        let keys = Keybindings::parse(r#"{"activate_tab_10": "alt-0"}"#).unwrap();
        let bindings = keys.bindings();
        assert_eq!(bindings.len(), 31);
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
    fn macos_block_binds_known_actions() {
        let bundled: Value = serde_json_lenient::from_str(DEFAULT_SETTINGS).unwrap();
        let macos = &bundled["os"]["macos"]["keybindings"];
        assert!(macos.as_object().is_some_and(|keys| !keys.is_empty()));
        Keybindings::parse(&macos.to_string()).unwrap();
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
            "quit",
            "hide",
            "hide_others",
            "minimize",
            "prev_tab",
            "select_all",
            "clear",
            "increase_font_size",
            "decrease_font_size",
            "reset_font_size",
            "open_settings",
            "reload_config",
            "complete",
            "completion_next",
            "activate_tab_1",
        ] {
            // other sections have keys like "copy" too
            let section = &DEFAULT_SETTINGS[DEFAULT_SETTINGS.find("\"keybindings\": {").unwrap()..];
            let line = section
                .lines()
                .position(|line| line.trim_start().starts_with(&format!("\"{action}\"")))
                .unwrap();
            let previous = section.lines().nth(line - 1).unwrap();
            assert!(
                previous.trim_start().starts_with("//"),
                "{action} has no comment above it"
            );
        }
    }

    #[test]
    fn keymap_entries_parse_and_bind_after_the_named_ones() {
        let full = config::combine(
            serde_json_lenient::from_str(
                r#"{"keymap": [
                {"keys": "ctrl-a c", "action": "new_tab"},
                {"keys": "ctrl-shift-k", "action": {"send_text": "\u001b[2J"}, "when": "terminal"},
                {"keys": "ctrl-shift-t", "action": "unbind", "when": "alt_screen"},
            ]}"#,
            )
            .unwrap(),
        );
        let keys = Keybindings::from_full(&full).unwrap();
        assert_eq!(
            keys.1[1].action,
            KeymapAction::Command(CommandAction::SendText("\x1b[2J".into()))
        );
        assert_eq!(keys.1[2].when, Some(When::AltScreen));
        let bindings = keys.bindings();
        let named = Keybindings::default().bindings().len();
        assert_eq!(bindings.len(), named + 3);
        // the keymap goes last, so its bindings win
        assert!(bindings[named + 2].action().as_any().is::<NoAction>());
    }

    #[test]
    fn bad_keymap_entries_are_errors() {
        for json in [
            r#"{"keymap": [{"keys": "ctrl-x", "action": "launch_rockets"}]}"#,
            r#"{"keymap": [{"keys": "foo-x", "action": "new_tab"}]}"#,
            r#"{"keymap": [{"keys": " ", "action": "new_tab"}]}"#,
            r#"{"keymap": [{"keys": "ctrl-x", "action": "new_tab", "when": "never"}]}"#,
            r#"{"keymap": [{"keys": "ctrl-x", "action": {"fly": 1}}]}"#,
        ] {
            let full = config::combine(serde_json_lenient::from_str(json).unwrap());
            assert!(Keybindings::from_full(&full).is_err(), "{json}");
        }
    }
}
