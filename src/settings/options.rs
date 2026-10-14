//! value types for settings that have a fixed set of choices

use alacritty_terminal::vte::ansi::CursorShape as AlacCursorShape;
use serde::{Deserialize, Serialize};

/// shell to launch
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    /// the user's login shell from /etc/passwd
    System,
    Program(String),
    WithArguments {
        program: String,
        args: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LineHeight {
    Comfortable,
    Standard,
    Custom(f32),
}

impl LineHeight {
    /// line height as a multiple of the font size
    pub fn value(&self) -> f32 {
        match self {
            LineHeight::Comfortable => 1.618,
            LineHeight::Standard => 1.3,
            LineHeight::Custom(value) => *value,
        }
    }
}

/// which theme to use
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    /// follow system dark/light preference
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
    Hollow,
}

impl From<CursorShape> for AlacCursorShape {
    fn from(shape: CursorShape) -> Self {
        match shape {
            CursorShape::Block => AlacCursorShape::Block,
            CursorShape::Bar => AlacCursorShape::Beam,
            CursorShape::Underline => AlacCursorShape::Underline,
            CursorShape::Hollow => AlacCursorShape::HollowBlock,
        }
    }
}

/// how the cursor looks while the terminal is not focused
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CursorUnfocused {
    Hollow,
    /// same shape as when focused
    Same,
    None,
}

/// slant of italic text
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FontSlant {
    Normal,
    Italic,
    Oblique,
}

impl From<FontSlant> for gpui::FontStyle {
    fn from(slant: FontSlant) -> Self {
        match slant {
            FontSlant::Normal => gpui::FontStyle::Normal,
            FontSlant::Italic => gpui::FontStyle::Italic,
            FontSlant::Oblique => gpui::FontStyle::Oblique,
        }
    }
}

/// when the cursor blinks
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CursorBlink {
    On,
    Off,
    /// only while the running program asks for it
    App,
}

impl CursorBlink {
    /// true when the cursor should blink, `app_asks` is what the program set
    pub fn active(&self, app_asks: bool) -> bool {
        match self {
            CursorBlink::On => true,
            CursorBlink::Off => false,
            CursorBlink::App => app_asks,
        }
    }
}

/// one piece of the tab title
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TabTitleBlock {
    /// tab position, starting at 1
    Number,
    /// user@host
    Prompt,
    Folder,
    /// name of the running program
    Command,
    /// title set by the running program
    Title,
    Text(String),
    /// first line of a shell command's output, run in the current folder
    Exec(String),
    /// icon from "icons"
    Icon,
    /// circle filling up with the progress the running program reports, spinning while it
    /// reports no percent
    Progress,
    /// tcp ports the tab's programs listen on
    Ports,
    /// cpu use of the tab's programs, 100% being one busy core
    Cpu,
    /// memory the tab's programs use
    Memory,
    /// cpu use of every tab added up
    TotalCpu,
    /// memory of every tab added up
    TotalMemory,
    /// ports of every tab
    AllPorts,
    /// tcp bytes the tab's programs receive and send per second
    Network,
    /// tcp traffic of every tab added up
    TotalNetwork,
}

impl TabTitleBlock {
    /// true for blocks that need the process sampler running
    pub fn sampled(&self) -> bool {
        matches!(
            self,
            Self::Ports
                | Self::Cpu
                | Self::Memory
                | Self::TotalCpu
                | Self::TotalMemory
                | Self::AllPorts
                | Self::Network
                | Self::TotalNetwork
        )
    }
}

/// unit memory blocks are shown in
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryUnit {
    /// the largest unit that keeps the number at 1 or more
    #[default]
    Auto,
    Kb,
    Mb,
    Gb,
}

impl MemoryUnit {
    /// the unit a click switches to
    pub fn next(self) -> Self {
        match self {
            Self::Auto => Self::Kb,
            Self::Kb => Self::Mb,
            Self::Mb => Self::Gb,
            Self::Gb => Self::Auto,
        }
    }
}

/// where the title sits inside its tab
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TabTitleAlign {
    Left,
    Center,
    Right,
}

/// what the visual bell flashes
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BellTarget {
    Tab,
    Terminal,
}

/// where the new tab button sits in the tab bar
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NewTabButton {
    Left,
    Right,
    AfterTabs,
}

/// when the scrollbar is shown
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ScrollbarEnable {
    On,
    Off,
    /// only while there is history to scroll
    Dynamic,
}

/// which side of the terminal the scrollbar sits on
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ScrollbarPlacement {
    Left,
    Right,
}

/// who draws the window frame
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Decorations {
    /// title bar and borders drawn by the desktop, like kde, gnome or xfwm
    // "full" was its first name, configs written then still say it
    #[serde(alias = "full")]
    Server,
    /// no frame, the tab bar moves the window
    None,
    /// no frame, window buttons sit in the top bar
    Client,
}

/// when the window draws its own menu bar
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MenuBarMode {
    /// shown on windows, hidden on linux
    Auto,
    Always,
    Never,
}

impl MenuBarMode {
    /// true when the window shows the menu bar, never on macos, which has the system one
    pub fn shown(self) -> bool {
        match self {
            _ if cfg!(target_os = "macos") => false,
            MenuBarMode::Auto => cfg!(target_os = "windows"),
            MenuBarMode::Always => true,
            MenuBarMode::Never => false,
        }
    }
}

