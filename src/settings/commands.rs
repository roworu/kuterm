//! command palette commands, the "commands" section of settings

use gpui::{App, Global};
use serde::Deserialize;
use serde_json_lenient::Value;

use super::config;

/// one step of a command
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandAction {
    About,
    /// read settings, the files they import and theme files again
    ReloadConfig,
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
    /// close every tab but the active one, pinned tabs stay
    CloseOtherTabs,
    /// give the active tab a title of its own, typed into the tab
    RenameTab,
    /// keep the active tab small at the front of its pane, or let it go again
    PinTab,
    /// open a tab with the active tab's profile and folder right next to it
    DuplicateTab,
    /// copy the active tab's folder to the clipboard
    CopyFolder,
    /// split the focused pane and move its active tab into a new pane on the left
    SplitLeft,
    /// split the focused pane and move its active tab into a new pane on the right
    SplitRight,
    /// split the focused pane and move its active tab into a new pane above
    SplitUp,
    /// split the focused pane and move its active tab into a new pane below
    SplitDown,
    /// close the focused pane with all its tabs
    ClosePane,
    FocusPaneLeft,
    FocusPaneRight,
    FocusPaneUp,
    FocusPaneDown,
    /// move the divider next to the focused pane that way
    ResizePaneLeft,
    ResizePaneRight,
    ResizePaneUp,
    ResizePaneDown,
    /// show the focused pane alone, or all panes again
    ZoomPane,
    /// give all panes the same size
    EqualizePanes,
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
    /// open the search bar of the active tab
    Search,
    /// label the matches of the hint patterns on screen
    Hints,
    /// start or end keyboard selection mode
    ViMode,
    /// scroll to the prompt above the view, from shell integration marks
    JumpToPrevPrompt,
    /// scroll to the prompt below the top of the view
    JumpToNextPrompt,
    /// select the output of the last command
    SelectLastOutput,
    /// copy the output of the last command
    CopyLastOutput,
    /// open the history in `scrollback_pager` in a new tab
    OpenScrollback,
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
    /// text sent to the program in the active tab right away, like escape sequences
    SendText(String),
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
    /// parse settings with a "commands" section, a user list replaces the bundled one whole
    pub fn parse(json: &str) -> serde_json_lenient::Result<Self> {
        Self::from_full(&config::combine(serde_json_lenient::from_str(json)?))
    }

    /// commands from settings already merged over the bundled ones
    pub fn from_full(full: &Value) -> serde_json_lenient::Result<Self> {
        Self::deserialize(full)
    }

    /// commands loaded at startup or on the last reload
    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DEFAULT_SETTINGS;

    #[test]
    fn bundled_commands_parse() {
        assert_eq!(
            Commands::parse(DEFAULT_SETTINGS).unwrap(),
            Commands::default()
        );
        assert!(!Commands::default().commands.is_empty());
    }

    #[test]
    fn parses_every_action() {
        let commands = Commands::parse(
            r#"{"commands": [{"name": "all", "actions": [
                "about", "reload_config", "new_tab", {"new_tab_with_profile": "dev"},
                "new_background_tab", {"new_background_tab_with_profile": "dev"}, "close_tab",
                "close_tabs_to_right", "close_tabs_to_left", "close_other_tabs", "rename_tab",
                "pin_tab", "duplicate_tab", "copy_folder",
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
                    CommandAction::ReloadConfig,
                    CommandAction::NewTab,
                    CommandAction::NewTabWithProfile("dev".into()),
                    CommandAction::NewBackgroundTab,
                    CommandAction::NewBackgroundTabWithProfile("dev".into()),
                    CommandAction::CloseTab,
                    CommandAction::CloseTabsToRight,
                    CommandAction::CloseTabsToLeft,
                    CommandAction::CloseOtherTabs,
                    CommandAction::RenameTab,
                    CommandAction::PinTab,
                    CommandAction::DuplicateTab,
                    CommandAction::CopyFolder,
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
    fn new_actions_parse() {
        let commands = Commands::parse(
            r#"{"commands": [{"name": "all", "actions": [
                "search", "hints", "vi_mode", "close_pane", "focus_pane_left", "focus_pane_right",
                "focus_pane_up", "focus_pane_down", "resize_pane_left", "resize_pane_right",
                "resize_pane_up", "resize_pane_down", "zoom_pane", "equalize_panes",
                "jump_to_prev_prompt", "jump_to_next_prompt", "select_last_output",
                "copy_last_output", "open_scrollback", {"send_text": "x"},
            ]}]}"#,
        )
        .unwrap();
        assert_eq!(
            commands.commands[0].actions,
            vec![
                CommandAction::Search,
                CommandAction::Hints,
                CommandAction::ViMode,
                CommandAction::ClosePane,
                CommandAction::FocusPaneLeft,
                CommandAction::FocusPaneRight,
                CommandAction::FocusPaneUp,
                CommandAction::FocusPaneDown,
                CommandAction::ResizePaneLeft,
                CommandAction::ResizePaneRight,
                CommandAction::ResizePaneUp,
                CommandAction::ResizePaneDown,
                CommandAction::ZoomPane,
                CommandAction::EqualizePanes,
                CommandAction::JumpToPrevPrompt,
                CommandAction::JumpToNextPrompt,
                CommandAction::SelectLastOutput,
                CommandAction::CopyLastOutput,
                CommandAction::OpenScrollback,
                CommandAction::SendText("x".into()),
            ]
        );
    }
}
