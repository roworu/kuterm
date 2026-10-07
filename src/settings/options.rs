//! value types for settings that have a fixed set of choices

use alacritty_terminal::vte::ansi::CursorShape as AlacCursorShape;
use serde::Deserialize;

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
}

/// where the title sits inside its tab
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TabTitleAlign {
    Left,
    Center,
    Right,
}

/// where the new tab button sits in the tab bar
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NewTabButton {
    Left,
    Right,
    AfterTabs,
}

/// which side of the tab the icon sits on
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TabIconPosition {
    Left,
    Right,
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