/// how the window opens
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StartupMode {
    Windowed,
    Maximized,
    Fullscreen,
}

/// when quitting asks first
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmQuit {
    Always,
    /// only while a program still runs in some tab
    Running,
    Never,
}

/// speed curve of a smooth scroll
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ScrollEasing {
    Linear,
    EaseOut,
    EaseInOut,
}

impl ScrollEasing {
    /// share of the distance covered at `t`, both from 0 to 1
    pub fn apply(&self, t: f32) -> f32 {
        let t = t.clamp(0., 1.);
        match self {
            ScrollEasing::Linear => t,
            ScrollEasing::EaseOut => 1. - (1. - t).powi(3),
            ScrollEasing::EaseInOut => {
                if t < 0.5 {
                    4. * t.powi(3)
                } else {
                    1. - (2. - 2. * t).powi(3) / 2.
                }
            }
        }
    }
}

/// what a program may do with the clipboard through osc 52
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardAccess {
    Allow,
    /// show a notification with allow and deny buttons
    Ask,
    Deny,
}

/// when search tells upper and lower case apart
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SearchCase {
    /// only when the query has an upper case letter
    Smart,
    Sensitive,
    Insensitive,
}

// written as "smart", true or false
impl<'de> Deserialize<'de> for SearchCase {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bool(bool),
            Name(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Bool(true) => Ok(Self::Sensitive),
            Raw::Bool(false) => Ok(Self::Insensitive),
            Raw::Name(name) if name == "smart" => Ok(Self::Smart),
            Raw::Name(name) => Err(serde::de::Error::custom(format!(
                "unknown case_sensitive {name:?}, use \"smart\", true or false"
            ))),
        }
    }
}

/// key held to underline links under the mouse and open them with a click
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LinkModifier {
    Ctrl,
    Alt,
    Shift,
    /// windows key, cmd on macos
    #[serde(alias = "cmd")]
    Super,
}

impl LinkModifier {
    /// true when exactly this modifier is held
    pub fn held(&self, modifiers: &gpui::Modifiers) -> bool {
        let wanted = match self {
            LinkModifier::Ctrl => modifiers.control,
            LinkModifier::Alt => modifiers.alt,
            LinkModifier::Shift => modifiers.shift,
            LinkModifier::Super => modifiers.platform,
        };
        wanted && modifiers.number_of_modifiers() == 1
    }
}

/// what happens with text a link or hint pattern matched
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PatternAction {
    /// open with `links.open_with`, or the system opener
    Open,
    Copy,
    /// type it into the terminal
    Paste,
    /// run a shell command with the text in $KUTERM_MATCH, in the folder of the tab
    Exec(String),
}

/// when notifications programs send are shown
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProgramNotifications {
    Always,
    /// only while the tab is not the one in front of a focused window
    Unfocused,
    Never,
}

/// whether tabs of the last session open again on launch
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RestoreSession {
    Always,
    Never,
    /// a notification asks
    Ask,
}

/// how other programs can drive kuterm
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteControl {
    Off,
    /// a unix socket in the runtime folder, for `kuterm msg`
    Socket,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_starts_at_0_and_ends_at_1() {
        for easing in [
            ScrollEasing::Linear,
            ScrollEasing::EaseOut,
            ScrollEasing::EaseInOut,
        ] {
            assert_eq!(easing.apply(0.), 0., "{easing:?}");
            assert_eq!(easing.apply(1.), 1., "{easing:?}");
            // out of range time is clamped
            assert_eq!(easing.apply(-1.), 0., "{easing:?}");
            assert_eq!(easing.apply(2.), 1., "{easing:?}");
            let mut last = 0.;
            for step in 1..=100 {
                let value = easing.apply(step as f32 / 100.);
                assert!(value >= last, "{easing:?} goes back at {step}");
                last = value;
            }
        }
        assert_eq!(ScrollEasing::Linear.apply(0.5), 0.5);
        assert!(ScrollEasing::EaseOut.apply(0.25) > 0.25);
        assert!(ScrollEasing::EaseInOut.apply(0.25) < 0.25);
        assert_eq!(ScrollEasing::EaseInOut.apply(0.5), 0.5);
    }

    #[test]
    fn cursor_blink_follows_mode() {
        for app_asks in [false, true] {
            assert!(CursorBlink::On.active(app_asks));
            assert!(!CursorBlink::Off.active(app_asks));
            assert_eq!(CursorBlink::App.active(app_asks), app_asks);
        }
    }

    #[test]
    fn line_height_values() {
        assert_eq!(LineHeight::Comfortable.value(), 1.618);
        assert_eq!(LineHeight::Standard.value(), 1.3);
        assert_eq!(LineHeight::Custom(2.0).value(), 2.0);
    }

    #[test]
    fn cursor_shapes_map_to_alacritty() {
        assert_eq!(
            AlacCursorShape::from(CursorShape::Block),
            AlacCursorShape::Block
        );
        assert_eq!(
            AlacCursorShape::from(CursorShape::Bar),
            AlacCursorShape::Beam
        );
        assert_eq!(
            AlacCursorShape::from(CursorShape::Underline),
            AlacCursorShape::Underline
        );
        assert_eq!(
            AlacCursorShape::from(CursorShape::Hollow),
            AlacCursorShape::HollowBlock
        );
    }
}
