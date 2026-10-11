//! command palette commands, read from a jsonc file next to settings

use std::path::PathBuf;

use gpui::{App, Global};
use serde::Deserialize;

use super::{config_dir, load_file, parse_over};

/// commented commands file written on first launch
pub const DEFAULT_COMMANDS: &str = include_str!("../../assets/default_commands.jsonc");

/// one step of a command
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandAction {
    About,
    ReloadSettings,
    ReloadThemes,
    ReloadKeybindings,
    ReloadAll,
    NewTab,
    /// open a tab with the profile of this name
    NewTabWithProfile(String),
    /// open a tab without switching to it, later actions of the command act on it
    NewBackgroundTab,
    /// open a tab with the profile of this name without switching to it
    NewBackgroundTabWithProfile(String),
    CloseTab,
    /// close every tab to the right of the active one
    CloseTabsToRight,
    /// close every tab to the left of the active one
    CloseTabsToLeft,
    /// split the focused pane and move its active tab into a new pane on the left
    SplitLeft,
    /// split the focused pane and move its active tab into a new pane on the right
    SplitRight,
    /// split the focused pane and move its active tab into a new pane above
    SplitUp,
    /// split the focused pane and move its active tab into a new pane below
    SplitDown,
    NextTab,
    PrevTab,
    /// switch to the tab at this 1 based position
    ActivateTab(usize),
    /// list the open tabs in the palette, picking one switches to it
    PickTab,
    /// copy the active tab's selection into the clipboard
    Copy,
    /// paste the clipboard into the active tab
    Paste,
    /// scroll the active tab up into history by these many lines
    ScrollUp(i32),
    /// scroll the active tab down by these many lines
    ScrollDown(i32),
    /// scroll the active tab to the top of its history
    ScrollTop,
    /// scroll the active tab back to the prompt
    ScrollBottom,
    /// close every tab and exit
    Quit,
    /// select the active tab's whole history and screen
    SelectAll,
    /// drop the active tab's history and move its prompt to the top
    Clear,
    /// make the terminal font one pixel bigger
    IncreaseFontSize,
    /// make the terminal font one pixel smaller
    DecreaseFontSize,
    /// set the terminal font back to its size from settings
    ResetFontSize,
    /// open settings.jsonc in the system text editor
    OpenSettings,
    /// text written to the active tab as if typed
    Type(String),
    /// show a notification with this text
    Notify(String),
    /// show a notification with this text once the program running in the tab finishes
    NotifyWhenDone(String),
}

/// palette entry, its actions run in order
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Command {
    pub name: String,
    /// group shown before the name and used to sort the palette, like "tabs" or "config"
    #[serde(default)]
    pub category: Option<String>,
    /// keep this command at the very top of the palette
    #[serde(default)]
    pub pinned: bool,
    pub actions: Vec<CommandAction>,
}

