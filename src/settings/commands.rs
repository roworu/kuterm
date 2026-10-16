//! command palette commands, the "commands" section of settings

use gpui::{App, Global};
use indexmap::IndexMap;
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use serde_json_lenient::Value;

use super::config;

/// values a command's actions are filled with, by input name
pub type Values = IndexMap<String, String>;

/// `text` with `{name}` placeholders replaced by `values`, shell quoted unless the placeholder
/// is written `{name:raw}`. placeholders `values` knows nothing about stay as they are written,
/// so text like awk's `'{print $1}'` survives
pub fn fill(text: &str, values: impl Fn(&str) -> Option<String>) -> String {
    let mut filled = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            break;
        };
        let name = &after[..end];
        let (name, raw) = match name.strip_suffix(":raw") {
            Some(name) => (name, true),
            None => (name, false),
        };
        let known = !name.is_empty() && !name.contains(['{', ' ', '\t']);
        filled.push_str(&rest[..start]);
        match known.then(|| values(name)).flatten() {
            Some(value) if raw => filled.push_str(&value),
            Some(value) => filled.push_str(&quote(&value)),
            None => filled.push_str(&rest[start..start + end + 2]),
        }
        rest = &after[end + 1..];
    }
    filled.push_str(rest);
    filled
}

/// `value` quoted for a shell, left as it is when it holds a character shlex can not quote
fn quote(value: &str) -> String {
    shlex::try_quote(value).map_or_else(|_| value.to_string(), |quoted| quoted.into_owned())
}

/// one value a command asks for before its actions run, picked in the palette
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Input {
    /// free text, this hint shows in the field while it is empty
    #[serde(default)]
    pub text: Option<String>,
    /// run this in the pane's folder and pick one of its output lines
    #[serde(default)]
    pub pick_from: Option<String>,
    /// pick one of these
    #[serde(default)]
    pub pick: Option<Vec<String>>,
    /// run the whole command once per picked value, each in a split of its own
    #[serde(default)]
    pub matrix: bool,
}

impl Input {
    /// the one source of values it was set up with
    fn check(&self, name: &str) -> Result<(), String> {
        let sources = [
            self.text.is_some(),
            self.pick_from.is_some(),
            self.pick.is_some(),
        ];
        if sources.iter().filter(|set| **set).count() != 1 {
            return Err(format!(
                "input {name:?} needs one of \"text\", \"pick_from\" or \"pick\""
            ));
        }
        if self.matrix && self.pick.is_none() {
            return Err(format!(
                "input {name:?} can only be a matrix with \"pick\" values"
            ));
        }
        Ok(())
    }
}

/// where a step acts, written as "new_split_right", "pane:tests", "all_panes_in_tab" or
/// "new_window"
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// split the focused pane and act in the new tab
    NewSplit(Side),
    /// the pane named, titled or numbered like this
    Pane(String),
    /// every pane of the window, like broadcast input
    AllPanes,
    /// a window of its own
    NewWindow,
}

/// side a "new_split_*" target splits
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

impl Target {
    /// the target this text names
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "new_split_left" => Some(Self::NewSplit(Side::Left)),
            "new_split_right" => Some(Self::NewSplit(Side::Right)),
            "new_split_up" => Some(Self::NewSplit(Side::Up)),
            "new_split_down" => Some(Self::NewSplit(Side::Down)),
            "all_panes_in_tab" => Some(Self::AllPanes),
            "new_window" => Some(Self::NewWindow),
            _ => text
                .strip_prefix("pane:")
                .filter(|name| !name.is_empty())
                .map(|name| Self::Pane(name.to_string())),
        }
    }
}

/// one step of a command: an action and, when it names one, the target it acts on
#[derive(Clone, Debug, PartialEq)]
pub struct CommandStep {
    /// where the action acts, the command's current target when none
    pub target: Option<Target>,
    pub action: CommandAction,
}

impl CommandStep {
    /// a step acting on the command's current target
    pub fn new(action: CommandAction) -> Self {
        Self {
            target: None,
            action,
        }
    }
}

impl From<CommandAction> for CommandStep {
    fn from(action: CommandAction) -> Self {
        Self::new(action)
    }
}

