//! user settings, read from a jsonc file

mod commands;
mod keybindings;
mod options;
mod pins;
mod tab_icons;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{App, Global};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json_lenient::Value;

pub use commands::{Command, CommandAction, Commands};
pub use keybindings::Keybindings;
pub use options::{
    CursorBlink, CursorShape, LineHeight, NewTabButton, ScrollEasing, ScrollbarEnable,
    ScrollbarPlacement, Shell, TabIconPosition, TabTitleAlign, TabTitleBlock, ThemeMode,
};
pub use pins::Pins;
pub use tab_icons::TabIcons;

use crate::cli::Cli;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Settings {
    pub ui_font_family: String,
    pub ui_font_size: f32,
    pub hide_bar_for_one_tab: bool,
    pub expand_tabs: bool,
    pub tab_width: u32,
    pub tab_title: Vec<TabTitleBlock>,
    pub tab_title_align: TabTitleAlign,
    pub tab_icon: TabIconSettings,
    pub show_tab_close_button: bool,
    pub drag_tabs: bool,
    pub close_running_tab_warn: bool,
    pub new_tab_button: NewTabButton,
    pub window_title: Vec<TabTitleBlock>,
    pub default_title: String,
    pub theme: ThemeSettings,
    pub terminal: TerminalSettings,
    pub profiles: Vec<Profile>,
    pub command_palette: CommandPaletteSettings,
    pub notifications: NotificationSettings,
}