impl Command {
    /// "category: name" shown in the palette, just the name when uncategorized
    pub fn label(&self) -> String {
        match &self.category {
            Some(category) => format!("{category}: {}", self.name),
            None => self.name.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Commands {
    pub commands: Vec<Command>,
}

impl Default for Commands {
    fn default() -> Self {
        Self::parse("{}").expect("bundled commands are invalid")
    }
}

impl Global for Commands {}

impl Commands {
    /// `commands.jsonc` in the config dir
    pub fn path() -> Option<PathBuf> {
        Some(config_dir()?.join("commands.jsonc"))
    }

    /// parse commands over the bundled ones, a user list replaces the bundled one whole
    pub fn parse(json: &str) -> serde_json_lenient::Result<Self> {
        parse_over(DEFAULT_COMMANDS, json)
    }

    /// load commands from their file, using defaults when it is missing or invalid
    pub fn load() -> Self {
        load_file(Self::path(), DEFAULT_COMMANDS, "commands", Self::parse)
    }

    /// commands loaded at startup or on the last reload
    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::tests::{temp_dir, with_config_home};

    #[test]
    fn bundled_commands_are_commented_and_parse() {
        assert!(DEFAULT_COMMANDS.contains("//"));
        assert_eq!(
            Commands::parse(DEFAULT_COMMANDS).unwrap(),
            Commands::default()
        );
        assert!(!Commands::default().commands.is_empty());
    }

    #[test]
    fn parses_every_action() {
        let commands = Commands::parse(
            r#"{"commands": [{"name": "all", "actions": [
                "about", "reload_settings", "reload_themes", "reload_keybindings",
                "reload_all", "new_tab", {"new_tab_with_profile": "dev"},
                "new_background_tab", {"new_background_tab_with_profile": "dev"}, "close_tab",
                "close_tabs_to_right", "close_tabs_to_left",
                "split_left", "split_right", "split_up", "split_down",
                "next_tab", "prev_tab", {"activate_tab": 2}, "pick_tab", "copy", "paste",
                {"scroll_up": 10}, {"scroll_down": 3}, "scroll_top", "scroll_bottom",
                "quit", "select_all", "clear", "increase_font_size", "decrease_font_size",
                "reset_font_size", "open_settings",
                {"type": "ls\n"}, {"notify": "hi"}, {"notify_when_done": "done"},
            ]}]}"#,
        )
        .unwrap();
        assert_eq!(
            commands.commands,
            vec![Command {
                name: "all".into(),
                category: None,
                pinned: false,
                actions: vec![
                    CommandAction::About,
                    CommandAction::ReloadSettings,
                    CommandAction::ReloadThemes,
                    CommandAction::ReloadKeybindings,
                    CommandAction::ReloadAll,
                    CommandAction::NewTab,
                    CommandAction::NewTabWithProfile("dev".into()),
                    CommandAction::NewBackgroundTab,
                    CommandAction::NewBackgroundTabWithProfile("dev".into()),
                    CommandAction::CloseTab,
                    CommandAction::CloseTabsToRight,
                    CommandAction::CloseTabsToLeft,
                    CommandAction::SplitLeft,
                    CommandAction::SplitRight,
                    CommandAction::SplitUp,
                    CommandAction::SplitDown,
                    CommandAction::NextTab,
                    CommandAction::PrevTab,
                    CommandAction::ActivateTab(2),
                    CommandAction::PickTab,
                    CommandAction::Copy,
                    CommandAction::Paste,
                    CommandAction::ScrollUp(10),
                    CommandAction::ScrollDown(3),
                    CommandAction::ScrollTop,
                    CommandAction::ScrollBottom,
                    CommandAction::Quit,
                    CommandAction::SelectAll,
                    CommandAction::Clear,
                    CommandAction::IncreaseFontSize,
                    CommandAction::DecreaseFontSize,
                    CommandAction::ResetFontSize,
                    CommandAction::OpenSettings,
                    CommandAction::Type("ls\n".into()),
                    CommandAction::Notify("hi".into()),
                    CommandAction::NotifyWhenDone("done".into()),
                ],
            }]
        );
    }

    #[test]
    fn category_is_optional_and_labels_the_command() {
        let commands = Commands::parse(
            r#"{"commands": [
                {"name": "create new", "category": "tabs", "actions": ["new_tab"]},
                {"name": "about", "actions": ["about"]},
            ]}"#,
        )
        .unwrap();
        assert_eq!(commands.commands[0].category.as_deref(), Some("tabs"));
        assert_eq!(commands.commands[0].label(), "tabs: create new");
        assert_eq!(commands.commands[1].category, None);
        assert_eq!(commands.commands[1].label(), "about");
    }

    #[test]
    fn pinned_defaults_to_false_and_parses() {
        let commands = Commands::parse(
            r#"{"commands": [
                {"name": "plain", "actions": ["about"]},
                {"name": "sticky", "pinned": true, "actions": ["about"]},
            ]}"#,
        )
        .unwrap();
        assert!(!commands.commands[0].pinned);
        assert!(commands.commands[1].pinned);
    }

    #[test]
    fn invalid_commands_are_rejected() {
        for json in [
            r#"{"commands": [{"name": "x", "actions": ["launch_rockets"]}]}"#,
            r#"{"commands": [{"actions": ["about"]}]}"#,
            r#"{"commands": [{"name": "x"}]}"#,
            r#"{"commands": [{"name": "x", "actions": [{"type": 5}]}]}"#,
            r#"{"commands": [{"name": "x", "actions": [{"new_tab_with_profile": 5}]}]}"#,
            r#"{"commands": [{"name": "x", "actions": [{"activate_tab": "two"}]}]}"#,
            r#"{"commands": [{"name": "x", "actions": [{"activate_tab": -1}]}]}"#,
            r#"{"commands": [{"name": "x", "actions": [{"scroll_up": "ten"}]}]}"#,
            r#"{"commands": [{"name": "x", "actions": ["notify"]}]}"#,
            r#"{"commands": [{"name": "x", "actions": [{"notify_when_done": 1}]}]}"#,
            r#"{"commands": "about"}"#,
        ] {
            assert!(Commands::parse(json).is_err(), "{json} should be invalid");
        }
    }

    #[test]
    fn load_creates_keeps_and_falls_back() {
        let dir = temp_dir("commands_load");
        with_config_home(&dir, || {
            let path = Commands::path().unwrap();
            assert_eq!(path, dir.join("kuterm/commands.jsonc"));
            assert_eq!(Commands::load(), Commands::default());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_COMMANDS);

            let json = r#"{"commands": [{"name": "only", "actions": ["new_tab"]}]}"#;
            std::fs::write(&path, json).unwrap();
            assert_eq!(Commands::load().commands.len(), 1);

            std::fs::write(&path, "{ broken").unwrap();
            assert_eq!(Commands::load(), Commands::default());
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