// a step is either an action of its own or { "target": "...", "action": ... }
impl<'de> Deserialize<'de> for CommandStep {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let text = |value: Option<&Value>| -> Result<String, D::Error> {
            value
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| Error::custom("a target is text like \"pane:tests\""))
        };
        let target = match value.get("target") {
            None | Some(Value::Null) => None,
            Some(target) => {
                let target = text(Some(target))?;
                Some(Target::parse(&target).ok_or_else(|| {
                    Error::custom(format!(
                        "unknown target {target:?}, use \"new_split_right\", \"pane:<name>\", \
                         \"all_panes_in_tab\" or \"new_window\""
                    ))
                })?)
            }
        };
        let action = if target.is_some() {
            CommandAction::deserialize(value.get("action").ok_or_else(|| {
                Error::custom("a step with a target needs an \"action\" next to it")
            })?)
            .map_err(|error| Error::custom(format!("action: {error}")))?
        } else {
            CommandAction::deserialize(&value).map_err(Error::custom)?
        };
        Ok(Self { target, action })
    }
}

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
    /// open the default profile incognito
    NewIncognitoTab,
    /// copy the active tab's folder to the clipboard
    CopyFolder,
    /// move the active tab with its running programs into a window of its own
    MoveTabToNewWindow,
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
    /// switch to the tab whose ai agent has waited longest for the user
    NextAttentionTab,
    /// list the tabs running an ai agent in the palette with how long each waits
    PickAgentTab,
    /// search the history of every tab of every window, picking a match goes there
    FindInAllTabs,
    /// list the themes, showing each one while moving through them, enter keeps one
    PickTheme,
    /// a grid of previews of every tab, picking one switches to it
    TabOverview,
    /// a map of the panes whose programs talk to each other
    ShowTopology,
    /// put the active tab into a new group
    NewGroup,
    /// put the active tab into the group of the tab next to it
    AddTabToGroup,
    /// take the active tab out of its group
    RemoveTabFromGroup,
    /// close every tab of the active tab's group
    CloseGroup,
    /// list the password manager's entries, type the picked one's password into the tab
    FillPassword,
    /// draw text programs concealed, or hide it again
    ShowHiddenText,
    /// type the name `kuterm msg --pane` finds the active tab by
    RenamePane,
    /// type into every pane of the window at once, or stop
    ToggleBroadcastAll,
    /// join the focused pane to the panes typed into together, or take it out
    ToggleBroadcastPane,
    /// scroll the focused pane together with the other joined panes, or take it out
    ToggleScrollSync,
    /// ask for a command typed into the active tab once its prompt is back
    QueueCommand,
    /// list the active tab's queued commands to edit, reorder or drop them
    EditQueue,
    /// go on with the queue after a failed command paused it
    ResumeQueue,
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
    /// a palette command a plugin added
    Plugin {
        plugin: String,
        command: String,
    },
}

impl CommandAction {
    /// the text this action gets `{name}` placeholders substituted in, none for the others
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Type(text) | Self::SendText(text) => Some(text),
            Self::Notify(text) | Self::NotifyWhenDone(text) => Some(text),
            Self::NewTabWithProfile(name) | Self::NewBackgroundTabWithProfile(name) => Some(name),
            _ => None,
        }
    }

    /// the same action with `text` in place of what `text` returns
    pub fn with_text(&self, text: String) -> Self {
        match self {
            Self::Type(_) => Self::Type(text),
            Self::SendText(_) => Self::SendText(text),
            Self::Notify(_) => Self::Notify(text),
            Self::NotifyWhenDone(_) => Self::NotifyWhenDone(text),
            Self::NewTabWithProfile(_) => Self::NewTabWithProfile(text),
            Self::NewBackgroundTabWithProfile(_) => Self::NewBackgroundTabWithProfile(text),
            _ => unreachable!("only actions with text are rebuilt"),
        }
    }
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
    /// values asked for before the actions run, kept in the order they are written since the
    /// palette asks for them in that order
    #[serde(default)]
    pub inputs: Inputs,
    pub actions: Vec<CommandStep>,
}

/// input name to what to ask for
pub type Inputs = IndexMap<String, Input>;

impl Command {
    /// "category: name" shown in the palette, just the name when uncategorized
    pub fn label(&self) -> String {
        match &self.category {
            Some(category) => format!("{category}: {}", self.name),
            None => self.name.clone(),
        }
    }