impl Default for Settings {
    fn default() -> Self {
        serde_json_lenient::from_str(DEFAULT_SETTINGS)
            .expect("bundled default settings are invalid")
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ThemeSettings {
    pub mode: ThemeMode,
    /// custom dark theme file, bundled one when none
    pub dark: Option<PathBuf>,
    /// custom light theme file, bundled one when none
    pub light: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CommandPaletteSettings {
    pub enable: bool,
    pub show_recent: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct NotificationSettings {
    pub enable: bool,
    /// seconds a notification stays, 0 keeps it until clicked
    pub timeout: f32,
    /// notify when text is copied, showing it shortened
    pub copy: bool,
    /// notify when text is pasted
    pub paste: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TabIconSettings {
    pub position: TabIconPosition,
    pub dynamic: bool,
    pub default: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TerminalSettings {
    pub font_family: String,
    pub font_size: f32,
    pub line_height: LineHeight,
    pub cursor_shape: CursorShape,
    pub cursor_blink: CursorBlink,
    /// milliseconds the blinking cursor stays shown, then hidden
    pub cursor_blink_interval: f32,
    pub max_history_length: usize,
    /// option key sends esc prefixed keys like alt does on linux, macos only
    pub option_as_meta: bool,
    pub scrollbar: ScrollbarSettings,
    pub smooth_scroll: SmoothScrollSettings,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ScrollbarSettings {
    pub enable: ScrollbarEnable,
    pub placement: ScrollbarPlacement,
    pub width: f32,
    /// seconds without scrolling before it hides, 0 never hides
    pub auto_hide: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct SmoothScrollSettings {
    pub enable: bool,
    /// milliseconds one glide takes, 0 jumps right away
    pub duration: f32,
    pub easing: ScrollEasing,
}

impl SmoothScrollSettings {
    /// true when scrolling should glide instead of jump
    pub fn active(&self) -> bool {
        self.enable && self.duration > 0.
    }
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Settings::default().terminal
    }
}

/// what a new tab starts with
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Profile {
    pub name: String,
    /// opened by the new tab action and a left click on "+"
    #[serde(default)]
    pub default: bool,
    pub command: Shell,
    /// folder to start in, kuterm's own folder when none
    pub working_directory: Option<PathBuf>,
    /// color scheme of the terminal, global theme when none
    pub theme: Option<ThemeSettings>,
    pub icon: Option<String>,
    /// extra environment variables for the command
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl Global for Settings {}

// smaller fonts break the terminal grid, bigger ones stop growing
const FONT_SIZE_RANGE: (f32, f32) = (6., 72.);
// lines below 1 overlap, above 3 waste the screen
const LINE_HEIGHT_RANGE: (f32, f32) = (1., 3.);
// thinner bars are hard to grab, wider ones eat the grid
const SCROLLBAR_WIDTH_RANGE: (f32, f32) = (2., 64.);
// Duration panics on negative or huge seconds, an hour is already "never"
const AUTO_HIDE_RANGE: (f32, f32) = (0., 3600.);
// faster blinking flickers, slower looks like a hang
const CURSOR_BLINK_INTERVAL_RANGE: (f32, f32) = (100., 2000.);
// longer glides feel like lag, not smoothness
const SMOOTH_SCROLL_DURATION_RANGE: (f32, f32) = (0., 1000.);

pub const MAX_HISTORY_LENGTH: usize = u32::MAX as usize;

// Duration panics on negative or huge seconds, an hour is already "until clicked"
const NOTIFICATION_TIMEOUT_RANGE: (f32, f32) = (0., 3600.);

fn limit(name: &str, value: f32, (min, max): (f32, f32)) -> Option<f32> {
    if value.is_nan() || value < min {
        eprintln!("{name} {value} is below {min}, using the default");
        return None;
    }
    if value > max {
        eprintln!("{name} {value} is above {max}, using {max}");
    }
    Some(value.min(max))
}

/// commented settings file written on first launch
pub const DEFAULT_SETTINGS: &str = include_str!("../../assets/default_settings.jsonc");

/// write a default config file if it does not exist yet, so users can see what to change
pub fn create_default_file(path: &Path, contents: &str) {
    if path.exists() {
        return;
    }
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, contents));
    if let Err(error) = result {
        eprintln!("failed to create {}: {error}", path.display());
    }
}

/// read a config file, creating it from `defaults` first; falls back to defaults when missing or invalid
pub(crate) fn load_file<T: Default>(
    path: Option<PathBuf>,
    defaults: &str,
    what: &str,
    parse: impl Fn(&str) -> serde_json_lenient::Result<T>,
) -> T {
    let Some(path) = path else {
        return T::default();
    };
    create_default_file(&path, defaults);
    let Ok(json) = std::fs::read_to_string(&path) else {
        return T::default();
    };
    parse(&json).unwrap_or_else(|error| {
        eprintln!("invalid {what} in {}: {error}", path.display());
        T::default()
    })
}

/// `$XDG_CONFIG_HOME/kuterm`, falling back to `~/.config/kuterm`
pub fn config_dir() -> Option<PathBuf> {
    // xdg says empty or relative values must be ignored
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config_dir.join("kuterm"))
}

impl Settings {
    /// `--config-file`, or `settings.jsonc` in the config dir
    pub fn path() -> Option<PathBuf> {
        Cli::get()
            .config_file
            .clone()
            .or_else(|| Some(config_dir()?.join("settings.jsonc")))
    }

    /// parse settings, we allow comments and trailing commas
    pub fn parse(json: &str) -> serde_json_lenient::Result<Self> {
        let mut settings: Self = parse_over(DEFAULT_SETTINGS, json)?;
        let defaults = Self::default();
        settings.ui_font_size = limit("ui_font_size", settings.ui_font_size, FONT_SIZE_RANGE)
            .unwrap_or(defaults.ui_font_size);
        let terminal = &mut settings.terminal;
        terminal.font_size = limit("terminal.font_size", terminal.font_size, FONT_SIZE_RANGE)
            .unwrap_or(defaults.terminal.font_size);
        if let LineHeight::Custom(value) = terminal.line_height {
            terminal.line_height = limit("terminal.line_height", value, LINE_HEIGHT_RANGE)
                .map_or(defaults.terminal.line_height, LineHeight::Custom);
        }
        terminal.cursor_blink_interval = limit(
            "terminal.cursor_blink_interval",
            terminal.cursor_blink_interval,
            CURSOR_BLINK_INTERVAL_RANGE,
        )
        .unwrap_or(defaults.terminal.cursor_blink_interval);
        if terminal.max_history_length > MAX_HISTORY_LENGTH {
            eprintln!(
                "terminal.max_history_length {} is above {MAX_HISTORY_LENGTH}, using {MAX_HISTORY_LENGTH}",
                terminal.max_history_length
            );
            terminal.max_history_length = MAX_HISTORY_LENGTH;
        }
        let scrollbar = &mut terminal.scrollbar;
        scrollbar.width = limit(
            "terminal.scrollbar.width",
            scrollbar.width,
            SCROLLBAR_WIDTH_RANGE,
        )
        .unwrap_or(defaults.terminal.scrollbar.width);
        scrollbar.auto_hide = limit(
            "terminal.scrollbar.auto_hide",
            scrollbar.auto_hide,
            AUTO_HIDE_RANGE,
        )
        .unwrap_or(defaults.terminal.scrollbar.auto_hide);
        let smooth_scroll = &mut terminal.smooth_scroll;
        smooth_scroll.duration = limit(
            "terminal.smooth_scroll.duration",
            smooth_scroll.duration,
            SMOOTH_SCROLL_DURATION_RANGE,
        )
        .unwrap_or(defaults.terminal.smooth_scroll.duration);
        settings.notifications.timeout = limit(
            "notifications.timeout",
            settings.notifications.timeout,
            NOTIFICATION_TIMEOUT_RANGE,
        )
        .unwrap_or(defaults.notifications.timeout);
        if settings.profiles.is_empty() {
            eprintln!("profiles is empty, using the default profiles");
            settings.profiles = defaults.profiles;
        }
        // exactly one profile is the default, the first marked one wins
        match settings.profiles.iter().position(|profile| profile.default) {
            Some(first) => {
                let (head, rest) = settings.profiles.split_at_mut(first + 1);
                for profile in rest.iter_mut().filter(|profile| profile.default) {
                    eprintln!(
                        "profile {:?} is also default, using {:?}",
                        profile.name, head[first].name
                    );
                    profile.default = false;
                }
            }
            None => settings.profiles[0].default = true,
        }
        Ok(settings)
    }

    /// replace font families that are not installed with the bundled defaults
    pub fn use_installed_fonts(&mut self, installed: &[String]) {
        // the system fallback is usually proportional, which breaks the terminal grid
        let defaults = Self::default();
        for (name, family, default) in [
            (
                "ui_font_family",
                &mut self.ui_font_family,
                defaults.ui_font_family,
            ),
            (
                "terminal.font_family",
                &mut self.terminal.font_family,
                defaults.terminal.font_family,
            ),
        ] {
            if !installed.contains(family) {
                eprintln!("{name} {family:?} is not installed, using {default:?}");
                *family = default;
            }
        }
    }

    /// profile opened by the new tab action
    pub fn default_profile(&self) -> &Profile {
        self.profiles
            .iter()
            .find(|profile| profile.default)
            .unwrap_or(&self.profiles[0])
    }

    /// load settings from the settings file, using defaults when it is missing or invalid
    pub fn load() -> Self {
        load_file(Self::path(), DEFAULT_SETTINGS, "settings", Self::parse)
    }

    /// set terminal font size, kept inside the allowed range
    pub fn set_font_size(&mut self, size: f32) {
        self.terminal.font_size = size.clamp(FONT_SIZE_RANGE.0, FONT_SIZE_RANGE.1);
    }

    /// settings loaded at startup
    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}

// user values override defaults key by key, so a partial file keeps the other defaults.
// anything that is not an object on both sides (enum values, arrays) is replaced as a whole
pub(crate) fn merge(base: &mut Value, overrides: Value) {
    match (base, overrides) {
        (Value::Object(base), Value::Object(overrides)) => {
            for (key, value) in overrides {
                // a missing key starts as null, which the fallback arm replaces with the value
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, overrides) => *base = overrides,
    }
}

/// parse `json` deep merged over the bundled `defaults`
pub(crate) fn parse_over<T: DeserializeOwned>(
    defaults: &str,
    json: &str,
) -> serde_json_lenient::Result<T> {
    let mut value: Value = serde_json_lenient::from_str(defaults)?;
    merge(&mut value, serde_json_lenient::from_str(json)?);
    serde_json_lenient::from_value(value)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;

    // env vars are process wide, so tests that set XDG_CONFIG_HOME take this lock
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// fresh temp dir unique to this test process
    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kuterm_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// run `f` with XDG_CONFIG_HOME pointing at `dir`, restoring it afterwards
    pub(crate) fn with_config_home(dir: &Path, f: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir) };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        unsafe {
            match old {
                Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn empty_file_uses_defaults() {
        let settings = Settings::parse("{}").unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn bundled_settings_are_commented_and_parse() {
        assert!(DEFAULT_SETTINGS.contains("//"));
        assert_eq!(
            Settings::parse(DEFAULT_SETTINGS).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn parse_settings() {
        let settings = Settings::parse(
            r#"{
                // comments are allowed
                "ui_font_family": "JetBrainsMonoNL Nerd Font Mono",
                "ui_font_size": 16, // inline comments too
                "profiles": [{
                    "name": "bash",
                    "command": {
                        "with_arguments": { "program": "/bin/bash", "args": ["--login"] }
                    },
                }],
                "terminal": {
                    "font_family": "JetBrainsMonoNL Nerd Font Mono",
                    "font_size": 16,
                    "line_height": { "custom": 2 },
                    "cursor_shape": "bar",
                },
            }"#,
        )
        .unwrap();
        assert_eq!(settings.ui_font_family, "JetBrainsMonoNL Nerd Font Mono");
        assert_eq!(settings.ui_font_size, 16.);
        assert_eq!(
            settings.default_profile().command,
            Shell::WithArguments {
                program: "/bin/bash".into(),
                args: vec!["--login".into()],
            }
        );
        assert_eq!(
            settings.terminal.font_family,
            "JetBrainsMonoNL Nerd Font Mono"
        );
        assert_eq!(settings.terminal.font_size, 16.0);
        assert_eq!(settings.terminal.line_height.value(), 2.);
        assert_eq!(settings.terminal.cursor_shape, CursorShape::Bar);
    }

    #[test]
    fn parses_shell_variants() {
        let parse = |command: &str| {
            let json = format!(r#"{{"profiles": [{{"name": "a", "command": {command}}}]}}"#);
            Settings::parse(&json).unwrap().profiles[0].command.clone()
        };
        assert_eq!(parse(r#""system""#), Shell::System);
        assert_eq!(parse(r#"{"program": "zsh"}"#), Shell::Program("zsh".into()));
    }

    #[test]
    fn partial_terminal_section_keeps_other_defaults() {
        let settings = Settings::parse(r#"{"terminal": {"cursor_shape": "hollow"}}"#).unwrap();
        let defaults = Settings::default();
        assert_eq!(settings.terminal.cursor_shape, CursorShape::Hollow);
        assert_eq!(
            settings.terminal.cursor_blink,
            defaults.terminal.cursor_blink
        );
        assert_eq!(settings.terminal.font_size, defaults.terminal.font_size);
        assert_eq!(settings.ui_font_size, defaults.ui_font_size);
    }

    #[test]
    fn partial_top_level_section_keeps_terminal_defaults() {
        let settings = Settings::parse(r#"{"ui_font_size": 12}"#).unwrap();
        let expected = Settings {
            ui_font_size: 12.,
            ..Settings::default()
        };
        assert_eq!(settings, expected);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let settings =
            Settings::parse(r#"{"foo": 1, "terminal": {"bar": true, "font_size": 14}}"#).unwrap();
        let mut expected = Settings::default();
        expected.terminal.font_size = 14.;
        assert_eq!(settings, expected);
    }

    #[test]
    fn parses_tab_title_blocks() {
        let settings = Settings::parse(
            r#"{"tab_title": ["number", {"text": ": "}, "prompt", "folder", "command", "title", {"exec": "date"}]}"#,
        )
        .unwrap();
        assert_eq!(
            settings.tab_title,
            vec![
                TabTitleBlock::Number,
                TabTitleBlock::Text(": ".into()),
                TabTitleBlock::Prompt,
                TabTitleBlock::Folder,
                TabTitleBlock::Command,
                TabTitleBlock::Title,
                TabTitleBlock::Exec("date".into()),
            ]
        );
        assert!(Settings::parse(r#"{"tab_title": ["unknown"]}"#).is_err());
        assert_eq!(Settings::default().tab_title_align, TabTitleAlign::Left);
        let align = |json: &str| Settings::parse(json).unwrap().tab_title_align;
        assert_eq!(
            align(r#"{"tab_title_align": "center"}"#),
            TabTitleAlign::Center
        );
        assert_eq!(
            align(r#"{"tab_title_align": "right"}"#),
            TabTitleAlign::Right
        );
        assert!(Settings::parse(r#"{"tab_title_align": "middle"}"#).is_err());
        assert!(Settings::parse(r#"{"tab_title": "number"}"#).is_err());
    }

    #[test]
    fn theme_paths_keep_other_defaults() {
        let settings = Settings::parse(r#"{"theme": {"dark": "themes/d.jsonc"}}"#).unwrap();
        assert_eq!(settings.theme.mode, ThemeMode::System);
        assert_eq!(settings.theme.dark, Some(PathBuf::from("themes/d.jsonc")));
        assert_eq!(settings.theme.light, Settings::default().theme.light);

        let settings = Settings::parse(
            r#"{"theme": {"mode": "dark", "dark": null, "light": "/abs/l.jsonc"}}"#,
        )
        .unwrap();
        assert_eq!(settings.theme.mode, ThemeMode::Dark);
        assert_eq!(settings.theme.dark, None);
        assert_eq!(settings.theme.light, Some(PathBuf::from("/abs/l.jsonc")));
        assert_eq!(settings.terminal, Settings::default().terminal);
    }

    #[test]
    fn rejects_unknown_values() {
        for json in [
            "",
            "{",
            "not json",
            "[]",
            "5",
            "null",
            r#"{"terminal": 5}"#,
            r#"{"tab_width": -1}"#,
            r#"{"tab_width": 60.5}"#,
            r#"{"ui_font_size": "big"}"#,
            r#"{"terminal": {"cursor_shape": "triangle"}}"#,
            r#"{"terminal": {"cursor_blink": true}}"#,
            r#"{"terminal": {"line_height": "tall"}}"#,
            r#"{"profiles": [{"name": "a", "command": {"unknown": "zsh"}}]}"#,
            r#"{"profiles": [{"name": "a", "command": {"with_arguments": {"program": "bash"}}}]}"#,
            r#"{"profiles": [{"name": "a"}]}"#,
            r#"{"profiles": [{"command": "system"}]}"#,
            r#"{"profiles": [{"name": "a", "command": "system", "env": {"A": 1}}]}"#,
            r#"{"profiles": [{"name": "a", "command": "system", "theme": "dark"}]}"#,
            r#"{"profiles": {"name": "a", "command": "system"}}"#,
            r#"{"theme": "dark"}"#,
            r#"{"theme": {"mode": "auto"}}"#,
            r#"{"theme": {"mode": null}}"#,
            r#"{"theme": {"dark": 5}}"#,
        ] {
            assert!(
                Settings::parse(json).is_err(),
                "expected error for {json:?}"
            );
        }
    }

    #[test]
    fn create_default_file_creates_dirs_and_never_overwrites() {
        let dir = temp_dir("create_default");
        let path = dir.join("a").join("b").join("file.jsonc");
        create_default_file(&path, "first");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        create_default_file(&path, "second");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_creates_and_reads_settings_file() {
        let dir = temp_dir("settings_load");
        with_config_home(&dir, || {
            let path = dir.join("kuterm").join("settings.jsonc");
            assert_eq!(Settings::path(), Some(path.clone()));

            // missing file is created from the bundled one
            assert_eq!(Settings::load(), Settings::default());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_SETTINGS);

            std::fs::write(&path, r#"{"theme": {"mode": "light", "light": "x.jsonc"}}"#).unwrap();
            let settings = Settings::load();
            assert_eq!(settings.theme.mode, ThemeMode::Light);
            assert_eq!(settings.theme.light, Some(PathBuf::from("x.jsonc")));

            // broken file falls back to defaults and is left untouched
            let broken = r#"{"theme": {"mode": "auto"}}"#;
            std::fs::write(&path, broken).unwrap();
            assert_eq!(Settings::load(), Settings::default());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn font_json(field: &str, value: &str) -> String {
        match field {
            "ui_font_size" => format!(r#"{{"ui_font_size": {value}}}"#),
            _ => format!(r#"{{"terminal": {{"font_size": {value}}}}}"#),
        }
    }

    fn parse_font(field: &str, value: &str) -> f32 {
        let settings = Settings::parse(&font_json(field, value)).unwrap();
        match field {
            "ui_font_size" => settings.ui_font_size,
            _ => settings.terminal.font_size,
        }
    }

    #[test]
    fn font_size_below_min_uses_default() {
        let defaults = Settings::default();
        for field in ["ui_font_size", "terminal.font_size"] {
            for value in [
                "0", "0.0", "-0", "-0.0", "1", "0.5", "1e-30", "5", "-1", "-6", "-16", "-72",
                "-100", "-1e30", "-3.4e38",
            ] {
                let got = parse_font(field, value);
                let default = match field {
                    "ui_font_size" => defaults.ui_font_size,
                    _ => defaults.terminal.font_size,
                };
                // exactly the default, not -0.0 or a clamp to 6
                assert_eq!(
                    got.to_bits(),
                    default.to_bits(),
                    "{field} {value} gave {got}"
                );
            }
        }
    }

    #[test]
    fn font_size_above_max_is_capped() {
        for field in ["ui_font_size", "terminal.font_size"] {
            for value in [
                "72.5", "73", "80", "100", "200", "1000", "1e6", "1e30", "3.4e38", "1e39", "1e300",
            ] {
                let got = parse_font(field, value);
                assert_eq!(got, 72.0, "{field} {value} gave {got}");
            }
        }
    }

    #[test]
    fn line_height_custom_boundaries() {
        let lh = |v: &str| {
            let json = format!(r#"{{"terminal": {{"line_height": {{"custom": {v}}}}}}}"#);
            Settings::parse(&json).unwrap().terminal.line_height
        };
        assert_eq!(lh("0.99"), LineHeight::Standard);
        assert_eq!(lh("0.9999"), LineHeight::Standard);
        assert_eq!(lh("1"), LineHeight::Custom(1.0));
        assert_eq!(lh("3"), LineHeight::Custom(3.0));
        assert_eq!(lh("3.01"), LineHeight::Custom(3.0));
        assert_eq!(lh("3.0001"), LineHeight::Custom(3.0));
    }

    #[test]
    fn tab_title_array_replaces_default_whole() {
        // default has 3 blocks, a shorter or longer user array must not be index merged
        let settings = Settings::parse(r#"{"tab_title": ["number", "prompt"]}"#).unwrap();
        assert_eq!(
            settings.tab_title,
            vec![TabTitleBlock::Number, TabTitleBlock::Prompt]
        );
        let settings = Settings::parse(r#"{"tab_title": ["folder"]}"#).unwrap();
        assert_eq!(settings.tab_title, vec![TabTitleBlock::Folder]);
    }

    #[test]
    fn bundled_settings_have_one_default_profile() {
        let settings = Settings::default();
        assert_eq!(settings.profiles.len(), 1);
        let profile = settings.default_profile();
        assert!(profile.default);
        assert_eq!(profile.command, Shell::System);
        assert_eq!(profile.working_directory, None);
        assert_eq!(profile.theme, None);
        assert!(profile.env.is_empty());
    }

    #[test]
    fn profile_optional_keys_can_be_left_out() {
        let settings = Settings::parse(
            r#"{"profiles": [
                {"name": "a", "command": "system"},
                {
                    "name": "b",
                    "default": true,
                    "command": {"program": "zsh"},
                    "working_directory": "~/src",
                    "theme": {"mode": "dark", "dark": "themes/b.jsonc"},
                    "env": {"EDITOR": "vim"},
                },
            ]}"#,
        )
        .unwrap();
        let [a, b] = &settings.profiles[..] else {
            panic!("expected two profiles");
        };
        assert!(!a.default);
        assert_eq!(a.working_directory, None);
        assert_eq!(a.theme, None);
        assert!(a.env.is_empty());
        assert_eq!(settings.default_profile(), b);
        assert_eq!(b.command, Shell::Program("zsh".into()));
        assert_eq!(b.working_directory, Some(PathBuf::from("~/src")));
        let theme = b.theme.as_ref().unwrap();
        assert_eq!(theme.mode, ThemeMode::Dark);
        assert_eq!(theme.dark, Some(PathBuf::from("themes/b.jsonc")));
        assert_eq!(theme.light, None);
        assert_eq!(b.env["EDITOR"], "vim");
    }

    #[test]
    fn exactly_one_profile_is_default() {
        let a = r#"{"name": "a", "command": "system"}"#;
        let a_default = r#"{"name": "a", "default": true, "command": "system"}"#;
        let b = r#"{"name": "b", "command": "system"}"#;
        let b_default = r#"{"name": "b", "default": true, "command": "system"}"#;
        let c_default = r#"{"name": "c", "default": true, "command": "system"}"#;
        // default flags of the parsed profiles, in order
        let parse = |profiles: &[&str]| -> Vec<bool> {
            let json = format!(r#"{{"profiles": [{}]}}"#, profiles.join(","));
            let settings = Settings::parse(&json).unwrap();
            settings
                .profiles
                .iter()
                .map(|profile| profile.default)
                .collect()
        };
        // none marked, the first one is used
        assert_eq!(parse(&[a, b]), [true, false]);
        assert_eq!(parse(&[b_default]), [true]);
        assert_eq!(parse(&[a, b_default]), [false, true]);
        // several marked, the first marked one wins
        assert_eq!(parse(&[a, b_default, c_default]), [false, true, false]);
        assert_eq!(
            parse(&[a_default, b_default, c_default]),
            [true, false, false]
        );
    }

    #[test]
    fn parses_tab_icon() {
        let defaults = Settings::default().tab_icon;
        let parse = |json: &str| Settings::parse(json).unwrap().tab_icon;
        for (json, position) in [
            (
                r#"{"tab_icon": {"position": "left"}}"#,
                TabIconPosition::Left,
            ),
            (
                r#"{"tab_icon": {"position": "right"}}"#,
                TabIconPosition::Right,
            ),
        ] {
            let expected = TabIconSettings {
                position,
                ..defaults.clone()
            };
            assert_eq!(parse(json), expected);
        }
        for dynamic in [true, false] {
            let json = format!(r#"{{"tab_icon": {{"dynamic": {dynamic}, "default": "D"}}}}"#);
            let icon = parse(&json);
            assert_eq!(icon.dynamic, dynamic);
            assert_eq!(icon.default, "D");
            assert_eq!(icon.position, defaults.position);
        }

        for json in [
            r#"{"tab_icon": "left"}"#,
            r#"{"tab_icon": {"position": "top"}}"#,
            r#"{"tab_icon": {"dynamic": "yes"}}"#,
            r#"{"tab_icon": {"default": null}}"#,
            r#"{"profiles": [{"name": "a", "command": "system", "icon": 5}]}"#,
        ] {
            assert!(
                Settings::parse(json).is_err(),
                "expected error for {json:?}"
            );
        }
    }

    #[test]
    fn profile_icon_is_optional() {
        let settings = Settings::parse(
            r#"{"profiles": [
                {"name": "a", "command": "system"},
                {"name": "b", "command": "system", "icon": "B"},
                {"name": "c", "command": "system", "icon": null},
            ]}"#,
        )
        .unwrap();
        let icons: Vec<_> = settings
            .profiles
            .iter()
            .map(|p| p.icon.as_deref())
            .collect();
        assert_eq!(icons, [None, Some("B"), None]);
    }

    #[test]
    fn parses_scrollbar() {
        let settings = Settings::parse(
            r#"{"terminal": {"max_history_length": 0, "scrollbar": {
                "enable": "on", "placement": "left", "width": 12, "auto_hide": 1.5,
            }}}"#,
        )
        .unwrap();
        assert_eq!(settings.terminal.max_history_length, 0);
        assert_eq!(
            settings.terminal.scrollbar,
            ScrollbarSettings {
                enable: ScrollbarEnable::On,
                placement: ScrollbarPlacement::Left,
                width: 12.,
                auto_hide: 1.5,
            }
        );
        let enable = |value: &str| {
            let json = format!(r#"{{"terminal": {{"scrollbar": {{"enable": "{value}"}}}}}}"#);
            Settings::parse(&json).unwrap().terminal.scrollbar
        };
        let defaults = Settings::default().terminal.scrollbar;
        for (value, expected) in [
            ("off", ScrollbarEnable::Off),
            ("dynamic", ScrollbarEnable::Dynamic),
        ] {
            let scrollbar = enable(value);
            assert_eq!(scrollbar.enable, expected);
            // other keys keep their defaults
            assert_eq!(scrollbar.placement, defaults.placement);
            assert_eq!(scrollbar.width, defaults.width);
        }

        for json in [
            r#"{"terminal": {"scrollbar": "on"}}"#,
            r#"{"terminal": {"scrollbar": {"enable": true}}}"#,
            r#"{"terminal": {"scrollbar": {"placement": "top"}}}"#,
            r#"{"terminal": {"scrollbar": {"auto_hide": "1s"}}}"#,
            r#"{"terminal": {"max_history_length": -1}}"#,
        ] {
            assert!(
                Settings::parse(json).is_err(),
                "expected error for {json:?}"
            );
        }
    }

    #[test]
    fn scrollbar_width_limits() {
        let width = |value: &str| {
            let json = format!(r#"{{"terminal": {{"scrollbar": {{"width": {value}}}}}}}"#);
            Settings::parse(&json).unwrap().terminal.scrollbar.width
        };
        let default = Settings::default().terminal.scrollbar.width;
        assert_eq!(width("0"), default);
        assert_eq!(width("1.9"), default);
        assert_eq!(width("-5"), default);
        assert_eq!(width("2"), 2.);
        assert_eq!(width("64"), 64.);
        assert_eq!(width("65"), 64.);
        assert_eq!(width("1e30"), 64.);
    }

    #[test]
    fn scrollbar_auto_hide_limits() {
        let auto_hide = |value: &str| {
            let json = format!(r#"{{"terminal": {{"scrollbar": {{"auto_hide": {value}}}}}}}"#);
            Settings::parse(&json).unwrap().terminal.scrollbar.auto_hide
        };
        let default = Settings::default().terminal.scrollbar.auto_hide;
        assert_eq!(auto_hide("-1"), default);
        assert_eq!(auto_hide("-0.5"), default);
        assert_eq!(auto_hide("0"), 0.);
        assert_eq!(auto_hide("0.25"), 0.25);
        assert_eq!(auto_hide("3600"), 3600.);
        assert_eq!(auto_hide("3601"), 3600.);
        assert_eq!(auto_hide("1e30"), 3600.);
    }

    #[test]
    fn cursor_blink_parses_and_interval_limits() {
        let terminal = |json: &str| Settings::parse(json).unwrap().terminal;
        assert_eq!(
            terminal(r#"{"terminal": {"cursor_blink": "on"}}"#).cursor_blink,
            CursorBlink::On
        );
        assert_eq!(
            terminal(r#"{"terminal": {"cursor_blink": "off"}}"#).cursor_blink,
            CursorBlink::Off
        );
        let interval = |value: &str| {
            let json = format!(r#"{{"terminal": {{"cursor_blink_interval": {value}}}}}"#);
            terminal(&json).cursor_blink_interval
        };
        let default = Settings::default().terminal.cursor_blink_interval;
        assert_eq!(interval("0"), default);
        assert_eq!(interval("99"), default);
        assert_eq!(interval("100"), 100.);
        assert_eq!(interval("750"), 750.);
        assert_eq!(interval("2000"), 2000.);
        assert_eq!(interval("5000"), 2000.);
    }

    #[test]
    fn notification_timeout_limits() {
        let timeout = |value: &str| {
            let json = format!(r#"{{"notifications": {{"timeout": {value}}}}}"#);
            Settings::parse(&json).unwrap().notifications.timeout
        };
        let default = Settings::default().notifications.timeout;
        assert_eq!(timeout("-1"), default);
        assert_eq!(timeout("0"), 0.);
        assert_eq!(timeout("2.5"), 2.5);
        assert_eq!(timeout("3601"), 3600.);
    }

    #[test]
    fn parses_smooth_scroll() {
        let smooth = |json: &str| Settings::parse(json).unwrap().terminal.smooth_scroll;
        let parsed = smooth(
            r#"{"terminal": {"smooth_scroll": {"enable": false, "duration": 300, "easing": "linear"}}}"#,
        );
        assert_eq!(
            parsed,
            SmoothScrollSettings {
                enable: false,
                duration: 300.,
                easing: ScrollEasing::Linear,
            }
        );
        assert!(!parsed.active());
        let easing = |name: &str| {
            let json = format!(r#"{{"terminal": {{"smooth_scroll": {{"easing": "{name}"}}}}}}"#);
            smooth(&json).easing
        };
        assert_eq!(easing("ease_out"), ScrollEasing::EaseOut);
        assert_eq!(easing("ease_in_out"), ScrollEasing::EaseInOut);
        assert!(
            Settings::parse(r#"{"terminal": {"smooth_scroll": {"easing": "bounce"}}}"#).is_err()
        );
        assert!(Settings::parse(r#"{"terminal": {"smooth_scroll": {"enable": "yes"}}}"#).is_err());

        let duration = |value: &str| {
            let json = format!(r#"{{"terminal": {{"smooth_scroll": {{"duration": {value}}}}}}}"#);
            smooth(&json)
        };
        let default = Settings::default().terminal.smooth_scroll.duration;
        assert_eq!(duration("-1").duration, default);
        assert_eq!(duration("1000").duration, 1000.);
        assert_eq!(duration("5000").duration, 1000.);
        // 0 jumps, even when enabled
        let zero = duration(r#"0, "enable": true"#);
        assert_eq!(zero.duration, 0.);
        assert!(!zero.active());
    }

    #[test]
    fn max_history_length_is_capped() {
        let history = |value: &str| {
            let json = format!(r#"{{"terminal": {{"max_history_length": {value}}}}}"#);
            Settings::parse(&json).unwrap().terminal.max_history_length
        };
        assert_eq!(history("0"), 0);
        assert_eq!(history("5000"), 5000);
        assert_eq!(history("4294967295"), MAX_HISTORY_LENGTH);
        assert_eq!(history("18446744073709551615"), MAX_HISTORY_LENGTH);
    }

    #[test]
    fn missing_font_families_use_defaults() {
        let defaults = Settings::default();
        let mut settings = Settings::parse(
            r#"{"ui_font_family": "No Such Font", "terminal": {"font_family": "DejaVu Sans Mono"}}"#,
        )
        .unwrap();
        settings.use_installed_fonts(&[
            "DejaVu Sans Mono".to_string(),
            defaults.ui_font_family.clone(),
        ]);
        assert_eq!(settings.ui_font_family, defaults.ui_font_family);
        assert_eq!(settings.terminal.font_family, "DejaVu Sans Mono");

        settings.use_installed_fonts(&[]);
        assert_eq!(settings.terminal.font_family, defaults.terminal.font_family);
    }

    #[test]
    fn empty_profiles_use_bundled_ones() {
        assert_eq!(
            Settings::parse(r#"{"profiles": []}"#).unwrap().profiles,
            Settings::default().profiles
        );
    }
}

#[cfg(test)]
mod close_warn_settings {

    use crate::settings::{DEFAULT_SETTINGS, Settings};

    #[test]
    fn bundled_settings_have_close_running_tab_warn_bool() {
        let raw: serde_json_lenient::Value =
            serde_json_lenient::from_str(DEFAULT_SETTINGS).unwrap();
        assert!(
            raw["close_running_tab_warn"].is_boolean(),
            "missing close_running_tab_warn bool"
        );
    }

    #[test]
    fn default_matches_bundled_value() {
        let raw: serde_json_lenient::Value =
            serde_json_lenient::from_str(DEFAULT_SETTINGS).unwrap();
        assert_eq!(
            Some(Settings::default().close_running_tab_warn),
            raw["close_running_tab_warn"].as_bool()
        );
    }

    #[test]
    fn user_true_and_false_are_parsed() {
        for value in [true, false] {
            let s = Settings::parse(&format!(r#"{{"close_running_tab_warn": {value}}}"#)).unwrap();
            assert_eq!(s.close_running_tab_warn, value);
        }
    }

    #[test]
    fn missing_key_keeps_default() {
        assert_eq!(
            Settings::parse("{}").unwrap().close_running_tab_warn,
            Settings::default().close_running_tab_warn
        );
    }

    #[test]
    fn non_bool_value_is_rejected() {
        assert!(Settings::parse(r#"{"close_running_tab_warn": "yes"}"#).is_err());
        assert!(Settings::parse(r#"{"close_running_tab_warn": 1}"#).is_err());
    }
}

#[cfg(test)]
mod fonts_history_settings {

    use crate::settings::{MAX_HISTORY_LENGTH, Settings};

    fn with_fonts(ui: &str, terminal: &str) -> Settings {
        let json =
            format!(r#"{{"ui_font_family": "{ui}", "terminal": {{"font_family": "{terminal}"}}}}"#);
        Settings::parse(&json).unwrap()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn installed_custom_fonts_are_kept() {
        let mut s = with_fonts("Agent Ui Font", "Agent Mono");
        s.use_installed_fonts(&names(&["Other", "Agent Ui Font", "Agent Mono"]));
        assert_eq!(s.ui_font_family, "Agent Ui Font");
        assert_eq!(s.terminal.font_family, "Agent Mono");
    }

    #[test]
    fn missing_ui_font_uses_bundled_default_only() {
        let d = Settings::default();
        let mut s = with_fonts("Agent Missing Ui", "Agent Mono");
        s.use_installed_fonts(&names(&["Agent Mono"]));
        assert_eq!(s.ui_font_family, d.ui_font_family);
        assert_eq!(s.terminal.font_family, "Agent Mono");
    }

    #[test]
    fn missing_terminal_font_uses_bundled_default_only() {
        let d = Settings::default();
        let mut s = with_fonts("Agent Ui Font", "Agent Missing Mono");
        s.use_installed_fonts(&names(&["Agent Ui Font"]));
        assert_eq!(s.ui_font_family, "Agent Ui Font");
        assert_eq!(s.terminal.font_family, d.terminal.font_family);
    }

    #[test]
    fn nothing_installed_uses_both_defaults() {
        let d = Settings::default();
        let mut s = with_fonts("Agent A", "Agent B");
        s.use_installed_fonts(&[]);
        assert_eq!(s.ui_font_family, d.ui_font_family);
        assert_eq!(s.terminal.font_family, d.terminal.font_family);
    }

    #[test]
    fn empty_family_name_uses_default() {
        let d = Settings::default();
        let mut s = with_fonts("", "");
        s.use_installed_fonts(&names(&["Agent Mono"]));
        assert_eq!(s.ui_font_family, d.ui_font_family);
        assert_eq!(s.terminal.font_family, d.terminal.font_family);
    }

    #[test]
    fn only_font_fields_change() {
        let mut s = with_fonts("Agent A", "Agent B");
        let before = s.clone();
        s.use_installed_fonts(&[]);
        s.ui_font_family = before.ui_font_family.clone();
        s.terminal.font_family = before.terminal.font_family.clone();
        assert_eq!(s, before);
    }

    #[test]
    fn defaults_untouched_when_installed() {
        let d = Settings::default();
        let mut s = Settings::default();
        s.use_installed_fonts(&[d.ui_font_family.clone(), d.terminal.font_family.clone()]);
        assert_eq!(s, d);
    }

    #[test]
    fn use_installed_fonts_is_idempotent() {
        let mut s = with_fonts("Agent A", "Agent B");
        s.use_installed_fonts(&names(&["Agent B"]));
        let once = s.clone();
        s.use_installed_fonts(&names(&["Agent B"]));
        assert_eq!(s, once);
    }

    fn history(value: &str) -> serde_json_lenient::Result<usize> {
        let json = format!(r#"{{"terminal": {{"max_history_length": {value}}}}}"#);
        Settings::parse(&json).map(|s| s.terminal.max_history_length)
    }

    #[test]
    fn history_cap_is_u32_max() {
        assert_eq!(MAX_HISTORY_LENGTH, u32::MAX as usize);
    }

    #[test]
    fn history_at_cap_is_kept() {
        assert_eq!(
            history(&MAX_HISTORY_LENGTH.to_string()).unwrap(),
            MAX_HISTORY_LENGTH
        );
    }

    #[test]
    fn history_just_above_cap_is_capped() {
        assert_eq!(
            history(&(MAX_HISTORY_LENGTH + 1).to_string()).unwrap(),
            MAX_HISTORY_LENGTH
        );
    }

    #[test]
    fn history_usize_max_is_capped() {
        assert_eq!(
            history(&usize::MAX.to_string()).unwrap(),
            MAX_HISTORY_LENGTH
        );
    }

    #[test]
    fn history_below_cap_and_zero_unchanged() {
        assert_eq!(history("0").unwrap(), 0);
        assert_eq!(history("1").unwrap(), 1);
        assert_eq!(
            history(&(MAX_HISTORY_LENGTH - 1).to_string()).unwrap(),
            MAX_HISTORY_LENGTH - 1
        );
    }

    #[test]
    fn history_negative_still_rejected() {
        assert!(history("-5").is_err());
    }

    #[test]
    fn history_cap_keeps_other_settings() {
        let d = Settings::default();
        let json = format!(
            r#"{{"terminal": {{"max_history_length": {}}}}}"#,
            usize::MAX
        );
        let mut s = Settings::parse(&json).unwrap();
        s.terminal.max_history_length = d.terminal.max_history_length;
        assert_eq!(s, d);
    }

    #[test]
    fn bundled_history_within_cap() {
        assert!(Settings::default().terminal.max_history_length <= MAX_HISTORY_LENGTH);
    }
}

#[cfg(test)]
mod notifications_settings {

    use crate::settings::{CommandAction, Commands, DEFAULT_SETTINGS, Settings};

    fn actions(json: &str) -> serde_json_lenient::Result<Vec<CommandAction>> {
        let json = format!(r#"{{"commands": [{{"name": "x", "actions": [{json}]}}]}}"#);
        Commands::parse(&json).map(|mut c| c.commands.remove(0).actions)
    }

    #[test]
    fn bundled_settings_have_notification_keys() {
        let raw: serde_json_lenient::Value =
            serde_json_lenient::from_str(DEFAULT_SETTINGS).unwrap();
        let n = &raw["notifications"];
        assert!(n["enable"].is_boolean(), "missing notifications.enable");
        assert!(n["timeout"].is_number(), "missing notifications.timeout");
        assert!(n["copy"].is_boolean(), "missing notifications.copy");
        assert!(n["paste"].is_boolean(), "missing notifications.paste");
    }

    #[test]
    fn bundled_timeout_is_within_limits() {
        let timeout = Settings::default().notifications.timeout;
        assert!((0. ..=3600.).contains(&timeout), "{timeout}");
    }

    #[test]
    fn partial_notifications_keep_other_defaults() {
        let d = Settings::default().notifications;
        let s = Settings::parse(r#"{"notifications": {"enable": !ENABLE}}"#
        .replace("!ENABLE", if d.enable { "false" } else { "true" })
        .as_str())
    .unwrap()
    .notifications;
        assert_eq!(s.enable, !d.enable);
        assert_eq!(s.timeout, d.timeout);

        let s = Settings::parse(r#"{"notifications": {"timeout": 7}}"#)
            .unwrap()
            .notifications;
        assert_eq!(s.enable, d.enable);
        assert_eq!(s.timeout, 7.);
    }

    #[test]
    fn empty_user_settings_keep_notification_defaults() {
        assert_eq!(
            Settings::parse("{}").unwrap().notifications,
            Settings::default().notifications
        );
    }

    #[test]
    fn timeout_edges() {
        let t = |v: &str| {
            Settings::parse(&format!(r#"{{"notifications": {{"timeout": {v}}}}}"#))
                .unwrap()
                .notifications
                .timeout
        };
        assert_eq!(t("3600"), 3600.);
        assert_eq!(t("0.1"), 0.1);
        assert_eq!(t("1e9"), 3600.);
        assert_eq!(t("-0.5"), Settings::default().notifications.timeout);
    }

    #[test]
    fn wrong_notification_types_are_rejected() {
        for json in [
            r#"{"notifications": {"enable": "yes"}}"#,
            r#"{"notifications": {"timeout": "5"}}"#,
            r#"{"notifications": true}"#,
        ] {
            assert!(Settings::parse(json).is_err(), "{json} should be invalid");
        }
    }

    #[test]
    fn new_actions_parse() {
        assert_eq!(
            actions(
                r#""new_background_tab", {"new_background_tab_with_profile": "p"},
               {"notify": ""}, {"notify_when_done": "é ✓"}"#
            )
            .unwrap(),
            vec![
                CommandAction::NewBackgroundTab,
                CommandAction::NewBackgroundTabWithProfile("p".into()),
                CommandAction::Notify(String::new()),
                CommandAction::NotifyWhenDone("é ✓".into()),
            ]
        );
    }

    #[test]
    fn new_actions_reject_wrong_shapes() {
        for json in [
            r#""new_background_tab_with_profile""#,
            r#"{"new_background_tab_with_profile": 3}"#,
            r#""notify_when_done""#,
            r#"{"notify": 5}"#,
            r#"{"notify": ["a"]}"#,
            r#"{"notify_when_done": null}"#,
            r#""background_tab""#,
        ] {
            assert!(actions(json).is_err(), "{json} should be invalid");
        }
    }
}

#[cfg(test)]
mod profiles_settings {

    use std::path::PathBuf;

    use crate::settings::{DEFAULT_SETTINGS, Settings, Shell, ThemeMode};

    fn parse(profiles: &str) -> Settings {
        Settings::parse(&format!(r#"{{"profiles": [{profiles}]}}"#)).unwrap()
    }

    fn defaults(settings: &Settings) -> Vec<bool> {
        settings.profiles.iter().map(|p| p.default).collect()
    }

    fn default_count(settings: &Settings) -> usize {
        settings.profiles.iter().filter(|p| p.default).count()
    }

    #[test]
    fn bundled_profiles_parse_with_one_default() {
        let raw: serde_json_lenient::Value =
            serde_json_lenient::from_str(DEFAULT_SETTINGS).unwrap();
        let profiles = raw["profiles"].as_array().expect("profiles is an array");
        assert!(!profiles.is_empty());
        let s = Settings::default();
        assert_eq!(s.profiles.len(), profiles.len());
        assert_eq!(default_count(&s), 1);
    }

    #[test]
    fn empty_user_settings_give_one_default_profile() {
        let s = Settings::parse("{}").unwrap();
        assert_eq!(s.profiles, Settings::default().profiles);
        assert_eq!(default_count(&s), 1);
        assert!(s.default_profile().default);
    }

    #[test]
    fn user_profiles_replace_bundled_list() {
        let s = parse(
            r#"{"name": "x", "command": "system"}, {"name": "y", "command": {"program": "zsh"}}"#,
        );
        let names: Vec<&str> = s.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["x", "y"]);
    }

    #[test]
    fn profile_sets_command_dir_theme_env() {
        let s = parse(
            r#"{"name": "p", "default": true,
            "command": {"with_arguments": {"program": "/bin/sh", "args": ["-l"]}},
            "working_directory": "/tmp",
            "theme": {"mode": "light"},
            "env": {"A": "1", "B": ""}}"#,
        );
        let p = s.default_profile();
        assert_eq!(p.name, "p");
        assert_eq!(
            p.command,
            Shell::WithArguments {
                program: "/bin/sh".into(),
                args: vec!["-l".into()]
            }
        );
        assert_eq!(p.working_directory, Some(PathBuf::from("/tmp")));
        let theme = p.theme.as_ref().unwrap();
        assert_eq!(theme.mode, ThemeMode::Light);
        assert_eq!(p.env.len(), 2);
        assert_eq!(p.env["A"], "1");
        assert_eq!(p.env["B"], "");
    }

    #[test]
    fn only_one_default_when_all_marked() {
        let s = parse(
            r#"{"name": "a", "default": true, "command": "system"},
           {"name": "b", "default": true, "command": "system"},
           {"name": "c", "default": true, "command": "system"},
           {"name": "d", "default": true, "command": "system"}"#,
        );
        assert_eq!(defaults(&s), [true, false, false, false]);
        assert_eq!(s.default_profile().name, "a");
    }

    #[test]
    fn last_marked_default_is_kept_when_only_one() {
        let s = parse(
            r#"{"name": "a", "command": "system"},
           {"name": "b", "default": false, "command": "system"},
           {"name": "c", "default": true, "command": "system"}"#,
        );
        assert_eq!(defaults(&s), [false, false, true]);
        assert_eq!(s.default_profile().name, "c");
    }

    #[test]
    fn explicit_false_everywhere_picks_first() {
        let s = parse(
            r#"{"name": "a", "default": false, "command": "system"},
           {"name": "b", "default": false, "command": "system"}"#,
        );
        assert_eq!(defaults(&s), [true, false]);
        assert_eq!(s.default_profile().name, "a");
    }

    #[test]
    fn single_user_profile_without_flag_becomes_default() {
        let s = parse(r#"{"name": "solo", "command": {"program": "fish"}}"#);
        assert_eq!(defaults(&s), [true]);
        assert_eq!(s.default_profile().command, Shell::Program("fish".into()));
    }

    #[test]
    fn empty_profiles_fall_back_to_one_default() {
        let s = Settings::parse(r#"{"profiles": []}"#).unwrap();
        assert_eq!(s.profiles, Settings::default().profiles);
        assert_eq!(default_count(&s), 1);
    }

    #[test]
    fn duplicate_names_are_kept() {
        let s = parse(
            r#"{"name": "same", "command": "system"}, {"name": "same", "command": "system"}"#,
        );
        assert_eq!(s.profiles.len(), 2);
        assert_eq!(default_count(&s), 1);
    }

    #[test]
    fn invalid_profiles_are_errors() {
        for json in [
            r#"{"profiles": [{"name": "a", "command": "system", "default": "yes"}]}"#,
            r#"{"profiles": [{"name": 1, "command": "system"}]}"#,
            r#"{"profiles": [{"name": "a", "command": "system", "working_directory": 5}]}"#,
            r#"{"profiles": [{"name": "a", "command": "system", "env": ["A=1"]}]}"#,
            r#"{"profiles": [{"name": "a", "command": "system", "theme": {"mode": "blue"}}]}"#,
            r#"{"profiles": "default"}"#,
        ] {
            assert!(Settings::parse(json).is_err(), "should fail: {json}");
        }
    }

    #[test]
    fn null_optional_keys_are_accepted() {
        let s = parse(
            r#"{"name": "a", "command": "system", "working_directory": null, "theme": null}"#,
        );
        assert_eq!(s.profiles[0].working_directory, None);
        assert_eq!(s.profiles[0].theme, None);
    }

    #[test]
    fn every_parse_result_has_exactly_one_default() {
        let cases = [
            "{}",
            r#"{"profiles": []}"#,
            r#"{"profiles": [{"name": "a", "command": "system"}]}"#,
            r#"{"profiles": [{"name": "a", "command": "system"}, {"name": "b", "command": "system"}]}"#,
            r#"{"profiles": [{"name": "a", "default": true, "command": "system"}, {"name": "b", "default": true, "command": "system"}]}"#,
        ];
        for json in cases {
            let s = Settings::parse(json).unwrap();
            assert_eq!(default_count(&s), 1, "{json}");
            assert!(s.default_profile().default, "{json}");
        }
    }
}

#[cfg(test)]
mod scrollbar_settings {

    use crate::{
        settings::{DEFAULT_SETTINGS, ScrollbarEnable, ScrollbarPlacement, Settings},
        theme::{DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME, Theme},
    };

    fn terminal(json: &str) -> serde_json_lenient::Result<Settings> {
        Settings::parse(&format!(r#"{{"terminal": {json}}}"#))
    }

    fn bar(json: &str) -> crate::settings::ScrollbarSettings {
        terminal(&format!(r#"{{"scrollbar": {json}}}"#))
            .unwrap()
            .terminal
            .scrollbar
    }

    #[test]
    fn bundled_settings_have_all_scrollbar_keys() {
        let raw: serde_json_lenient::Value =
            serde_json_lenient::from_str(DEFAULT_SETTINGS).unwrap();
        let t = &raw["terminal"];
        assert!(t["max_history_length"].is_u64());
        let s = &t["scrollbar"];
        for key in ["enable", "placement", "width", "auto_hide"] {
            assert!(!s[key].is_null(), "missing terminal.scrollbar.{key}");
        }
    }

    #[test]
    fn bundled_defaults_parse_without_clamping() {
        let raw: serde_json_lenient::Value =
            serde_json_lenient::from_str(DEFAULT_SETTINGS).unwrap();
        let d = Settings::default().terminal;
        assert_eq!(
            d.max_history_length as u64,
            raw["terminal"]["max_history_length"].as_u64().unwrap()
        );
        assert_eq!(
            d.scrollbar.width as f64,
            raw["terminal"]["scrollbar"]["width"].as_f64().unwrap()
        );
        assert_eq!(
            d.scrollbar.auto_hide as f64,
            raw["terminal"]["scrollbar"]["auto_hide"].as_f64().unwrap()
        );
        assert_eq!(Settings::parse("{}").unwrap().terminal, d);
    }

    #[test]
    fn enable_on() {
        assert_eq!(bar(r#"{"enable": "on"}"#).enable, ScrollbarEnable::On);
    }

    #[test]
    fn enable_off() {
        assert_eq!(bar(r#"{"enable": "off"}"#).enable, ScrollbarEnable::Off);
    }

    #[test]
    fn enable_dynamic() {
        assert_eq!(
            bar(r#"{"enable": "dynamic"}"#).enable,
            ScrollbarEnable::Dynamic
        );
    }

    #[test]
    fn placement_left_and_right() {
        assert_eq!(
            bar(r#"{"placement": "left"}"#).placement,
            ScrollbarPlacement::Left
        );
        assert_eq!(
            bar(r#"{"placement": "right"}"#).placement,
            ScrollbarPlacement::Right
        );
    }

    #[test]
    fn width_in_range_is_kept() {
        assert_eq!(bar(r#"{"width": 5}"#).width, 5.);
        assert_eq!(bar(r#"{"width": 16.5}"#).width, 16.5);
    }

    #[test]
    fn width_out_of_range_is_not_used_verbatim() {
        let too_small = bar(r#"{"width": 0}"#).width;
        let too_big = bar(r#"{"width": 100000}"#).width;
        let negative = bar(r#"{"width": -3}"#).width;
        for w in [too_small, too_big, negative] {
            assert!(w > 0. && w < 1000., "width {w} not clamped");
        }
    }

    fn auto_hide(v: &str) -> f32 {
        bar(&format!(r#"{{"auto_hide": {v}}}"#)).auto_hide
    }

    #[test]
    fn auto_hide_zero_is_kept() {
        assert_eq!(auto_hide("0"), 0.);
        assert_eq!(auto_hide("0.0"), 0.);
    }

    #[test]
    fn auto_hide_whole_seconds() {
        assert_eq!(auto_hide("1"), 1.);
        assert_eq!(auto_hide("30"), 30.);
    }

    #[test]
    fn auto_hide_fractional_seconds() {
        assert_eq!(auto_hide("1.5"), 1.5);
        assert_eq!(auto_hide("0.1"), 0.1);
    }

    #[test]
    fn auto_hide_negative_falls_back_to_default() {
        let d = Settings::default().terminal.scrollbar.auto_hide;
        assert_eq!(auto_hide("-1"), d);
        assert_eq!(auto_hide("-0.001"), d);
        assert_eq!(auto_hide("-3600"), d);
    }

    #[test]
    fn auto_hide_capped_at_one_hour() {
        assert_eq!(auto_hide("3600"), 3600.);
        assert_eq!(auto_hide("3599.5"), 3599.5);
        assert_eq!(auto_hide("3600.5"), 3600.);
        assert_eq!(auto_hide("86400"), 3600.);
        assert_eq!(auto_hide("1e30"), 3600.);
    }

    #[test]
    fn auto_hide_huge_values_parse_to_cap_not_error() {
        assert_eq!(auto_hide("1e300"), 3600.);
    }

    #[test]
    fn auto_hide_invalid_type_rejected() {
        for v in [r#""1s""#, r#""fast""#, "true", "[1]"] {
            assert!(
                terminal(&format!(r#"{{"scrollbar": {{"auto_hide": {v}}}}}"#)).is_err(),
                "accepted auto_hide {v}"
            );
        }
    }

    #[test]
    fn bundled_auto_hide_is_in_range() {
        let d = Settings::default().terminal.scrollbar.auto_hide;
        assert!(d.is_finite() && (0. ..=3600.).contains(&d));
    }

    #[test]
    fn max_history_length_values() {
        let h = |v: &str| {
            terminal(&format!(r#"{{"max_history_length": {v}}}"#))
                .unwrap()
                .terminal
                .max_history_length
        };
        assert_eq!(h("0"), 0);
        assert_eq!(h("1"), 1);
        assert_eq!(h("123456"), 123456);
    }

    #[test]
    fn partial_scrollbar_keeps_other_defaults() {
        let d = Settings::default().terminal.scrollbar;
        let s = bar(r#"{"placement": "left"}"#);
        assert_eq!(s.enable, d.enable);
        assert_eq!(s.width, d.width);
        assert_eq!(s.auto_hide, d.auto_hide);
        let s = bar(r#"{"auto_hide": 7.5}"#);
        assert_eq!(s.enable, d.enable);
        assert_eq!(s.placement, d.placement);
    }

    #[test]
    fn scrollbar_settings_do_not_touch_history() {
        let d = Settings::default().terminal.max_history_length;
        let s = terminal(r#"{"scrollbar": {"enable": "off"}}"#).unwrap();
        assert_eq!(s.terminal.max_history_length, d);
    }

    #[test]
    fn invalid_enable_rejected() {
        for v in [r#""yes""#, r#""ON""#, "1", "null", r#""auto""#] {
            assert!(
                terminal(&format!(r#"{{"scrollbar": {{"enable": {v}}}}}"#)).is_err(),
                "accepted enable {v}"
            );
        }
    }

    #[test]
    fn invalid_placement_rejected() {
        for v in [r#""bottom""#, r#""Left""#, "0"] {
            assert!(
                terminal(&format!(r#"{{"scrollbar": {{"placement": {v}}}}}"#)).is_err(),
                "accepted placement {v}"
            );
        }
    }

    #[test]
    fn invalid_width_type_rejected() {
        assert!(terminal(r#"{"scrollbar": {"width": "8px"}}"#).is_err());
    }

    #[test]
    fn invalid_history_rejected() {
        for v in ["-5", "1.5", r#""inf""#] {
            assert!(
                terminal(&format!(r#"{{"max_history_length": {v}}}"#)).is_err(),
                "accepted max_history_length {v}"
            );
        }
    }

    #[test]
    fn bundled_themes_have_scrollbar_color() {
        for theme in [DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME] {
            let raw: serde_json_lenient::Value = serde_json_lenient::from_str(theme).unwrap();
            assert!(raw["scrollbar"].is_string());
        }
        let _ = Theme::bundled(true);
        let _ = Theme::bundled(false);
    }

    #[test]
    fn user_theme_sets_scrollbar_color() {
        for dark in [true, false] {
            let t =
                Theme::parse(r##"{"cursor": "#ff0000", "scrollbar": "#ff0000"}"##, dark).unwrap();
            assert_eq!(t.scrollbar, t.cursor);
            let other = Theme::parse(r##"{"scrollbar": "#00ff00"}"##, dark).unwrap();
            assert_ne!(other.scrollbar, t.scrollbar);
        }
    }

    #[test]
    fn partial_theme_keeps_bundled_scrollbar_color() {
        for dark in [true, false] {
            let t = Theme::parse(r##"{"cursor": "#123456"}"##, dark).unwrap();
            assert_eq!(t.scrollbar, Theme::bundled(dark).scrollbar);
        }
    }
}

#[cfg(test)]
mod smooth_scroll_settings {

    use crate::settings::{DEFAULT_SETTINGS, ScrollEasing, Settings, SmoothScrollSettings};

    fn parse(json: &str) -> serde_json_lenient::Result<Settings> {
        Settings::parse(&format!(r#"{{"terminal": {{"smooth_scroll": {json}}}}}"#))
    }

    fn smooth(json: &str) -> SmoothScrollSettings {
        parse(json).unwrap().terminal.smooth_scroll
    }

    fn bundled() -> serde_json_lenient::Value {
        let raw: serde_json_lenient::Value =
            serde_json_lenient::from_str(DEFAULT_SETTINGS).unwrap();
        raw["terminal"]["smooth_scroll"].clone()
    }

    const ALL: [ScrollEasing; 3] = [
        ScrollEasing::Linear,
        ScrollEasing::EaseOut,
        ScrollEasing::EaseInOut,
    ];

    #[test]
    fn bundled_settings_have_all_smooth_scroll_keys() {
        let s = bundled();
        assert!(s.is_object(), "missing terminal.smooth_scroll");
        assert!(s["enable"].is_boolean(), "enable is not a bool");
        assert!(s["duration"].is_number(), "duration is not a number");
        assert!(s["easing"].is_string(), "easing is not a string");
    }

    #[test]
    fn bundled_smooth_scroll_parses_as_written() {
        let s = bundled();
        let d = Settings::default().terminal.smooth_scroll;
        assert_eq!(d.enable, s["enable"].as_bool().unwrap());
        assert_eq!(d.duration as f64, s["duration"].as_f64().unwrap());
        let easing: ScrollEasing = serde_json_lenient::from_value(s["easing"].clone()).unwrap();
        assert_eq!(d.easing, easing);
        assert_eq!(Settings::parse("{}").unwrap().terminal.smooth_scroll, d);
    }

    #[test]
    fn bundled_duration_is_in_range() {
        let d = bundled()["duration"].as_f64().unwrap();
        assert!((0. ..=1000.).contains(&d), "{d}");
    }

    #[test]
    fn enable_true_and_false() {
        assert!(smooth(r#"{"enable": true}"#).enable);
        assert!(!smooth(r#"{"enable": false}"#).enable);
    }

    #[test]
    fn enable_invalid_type_rejected() {
        assert!(parse(r#"{"enable": "yes"}"#).is_err());
        assert!(parse(r#"{"enable": 1}"#).is_err());
    }

    #[test]
    fn easing_linear() {
        assert_eq!(
            smooth(r#"{"easing": "linear"}"#).easing,
            ScrollEasing::Linear
        );
    }

    #[test]
    fn easing_ease_out() {
        assert_eq!(
            smooth(r#"{"easing": "ease_out"}"#).easing,
            ScrollEasing::EaseOut
        );
    }

    #[test]
    fn easing_ease_in_out() {
        assert_eq!(
            smooth(r#"{"easing": "ease_in_out"}"#).easing,
            ScrollEasing::EaseInOut
        );
    }

    #[test]
    fn easing_unknown_rejected() {
        assert!(parse(r#"{"easing": "bounce"}"#).is_err());
        assert!(parse(r#"{"easing": "EaseOut"}"#).is_err());
        assert!(parse(r#"{"easing": 1}"#).is_err());
    }

    #[test]
    fn duration_in_range_is_kept() {
        assert_eq!(smooth(r#"{"duration": 0}"#).duration, 0.);
        assert_eq!(smooth(r#"{"duration": 1}"#).duration, 1.);
        assert_eq!(smooth(r#"{"duration": 275.5}"#).duration, 275.5);
        assert_eq!(smooth(r#"{"duration": 1000}"#).duration, 1000.);
    }

    #[test]
    fn duration_above_max_is_clamped() {
        assert_eq!(smooth(r#"{"duration": 1001}"#).duration, 1000.);
        assert_eq!(smooth(r#"{"duration": 1e30}"#).duration, 1000.);
    }

    #[test]
    fn duration_negative_is_not_used() {
        let d = smooth(r#"{"duration": -5}"#).duration;
        assert!((0. ..=1000.).contains(&d), "{d}");
        assert_eq!(d, Settings::default().terminal.smooth_scroll.duration);
    }

    #[test]
    fn duration_invalid_type_rejected() {
        assert!(parse(r#"{"duration": "fast"}"#).is_err());
    }

    #[test]
    fn partial_smooth_scroll_keeps_other_keys() {
        let d = Settings::default().terminal.smooth_scroll;
        let only_easing = smooth(r#"{"easing": "linear"}"#);
        assert_eq!(only_easing.enable, d.enable);
        assert_eq!(only_easing.duration, d.duration);
        let only_enable = smooth(&format!(r#"{{"enable": {}}}"#, !d.enable));
        assert_eq!(only_enable.enable, !d.enable);
        assert_eq!(only_enable.duration, d.duration);
        assert_eq!(only_enable.easing, d.easing);
    }

    #[test]
    fn smooth_scroll_does_not_touch_scrollbar_settings() {
        let parsed = parse(r#"{"enable": false, "duration": 10, "easing": "linear"}"#).unwrap();
        assert_eq!(
            parsed.terminal.scrollbar,
            Settings::default().terminal.scrollbar
        );
    }

    #[test]
    fn active_needs_enable_and_duration() {
        assert!(smooth(r#"{"enable": true, "duration": 1}"#).active());
        assert!(!smooth(r#"{"enable": false, "duration": 500}"#).active());
        assert!(!smooth(r#"{"enable": true, "duration": 0}"#).active());
        assert!(!smooth(r#"{"enable": false, "duration": 0}"#).active());
    }

    #[test]
    fn easing_endpoints() {
        for e in ALL {
            assert_eq!(e.apply(0.), 0., "{e:?}");
            assert_eq!(e.apply(1.), 1., "{e:?}");
        }
    }

    #[test]
    fn easing_clamps_out_of_range_time() {
        for e in ALL {
            assert_eq!(e.apply(-0.5), 0., "{e:?}");
            assert_eq!(e.apply(-1e9), 0., "{e:?}");
            assert_eq!(e.apply(1.5), 1., "{e:?}");
            assert_eq!(e.apply(1e9), 1., "{e:?}");
        }
    }

    #[test]
    fn easing_stays_within_0_and_1() {
        for e in ALL {
            for step in 0..=1000 {
                let v = e.apply(step as f32 / 1000.);
                assert!((0. ..=1.).contains(&v), "{e:?} at {step}: {v}");
            }
        }
    }

    #[test]
    fn easing_never_goes_back() {
        for e in ALL {
            let mut last = e.apply(0.);
            for step in 1..=1000 {
                let v = e.apply(step as f32 / 1000.);
                assert!(v >= last, "{e:?} went back at {step}");
                last = v;
            }
        }
    }

    #[test]
    fn easing_is_continuous() {
        // no visible jumps between frames
        for e in ALL {
            for step in 0..1000 {
                let a = e.apply(step as f32 / 1000.);
                let b = e.apply((step + 1) as f32 / 1000.);
                assert!(b - a < 0.01, "{e:?} jumps at {step}: {a} -> {b}");
            }
        }
    }

    #[test]
    fn linear_is_identity() {
        for step in 0..=100 {
            let t = step as f32 / 100.;
            assert!((ScrollEasing::Linear.apply(t) - t).abs() < 1e-6, "{t}");
        }
    }

    #[test]
    fn ease_out_is_ahead_of_linear() {
        for step in 1..100 {
            let t = step as f32 / 100.;
            assert!(ScrollEasing::EaseOut.apply(t) > t, "{t}");
        }
    }

    #[test]
    fn ease_out_slows_down_at_the_end() {
        let e = ScrollEasing::EaseOut;
        let start = e.apply(0.1) - e.apply(0.);
        let end = e.apply(1.) - e.apply(0.9);
        assert!(start > end, "start {start} end {end}");
    }

    #[test]
    fn ease_in_out_soft_on_both_ends() {
        let e = ScrollEasing::EaseInOut;
        let start = e.apply(0.1) - e.apply(0.);
        let middle = e.apply(0.55) - e.apply(0.45);
        let end = e.apply(1.) - e.apply(0.9);
        assert!(start < middle && end < middle, "{start} {middle} {end}");
        assert!((e.apply(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn ease_in_out_is_symmetric() {
        let e = ScrollEasing::EaseInOut;
        for step in 0..=100 {
            let t = step as f32 / 100.;
            let sum = e.apply(t) + e.apply(1. - t);
            assert!((sum - 1.).abs() < 1e-5, "{t}: {sum}");
        }
    }
}