    /// true when an action asks for `{hint}`, which needs a hint picked before the run
    pub fn uses_hint(&self) -> bool {
        self.actions.iter().any(|step| {
            step.action
                .text()
                .is_some_and(|text| text.contains("{hint}") || text.contains("{hint:raw}"))
        })
    }

    /// the input of a matrix, the one running the command once per value
    pub fn matrix(&self) -> Option<(&str, &[String])> {
        self.inputs
            .iter()
            .find(|(_, input)| input.matrix)
            .and_then(|(name, input)| Some((name.as_str(), input.pick.as_deref()?)))
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
        let commands = Self::deserialize(full)?;
        for command in &commands.commands {
            for (name, input) in &command.inputs {
                input.check(name).map_err(|error| {
                    Error::custom(format!("command {:?}: {error}", command.name))
                })?;
                if BUILT_INS.iter().any(|built_in| built_in == name) {
                    return Err(Error::custom(format!(
                        "command {:?}: {name:?} is a built in value, pick another name",
                        command.name
                    )));
                }
            }
            if command.inputs.values().filter(|input| input.matrix).count() > 1 {
                return Err(Error::custom(format!(
                    "command {:?}: only one input can be a matrix",
                    command.name
                )));
            }
        }
        Ok(commands)
    }

    /// commands loaded at startup or on the last reload
    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// the command `name` labels, by label or plain name, ignoring case
    pub fn find(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|command| {
            command.label().eq_ignore_ascii_case(name) || command.name.eq_ignore_ascii_case(name)
        })
    }
}

/// values a command's actions can use without an input of their own
const BUILT_INS: [&str; 5] = ["selection", "last_output", "cwd", "clipboard", "hint"];

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
                "pin_tab", "duplicate_tab", "copy_folder", "move_tab_to_new_window",
                "split_left", "split_right", "split_up", "split_down",
                "next_tab", "prev_tab", {"activate_tab": 2}, "pick_tab", "copy", "paste",
                {"scroll_up": 10}, {"scroll_down": 3}, "scroll_top", "scroll_bottom",
                "quit", "select_all", "clear", "increase_font_size", "decrease_font_size",
                "reset_font_size", "open_settings",
                {"type": "ls\n"}, {"notify": "hi"}, {"notify_when_done": "done"},
            ]}]}"#,
        )
        .unwrap();
        assert_eq!(commands.commands[0].name, "all");
        assert_eq!(
            commands.commands[0].actions,
            vec![
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
                CommandAction::MoveTabToNewWindow,
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
            ]
            .into_iter()
            .map(CommandStep::from)
            .collect::<Vec<_>>(),
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
            .into_iter()
            .map(CommandStep::from)
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn inputs_keep_their_order_and_are_checked() {
        let commands = Commands::parse(
            r#"{"commands": [{"name": "checkout", "inputs": {
                "branch": {"pick_from": "git branch --format='%(refname:short)'"},
                "remote": {"pick": ["origin", "upstream"]},
                "message": {"text": "commit message"},
            }, "actions": [{"type": "git checkout {branch}\n"}]}]}"#,
        )
        .unwrap();
        let inputs: Vec<&str> = commands.commands[0]
            .inputs
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(inputs, ["branch", "remote", "message"]);
        assert_eq!(
            commands.commands[0].inputs["remote"].pick.as_deref(),
            Some(["origin".to_string(), "upstream".to_string()].as_slice())
        );
        assert!(commands.commands[0].matrix().is_none());

        for json in [
            // no source, two sources, and a matrix without values
            r#"{"commands": [{"name": "x", "inputs": {"a": {}}, "actions": ["about"]}]}"#,
            r#"{"commands": [{"name": "x", "inputs": {"a": {"text": "t", "pick": ["v"]}}, "actions": ["about"]}]}"#,
            r#"{"commands": [{"name": "x", "inputs": {"a": {"text": "t", "matrix": true}}, "actions": ["about"]}]}"#,
            // a built in value can not be an input
            r#"{"commands": [{"name": "x", "inputs": {"cwd": {"text": "t"}}, "actions": ["about"]}]}"#,
            // two matrices
            r#"{"commands": [{"name": "x", "inputs": {"a": {"pick": ["1"], "matrix": true}, "b": {"pick": ["2"], "matrix": true}}, "actions": ["about"]}]}"#,
        ] {
            assert!(Commands::parse(json).is_err(), "{json} should be invalid");
        }
    }

    #[test]
    fn matrix_inputs_are_found() {
        let commands = Commands::parse(
            r#"{"commands": [{"name": "test", "inputs": {
                "python": {"pick": ["3.11", "3.12", "3.13"], "matrix": true},
            }, "actions": [{"type": "pytest -$(python {python})\n"}]}]}"#,
        )
        .unwrap();
        let (name, values) = commands.commands[0].matrix().unwrap();
        assert_eq!(name, "python");
        assert_eq!(values, ["3.11", "3.12", "3.13"]);
    }

    #[test]
    fn steps_take_a_target_of_their_own() {
        let commands = Commands::parse(
            r#"{"commands": [{"name": "x", "actions": [
                "new_tab",
                {"target": "pane:tests", "action": {"type": "cargo test\n"}},
                {"target": "all_panes_in_tab", "action": {"type": "uptime\n"}},
                {"target": "new_split_down", "action": {"type": "clear\n"}},
                {"target": "new_window", "action": "about"},
            ]}]}"#,
        )
        .unwrap();
        let steps = &commands.commands[0].actions;
        assert_eq!(steps[0].target, None);
        assert_eq!(steps[1].target, Some(Target::Pane("tests".into())));
        assert_eq!(steps[1].action, CommandAction::Type("cargo test\n".into()));
        assert_eq!(steps[2].target, Some(Target::AllPanes));
        assert_eq!(steps[3].target, Some(Target::NewSplit(Side::Down)));
        assert_eq!(steps[4].target, Some(Target::NewWindow));

        for json in [
            r#"{"commands": [{"name": "x", "actions": [{"target": "sideways", "action": "about"}]}]}"#,
            r#"{"commands": [{"name": "x", "actions": [{"target": "pane:", "action": "about"}]}]}"#,
            r#"{"commands": [{"name": "x", "actions": [{"target": "pane:t"}]}]}"#,
            r#"{"commands": [{"name": "x", "actions": [{"target": 5, "action": "about"}]}]}"#,
        ] {
            assert!(Commands::parse(json).is_err(), "{json} should be invalid");
        }
    }

    #[test]
    fn placeholders_are_quoted_unless_written_raw() {
        let values = |name: &str| match name {
            "branch" => Some("my branch; rm -rf /".to_string()),
            "file" => Some("a b.txt".to_string()),
            _ => None,
        };
        assert_eq!(
            fill("git checkout {branch}\n", values),
            "git checkout 'my branch; rm -rf /'\n"
        );
        assert_eq!(
            fill("echo {branch:raw}\n", values),
            "echo my branch; rm -rf /\n"
        );
        // a plain value is not quoted at all
        assert_eq!(fill("cat {file}\n", values), "cat 'a b.txt'\n");
        // unknown placeholders and text that only looks like one stay as written
        assert_eq!(
            fill("awk '{print $1}' {nope}", values),
            "awk '{print $1}' {nope}"
        );
        assert_eq!(fill("no placeholders", values), "no placeholders");
        assert_eq!(fill("half {open", values), "half {open");
        assert_eq!(fill("{} { }", values), "{} { }");
    }

    #[test]
    fn hint_commands_are_told_apart_and_their_text_replaced() {
        let commands = Commands::parse(
            r#"{"commands": [
                {"name": "open in browser", "actions": [{"type": "xdg-open {hint:raw}\n"}]},
                {"name": "plain", "actions": [{"type": "ls\n"}, "about"]},
            ]}"#,
        )
        .unwrap();
        assert!(commands.commands[0].uses_hint());
        assert!(!commands.commands[1].uses_hint());
        assert_eq!(
            commands.commands[0].actions[0].action.text(),
            Some("xdg-open {hint:raw}\n")
        );
        assert_eq!(
            commands.commands[0].actions[0]
                .action
                .clone()
                .with_text("picked".into()),
            CommandAction::Type("picked".into())
        );
        // actions without text are left as they are
        assert_eq!(commands.commands[1].actions[1].action.text(), None);
    }

    #[test]
    fn commands_are_found_by_label_or_name() {
        let commands = Commands::parse(
            r#"{"commands": [{"name": "checkout branch", "category": "git", "actions": ["about"]}]}"#,
        )
        .unwrap();
        assert!(commands.find("git: checkout branch").is_some());
        assert!(commands.find("CHECKOUT BRANCH").is_some());
        assert!(commands.find("checkout").is_none());
    }
}
