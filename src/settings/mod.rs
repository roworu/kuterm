//! user settings, read from a jsonc file

mod commands;
mod config;
mod keybindings;
mod options;
mod pins;
mod tab_icons;
mod window_size;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use alacritty_terminal::term::search::RegexSearch;
use gpui::{App, CursorHideMode, Global, Hsla};
use serde::Deserialize;
use serde_json_lenient::Value;

pub use commands::{Command, CommandAction, Commands};
#[cfg(test)]
pub(crate) use config::combine as config_combine;
pub(crate) use config::defaults as bundled_settings;
pub use config::{Config, ConfigFiles};
pub use keybindings::Keybindings;
pub use options::{
    ClipboardAccess, ConfirmQuit, CursorBlink, CursorShape, CursorUnfocused, Decorations,
    FontSlant, LineHeight, LinkModifier, NewTabButton, PatternAction, ProgramNotifications,
    RemoteControl, RestoreSession, ScrollEasing, ScrollbarEnable, ScrollbarPlacement, SearchCase,
    Shell, StartupMode, TabIconPosition, TabTitleAlign, TabTitleBlock, ThemeMode,
};
pub use pins::Pins;
pub use tab_icons::TabIcons;
pub use window_size::WindowSize;

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
    pub confirm_quit: ConfirmQuit,
    pub new_tab_button: NewTabButton,
    pub window_title: Vec<TabTitleBlock>,
    pub default_title: String,
    pub theme: ThemeSettings,
    pub window: WindowSettings,
    pub terminal: TerminalSettings,
    pub profiles: Vec<Profile>,
    pub command_palette: CommandPaletteSettings,
    pub notifications: NotificationSettings,
    /// hovering a pane focuses it
    pub focus_follows_mouse: bool,
    pub bell: BellSettings,
    pub search: SearchSettings,
    pub links: LinkSettings,
    pub hints: HintSettings,
    pub panes: PaneSettings,
    pub shell_integration: ShellIntegrationSettings,
    /// program and arguments the history is opened with, the file goes last
    pub scrollback_pager: Vec<String>,
    pub restore_session: RestoreSession,
    pub remote_control: RemoteControl,
    pub graphics: GraphicsSettings,
}

/// images programs show with kitty's graphics protocol
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct GraphicsSettings {
    pub enable: bool,
    /// megabytes of decoded images each tab keeps, the oldest go first
    pub max_memory_mb: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct ShellIntegrationSettings {
    /// load kuterm's integration scripts into bash, zsh and fish
    pub inject: bool,
    /// seconds a command runs before its end is notified while its tab is not seen,
    /// 0 never notifies
    pub notify_after: f32,
}

/// split panes
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct PaneSettings {
    /// pixels between two panes
    pub divider_width: f32,
    /// how much panes without focus are darkened, from 0 to 1
    pub dim_inactive: f32,
    /// a shell a split starts opens in the folder of the tab that was split
    pub split_inherits_cwd: bool,
}

/// urls and other text found by patterns, and osc 8 hyperlinks, opened with modifier and click
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct LinkSettings {
    pub modifier: LinkModifier,
    /// program links are opened with, the system opener when none
    pub open_with: Option<String>,
    pub patterns: Vec<TextPattern>,
}

/// text matched by a regex, with what to do with it
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TextPattern {
    #[serde(default)]
    pub name: Option<String>,
    pub regex: String,
    pub action: PatternAction,
}

/// labels over matches on screen, typing a label runs the action of its pattern
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct HintSettings {
    /// letters labels are made of
    pub alphabet: String,
    pub patterns: Vec<TextPattern>,
}

/// how the search bar starts out, its buttons change it for one search
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct SearchSettings {
    pub case_sensitive: SearchCase,
    pub regex: bool,
    pub whole_word: bool,
}

impl Default for Settings {
    fn default() -> Self {
        serde_json_lenient::from_value(config::defaults())
            .expect("bundled default settings are invalid")
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ThemeSettings {
    pub mode: ThemeMode,
    /// theme name from `themes` or a theme file path, bundled theme when none
    pub dark: Option<String>,
    /// theme name from `themes` or a theme file path, bundled theme when none
    pub light: Option<String>,
    /// theme colors by name, checked when a theme is loaded
    pub themes: BTreeMap<String, Value>,
}

impl ThemeSettings {
    /// these themes picked the way `profile` says
    pub fn with_profile(&self, profile: &ProfileTheme) -> Self {
        Self {
            mode: profile.mode,
            dark: profile.dark.clone(),
            light: profile.light.clone(),
            themes: self.themes.clone(),
        }
    }
}

/// theme of a profile, names come from `theme.themes`
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ProfileTheme {
    pub mode: ThemeMode,
    /// theme name or file path, bundled theme when none
    pub dark: Option<String>,
    /// theme name or file path, bundled theme when none
    pub light: Option<String>,
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
    /// show notifications programs send with osc 9 and osc 777
    pub from_programs: ProgramNotifications,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct BellSettings {
    pub visual: VisualBell,
    /// play the system bell sound
    pub audio: bool,
    /// ask the desktop for attention while the window is not focused
    pub urgent: bool,
    /// mark a background tab that rang until it is shown
    pub tab_indicator: bool,
}

/// flash over the terminal that fades out
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct VisualBell {
    pub enable: bool,
    /// flash color, a faint terminal foreground when none
    pub color: Option<Hsla>,
    /// milliseconds the flash takes to fade out
    pub duration: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct WindowSettings {
    /// pixels between the window edges and the terminal grid
    pub padding: WindowPadding,
    /// split the space left over by whole cells between both sides
    pub padding_balance: bool,
    /// terminal background opacity, from 0 to 1
    pub opacity: f32,
    /// blur what is behind a see through background
    pub blur: bool,
    pub decorations: Decorations,
    /// grid size the window opens with
    pub columns: u32,
    pub rows: u32,
    pub startup: StartupMode,
    /// open with the size the window had when it was last resized
    pub remember_size: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct WindowPadding {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TabIconSettings {
    pub position: TabIconPosition,
    pub dynamic: bool,
    pub default: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TerminalSettings {
    pub font: FontSettings,
    /// opentype features like "-calt" or "ss01", checked when parsed
    pub font_features: Vec<String>,
    /// families tried in order for glyphs the font lacks, before the system fallback
    pub font_fallbacks: Vec<String>,
    /// pixels added to the measured cell width
    pub cell_width: f32,
    /// pixels glyphs are moved by inside their cell
    pub glyph_offset: GlyphOffset,
    /// bold text in colors 0-7 uses colors 8-15
    pub bold_is_bright: bool,
    /// wcag contrast ratio text keeps against its background, 1 turns it off
    pub minimum_contrast: f32,
    /// alpha of faint text that has no dim theme color
    pub dim_opacity: f32,
    /// draw box drawing, block and powerline glyphs as shapes that fill the cell
    pub builtin_box_drawing: bool,
    pub line_height: LineHeight,
    pub cursor_shape: CursorShape,
    pub cursor: CursorSettings,
    pub cursor_blink: CursorBlink,
    /// milliseconds the blinking cursor stays shown, then hidden
    pub cursor_blink_interval: f32,
    /// seconds without typing after which the cursor stops blinking, 0 blinks forever
    pub cursor_blink_timeout: f32,
    pub max_history_length: usize,
    /// option key sends esc prefixed keys like alt does on linux, macos only
    pub option_as_meta: bool,
    /// copy the selection into the clipboard when the mouse button is released
    pub select_to_copy: bool,
    /// right click copies the selection into the clipboard
    pub right_click_copy: bool,
    /// drop spaces and tabs at the end of each copied line
    pub copy_trim_trailing_whitespace: bool,
    /// characters that end a word for double click selection
    pub word_separators: String,
    /// lines scrolled per wheel step, and touchpad speed
    pub scroll_multiplier: f32,
    /// hide the mouse pointer on key presses until it moves
    pub hide_mouse_when_typing: bool,
    pub scrollbar: ScrollbarSettings,
    pub smooth_scroll: SmoothScrollSettings,
    pub clipboard: ClipboardSettings,
    /// ask before pasting text that would run right away or has control characters
    pub paste_protection: bool,
}

/// what programs may do with the clipboard through osc 52
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct ClipboardSettings {
    pub write: ClipboardAccess,
    pub read: ClipboardAccess,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct FontSettings {
    pub family: String,
    pub size: f32,
    pub bold: BoldFont,
    pub italic: ItalicFont,
    pub bold_italic: BoldItalicFont,
}

/// face for bold text, regular family when none
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct BoldFont {
    pub family: Option<String>,
    pub weight: f32,
}

/// face for italic text, regular family when none
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ItalicFont {
    pub family: Option<String>,
    pub style: FontSlant,
}

/// face for bold italic text, regular family with bold weight and italic style when none
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct BoldItalicFont {
    pub family: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct CursorSettings {
    /// cursor color, theme cursor color when none
    pub color: Option<Hsla>,
    /// color of the glyph under a block cursor, terminal background when none
    pub text_color: Option<Hsla>,
    /// pixels a bar or underline cursor is thick
    pub thickness: f32,
    pub unfocused: CursorUnfocused,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct GlyphOffset {
    pub x: f32,
    pub y: f32,
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

impl TerminalSettings {
    /// when gpui hides the mouse pointer on key presses
    pub fn cursor_hide_mode(&self) -> CursorHideMode {
        if self.hide_mouse_when_typing {
            CursorHideMode::OnTypingAndAction
        } else {
            CursorHideMode::Never
        }
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
    pub theme: Option<ProfileTheme>,
    pub icon: Option<String>,
    /// extra environment variables for the command
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl Global for Settings {}

// smaller fonts break the terminal grid, bigger ones stop growing
const FONT_SIZE_RANGE: (f32, f32) = (6., 72.);
// css font weights, the font picks its nearest face
const FONT_WEIGHT_RANGE: (f32, f32) = (100., 900.);
// a few pixels fix a font, more breaks the grid
const CELL_ADJUST_RANGE: (f32, f32) = (-10., 10.);
// 1 is no contrast at all, 21 is black on white
const MINIMUM_CONTRAST_RANGE: (f32, f32) = (1., 21.);
const DIM_OPACITY_RANGE: (f32, f32) = (0., 1.);
// lines below 1 overlap, above 3 waste the screen
const LINE_HEIGHT_RANGE: (f32, f32) = (1., 3.);
// thinner bars are hard to grab, wider ones eat the grid
const SCROLLBAR_WIDTH_RANGE: (f32, f32) = (2., 64.);
// Duration panics on negative or huge seconds, an hour is already "never"
const AUTO_HIDE_RANGE: (f32, f32) = (0., 3600.);
// faster blinking flickers, slower looks like a hang
const CURSOR_BLINK_INTERVAL_RANGE: (f32, f32) = (100., 2000.);
// Duration panics on negative or huge seconds, an hour is already "never"
const CURSOR_BLINK_TIMEOUT_RANGE: (f32, f32) = (0., 3600.);
// thinner is invisible, thicker looks like a block
const CURSOR_THICKNESS_RANGE: (f32, f32) = (1., 8.);
// slower scrolling is useless, faster skips whole screens
const SCROLL_MULTIPLIER_RANGE: (f32, f32) = (0.1, 20.);
// Duration panics on negative seconds, a longer flash hides the output
const BELL_DURATION_RANGE: (f32, f32) = (0., 2000.);
// longer glides feel like lag, not smoothness
const SMOOTH_SCROLL_DURATION_RANGE: (f32, f32) = (0., 1000.);

pub const MAX_HISTORY_LENGTH: usize = u32::MAX as usize;

// more padding than this leaves no room for the grid
const PADDING_RANGE: (f32, f32) = (0., 200.);
const OPACITY_RANGE: (f32, f32) = (0., 1.);
// a window smaller than this is useless, bigger ones don't fit any screen
const COLUMNS_RANGE: (f32, f32) = (10., 1000.);
const ROWS_RANGE: (f32, f32) = (2., 500.);

// a wider divider wastes room, 0 still leaves a grab handle
const DIVIDER_WIDTH_RANGE: (f32, f32) = (0., 20.);
const DIM_INACTIVE_RANGE: (f32, f32) = (0., 1.);
// a few megabytes hold some images, more than this is a lot for one tab
const GRAPHICS_MEMORY_RANGE: (f32, f32) = (1., 4096.);
// Duration panics on negative or huge seconds, a day is already "never"
const NOTIFY_AFTER_RANGE: (f32, f32) = (0., 86400.);

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

/// `$XDG_CONFIG_HOME/kuterm`, falling back to `~/.config/kuterm`
pub fn config_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_STATE_HOME/kuterm`, falling back to `~/.local/state/kuterm`, for files the app writes
pub fn state_dir() -> Option<PathBuf> {
    xdg_dir("XDG_STATE_HOME", ".local/state")
}

fn xdg_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    // xdg says empty or relative values must be ignored
    let dir = std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback)))?;
    Some(dir.join("kuterm"))
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
    #[cfg(test)]
    pub fn parse(json: &str) -> serde_json_lenient::Result<Self> {
        Self::from_full(config::combine(serde_json_lenient::from_str(json)?))
    }

    /// settings from user values already merged over the bundled ones, keeping values in limits
    pub fn from_full(full: Value) -> serde_json_lenient::Result<Self> {
        let mut settings: Self = serde_json_lenient::from_value(full)?;
        let defaults = Self::default();
        settings.ui_font_size = limit("ui_font_size", settings.ui_font_size, FONT_SIZE_RANGE)
            .unwrap_or(defaults.ui_font_size);
        let terminal = &mut settings.terminal;
        let font = &mut terminal.font;
        font.size = limit("terminal.font.size", font.size, FONT_SIZE_RANGE)
            .unwrap_or(defaults.terminal.font.size);
        font.bold.weight = limit(
            "terminal.font.bold.weight",
            font.bold.weight,
            FONT_WEIGHT_RANGE,
        )
        .unwrap_or(defaults.terminal.font.bold.weight);
        terminal.font_features.retain(|feature| {
            let valid = parse_font_feature(feature).is_some();
            if !valid {
                eprintln!("terminal.font_features {feature:?} is not a feature like \"-calt\", \"ss01\" or \"cv01=2\", skipped");
            }
            valid
        });
        terminal.cell_width = limit(
            "terminal.cell_width",
            terminal.cell_width,
            CELL_ADJUST_RANGE,
        )
        .unwrap_or(defaults.terminal.cell_width);
        let offset = &mut terminal.glyph_offset;
        offset.x = limit("terminal.glyph_offset.x", offset.x, CELL_ADJUST_RANGE)
            .unwrap_or(defaults.terminal.glyph_offset.x);
        offset.y = limit("terminal.glyph_offset.y", offset.y, CELL_ADJUST_RANGE)
            .unwrap_or(defaults.terminal.glyph_offset.y);
        terminal.minimum_contrast = limit(
            "terminal.minimum_contrast",
            terminal.minimum_contrast,
            MINIMUM_CONTRAST_RANGE,
        )
        .unwrap_or(defaults.terminal.minimum_contrast);
        terminal.dim_opacity = limit(
            "terminal.dim_opacity",
            terminal.dim_opacity,
            DIM_OPACITY_RANGE,
        )
        .unwrap_or(defaults.terminal.dim_opacity);
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
        terminal.cursor_blink_timeout = limit(
            "terminal.cursor_blink_timeout",
            terminal.cursor_blink_timeout,
            CURSOR_BLINK_TIMEOUT_RANGE,
        )
        .unwrap_or(defaults.terminal.cursor_blink_timeout);
        terminal.cursor.thickness = limit(
            "terminal.cursor.thickness",
            terminal.cursor.thickness,
            CURSOR_THICKNESS_RANGE,
        )
        .unwrap_or(defaults.terminal.cursor.thickness);
        terminal.scroll_multiplier = limit(
            "terminal.scroll_multiplier",
            terminal.scroll_multiplier,
            SCROLL_MULTIPLIER_RANGE,
        )
        .unwrap_or(defaults.terminal.scroll_multiplier);
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
        settings.bell.visual.duration = limit(
            "bell.visual.duration",
            settings.bell.visual.duration,
            BELL_DURATION_RANGE,
        )
        .unwrap_or(defaults.bell.visual.duration);
        let panes = &mut settings.panes;
        panes.divider_width = limit(
            "panes.divider_width",
            panes.divider_width,
            DIVIDER_WIDTH_RANGE,
        )
        .unwrap_or(defaults.panes.divider_width);
        panes.dim_inactive = limit("panes.dim_inactive", panes.dim_inactive, DIM_INACTIVE_RANGE)
            .unwrap_or(defaults.panes.dim_inactive);
        settings.shell_integration.notify_after = limit(
            "shell_integration.notify_after",
            settings.shell_integration.notify_after,
            NOTIFY_AFTER_RANGE,
        )
        .unwrap_or(defaults.shell_integration.notify_after);
        settings.graphics.max_memory_mb = limit(
            "graphics.max_memory_mb",
            settings.graphics.max_memory_mb as f32,
            GRAPHICS_MEMORY_RANGE,
        )
        .map_or(defaults.graphics.max_memory_mb, |mb| mb as usize);
        let window = &mut settings.window;
        window.padding.x = limit("window.padding.x", window.padding.x, PADDING_RANGE)
            .unwrap_or(defaults.window.padding.x);
        window.padding.y = limit("window.padding.y", window.padding.y, PADDING_RANGE)
            .unwrap_or(defaults.window.padding.y);
        window.opacity = limit("window.opacity", window.opacity, OPACITY_RANGE)
            .unwrap_or(defaults.window.opacity);
        window.columns = limit("window.columns", window.columns as f32, COLUMNS_RANGE)
            .map_or(defaults.window.columns, |columns| columns as u32);
        window.rows = limit("window.rows", window.rows as f32, ROWS_RANGE)
            .map_or(defaults.window.rows, |rows| rows as u32);
        for (name, patterns) in [
            ("links.patterns", &mut settings.links.patterns),
            ("hints.patterns", &mut settings.hints.patterns),
        ] {
            patterns.retain(|pattern| {
                let error = RegexSearch::new(&pattern.regex).err();
                if let Some(error) = &error {
                    eprintln!(
                        "{name} {:?} is not a valid regex, skipped: {error}",
                        pattern.regex
                    );
                }
                error.is_none()
            });
        }
        let alphabet = &mut settings.hints.alphabet;
        let mut letters: Vec<char> = alphabet.chars().collect();
        letters.dedup();
        // labels need at least two different letters, and spaces would be invisible
        if letters.len() != alphabet.chars().count()
            || letters.len() < 2
            || letters.iter().any(|c| c.is_whitespace() || c.is_control())
        {
            eprintln!(
                "hints.alphabet {alphabet:?} needs two or more different letters, using the default"
            );
            *alphabet = defaults.hints.alphabet;
        }
        if settings
            .scrollback_pager
            .first()
            .is_none_or(|program| program.is_empty())
        {
            eprintln!("scrollback_pager needs a program, using the default");
            settings.scrollback_pager = defaults.scrollback_pager;
        }
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
                "terminal.font.family",
                &mut self.terminal.font.family,
                defaults.terminal.font.family,
            ),
        ] {
            if !installed.contains(family) {
                eprintln!("{name} {family:?} is not installed, using {default:?}");
                *family = default;
            }
        }
        let font = &mut self.terminal.font;
        for (name, family) in [
            ("terminal.font.bold.family", &mut font.bold.family),
            ("terminal.font.italic.family", &mut font.italic.family),
            (
                "terminal.font.bold_italic.family",
                &mut font.bold_italic.family,
            ),
        ] {
            if let Some(missing) = family.take_if(|family| !installed.contains(family)) {
                eprintln!("{name} {missing:?} is not installed, using the regular family");
            }
        }
        self.terminal.font_fallbacks.retain(|family| {
            let found = installed.contains(family);
            if !found {
                eprintln!("terminal.font_fallbacks {family:?} is not installed, skipped");
            }
            found
        });
    }

    /// profile opened by the new tab action
    pub fn default_profile(&self) -> &Profile {
        self.profiles
            .iter()
            .find(|profile| profile.default)
            .unwrap_or(&self.profiles[0])
    }

    /// profile with this name, none when there is none
    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.name == name)
    }

    /// keep a font size inside the allowed range
    pub fn clamp_font_size(size: f32) -> f32 {
        size.clamp(FONT_SIZE_RANGE.0, FONT_SIZE_RANGE.1)
    }

    /// settings loaded at startup or on the last reload
    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}

/// opentype tag and value of a feature like "-calt", "+calt", "ss01" or "cv01=2"
pub fn parse_font_feature(feature: &str) -> Option<(String, u32)> {
    let (tag, value) = if let Some(tag) = feature.strip_prefix('-') {
        (tag, 0)
    } else if let Some((tag, value)) = feature.split_once('=') {
        (tag, value.parse().ok()?)
    } else {
        (feature.strip_prefix('+').unwrap_or(feature), 1)
    };
    // opentype tags are exactly four printable ascii chars
    (tag.len() == 4 && tag.bytes().all(|b| b.is_ascii_graphic())).then(|| (tag.to_string(), value))
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

    /// run `f` with XDG_CONFIG_HOME pointing at `dir` and XDG_STATE_HOME at `dir/state`,
    /// restoring them afterwards
    pub(crate) fn with_config_home(dir: &Path, f: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let vars = [
            ("XDG_CONFIG_HOME", dir.to_path_buf()),
            ("XDG_STATE_HOME", dir.join("state")),
        ];
        let old: Vec<_> = vars
            .iter()
            .map(|(name, _)| std::env::var_os(name))
            .collect();
        for (name, value) in &vars {
            unsafe { std::env::set_var(name, value) };
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        for ((name, _), old) in vars.iter().zip(old) {
            unsafe {
                match old {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
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
            settings.terminal.font.family,
            "JetBrainsMonoNL Nerd Font Mono"
        );
        assert_eq!(settings.terminal.font.size, 16.0);
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
        assert_eq!(settings.terminal.font.size, defaults.terminal.font.size);
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
        expected.terminal.font.size = 14.;
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
        assert_eq!(settings.theme.dark, Some("themes/d.jsonc".into()));
        assert_eq!(settings.theme.light, Settings::default().theme.light);

        let settings = Settings::parse(
            r#"{"theme": {"mode": "dark", "dark": null, "light": "/abs/l.jsonc"}}"#,
        )
        .unwrap();
        assert_eq!(settings.theme.mode, ThemeMode::Dark);
        assert_eq!(settings.theme.dark, None);
        assert_eq!(settings.theme.light, Some("/abs/l.jsonc".into()));
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

            let load = || Config::load().config.map(|config| config.settings);
            // missing file is created from the bundled one
            assert_eq!(load(), Ok(Settings::default()));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_SETTINGS);

            std::fs::write(&path, r#"{"theme": {"mode": "light", "light": "x.jsonc"}}"#).unwrap();
            let settings = load().unwrap();
            assert_eq!(settings.theme.mode, ThemeMode::Light);
            assert_eq!(settings.theme.light, Some("x.jsonc".into()));

            // broken file is an error and is left untouched
            let broken = r#"{"theme": {"mode": "auto"}}"#;
            std::fs::write(&path, broken).unwrap();
            assert!(load().is_err());
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
            _ => settings.terminal.font.size,
        }
    }

    #[test]
    fn font_size_below_min_uses_default() {
        let defaults = Settings::default();
        for field in ["ui_font_size", "terminal.font.size"] {
            for value in [
                "0", "0.0", "-0", "-0.0", "1", "0.5", "1e-30", "5", "-1", "-6", "-16", "-72",
                "-100", "-1e30", "-3.4e38",
            ] {
                let got = parse_font(field, value);
                let default = match field {
                    "ui_font_size" => defaults.ui_font_size,
                    _ => defaults.terminal.font.size,
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
        for field in ["ui_font_size", "terminal.font.size"] {
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
        assert_eq!(b.working_directory, Some("~/src".into()));
        let theme = b.theme.as_ref().unwrap();
        assert_eq!(theme.mode, ThemeMode::Dark);
        assert_eq!(theme.dark, Some("themes/b.jsonc".into()));
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
    fn cursor_blink_timeout_limits() {
        let timeout = |value: &str| {
            let json = format!(r#"{{"terminal": {{"cursor_blink_timeout": {value}}}}}"#);
            Settings::parse(&json)
                .unwrap()
                .terminal
                .cursor_blink_timeout
        };
        let default = Settings::default().terminal.cursor_blink_timeout;
        assert_eq!(timeout("-1"), default);
        assert_eq!(timeout("0"), 0.);
        assert_eq!(timeout("2.5"), 2.5);
        assert_eq!(timeout("3600"), 3600.);
        assert_eq!(timeout("1e30"), 3600.);
    }

    #[test]
    fn parses_cursor_settings_and_thickness_limits() {
        let cursor = |json: &str| Settings::parse(json).unwrap().terminal.cursor;
        let parsed = cursor(
            r##"{"terminal": {"cursor": {"color": "#ff0000", "text_color": "#00ff00", "thickness": 3, "unfocused": "none"}}}"##,
        );
        assert_eq!(parsed.color, Some(gpui::rgb(0xff0000).into()));
        assert_eq!(parsed.text_color, Some(gpui::rgb(0x00ff00).into()));
        assert_eq!(parsed.thickness, 3.);
        assert_eq!(parsed.unfocused, CursorUnfocused::None);
        let unfocused = |value: &str| {
            cursor(&format!(
                r#"{{"terminal": {{"cursor": {{"unfocused": "{value}"}}}}}}"#
            ))
            .unfocused
        };
        assert_eq!(unfocused("hollow"), CursorUnfocused::Hollow);
        assert_eq!(unfocused("same"), CursorUnfocused::Same);
        // null colors fall back to the theme
        let parsed = cursor(r#"{"terminal": {"cursor": {"color": null, "text_color": null}}}"#);
        assert_eq!((parsed.color, parsed.text_color), (None, None));

        let thickness = |value: &str| {
            cursor(&format!(
                r#"{{"terminal": {{"cursor": {{"thickness": {value}}}}}}}"#
            ))
            .thickness
        };
        let default = Settings::default().terminal.cursor.thickness;
        assert_eq!(thickness("0"), default);
        assert_eq!(thickness("-2"), default);
        assert_eq!(thickness("1"), 1.);
        assert_eq!(thickness("8"), 8.);
        assert_eq!(thickness("20"), 8.);

        for json in [
            r#"{"terminal": {"cursor": {"unfocused": "dim"}}}"#,
            r#"{"terminal": {"cursor": {"color": "red"}}}"#,
            r#"{"terminal": {"cursor": {"color": 5}}}"#,
            r#"{"terminal": {"cursor": "bar"}}"#,
        ] {
            assert!(
                Settings::parse(json).is_err(),
                "expected error for {json:?}"
            );
        }
    }

    #[test]
    fn mouse_settings_parse_and_scroll_multiplier_limits() {
        let terminal = |json: &str| Settings::parse(json).unwrap().terminal;
        let parsed = terminal(
            r#"{"terminal": {"hide_mouse_when_typing": false, "word_separators": " -", "copy_trim_trailing_whitespace": false}}"#,
        );
        assert!(!parsed.hide_mouse_when_typing);
        assert_eq!(parsed.cursor_hide_mode(), CursorHideMode::Never);
        assert_eq!(parsed.word_separators, " -");
        assert!(!parsed.copy_trim_trailing_whitespace);
        let parsed = terminal(r#"{"terminal": {"hide_mouse_when_typing": true}}"#);
        assert_eq!(parsed.cursor_hide_mode(), CursorHideMode::OnTypingAndAction);
        assert!(
            Settings::parse(r#"{"focus_follows_mouse": true}"#)
                .unwrap()
                .focus_follows_mouse
        );

        let multiplier = |value: &str| {
            terminal(&format!(
                r#"{{"terminal": {{"scroll_multiplier": {value}}}}}"#
            ))
            .scroll_multiplier
        };
        let default = Settings::default().terminal.scroll_multiplier;
        assert_eq!(multiplier("0"), default);
        assert_eq!(multiplier("-1"), default);
        assert_eq!(multiplier("0.1"), 0.1);
        assert_eq!(multiplier("5.5"), 5.5);
        assert_eq!(multiplier("100"), 20.);
    }

    #[test]
    fn parses_bell_settings_and_duration_limits() {
        let bell = |json: &str| Settings::parse(json).unwrap().bell;
        let parsed = bell(
            r##"{"bell": {"visual": {"enable": false, "color": "#ffffff40", "duration": 300}, "audio": true, "urgent": false, "tab_indicator": false}}"##,
        );
        assert_eq!(
            parsed,
            BellSettings {
                visual: VisualBell {
                    enable: false,
                    color: Some(gpui::rgba(0xffffff40).into()),
                    duration: 300.,
                },
                audio: true,
                urgent: false,
                tab_indicator: false,
            }
        );
        let duration = |value: &str| {
            bell(&format!(
                r#"{{"bell": {{"visual": {{"duration": {value}}}}}}}"#
            ))
            .visual
            .duration
        };
        let default = Settings::default().bell.visual.duration;
        assert_eq!(duration("-1"), default);
        assert_eq!(duration("0"), 0.);
        assert_eq!(duration("2000"), 2000.);
        assert_eq!(duration("1e9"), 2000.);
        assert!(Settings::parse(r#"{"bell": {"audio": "loud"}}"#).is_err());
        assert!(Settings::parse(r#"{"bell": true}"#).is_err());
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
        assert_eq!(settings.terminal.font.family, "DejaVu Sans Mono");

        settings.use_installed_fonts(&[]);
        assert_eq!(settings.terminal.font.family, defaults.terminal.font.family);
    }

    #[test]
    fn flat_font_keys_move_into_font() {
        let settings =
            Settings::parse(r#"{"terminal": {"font_family": "Agent Mono", "font_size": 15}}"#)
                .unwrap();
        assert_eq!(settings.terminal.font.family, "Agent Mono");
        assert_eq!(settings.terminal.font.size, 15.);

        // the new keys win over the old ones
        let settings = Settings::parse(
            r#"{"terminal": {"font_size": 15, "font": {"size": 17, "bold": {"weight": 600}}}}"#,
        )
        .unwrap();
        assert_eq!(settings.terminal.font.size, 17.);
        assert_eq!(settings.terminal.font.bold.weight, 600.);
    }

    #[test]
    fn parses_font_styles() {
        let settings = Settings::parse(
            r#"{"terminal": {"font": {
                "bold": {"family": "Agent Bold", "weight": 800},
                "italic": {"family": null, "style": "oblique"},
                "bold_italic": {"family": "Agent Bold Italic"}
            }}}"#,
        )
        .unwrap();
        let font = &settings.terminal.font;
        assert_eq!(font.bold.family.as_deref(), Some("Agent Bold"));
        assert_eq!(font.bold.weight, 800.);
        assert_eq!(font.italic.family, None);
        assert_eq!(font.italic.style, FontSlant::Oblique);
        assert_eq!(
            font.bold_italic.family.as_deref(),
            Some("Agent Bold Italic")
        );
        assert!(Settings::parse(r#"{"terminal": {"font": {"italic": {"style": "x"}}}}"#).is_err());
    }

    #[test]
    fn font_and_color_rule_limits() {
        let defaults = Settings::default().terminal;
        let terminal = |json: &str| {
            Settings::parse(&format!(r#"{{"terminal": {json}}}"#))
                .unwrap()
                .terminal
        };
        let bold = |weight: &str| {
            terminal(&format!(
                r#"{{"font": {{"bold": {{"weight": {weight}}}}}}}"#
            ))
            .font
            .bold
            .weight
        };
        assert_eq!(bold("99"), defaults.font.bold.weight);
        assert_eq!(bold("100"), 100.);
        assert_eq!(bold("950"), 900.);

        let cell = |value: &str| terminal(&format!(r#"{{"cell_width": {value}}}"#)).cell_width;
        assert_eq!(cell("-11"), defaults.cell_width);
        assert_eq!(cell("-10"), -10.);
        assert_eq!(cell("3.5"), 3.5);
        assert_eq!(cell("11"), 10.);

        let offset = terminal(r#"{"glyph_offset": {"x": -20, "y": 20}}"#).glyph_offset;
        assert_eq!(offset.x, defaults.glyph_offset.x);
        assert_eq!(offset.y, 10.);

        let contrast =
            |value: &str| terminal(&format!(r#"{{"minimum_contrast": {value}}}"#)).minimum_contrast;
        assert_eq!(contrast("0.5"), defaults.minimum_contrast);
        assert_eq!(contrast("4.5"), 4.5);
        assert_eq!(contrast("30"), 21.);

        let dim = |value: &str| terminal(&format!(r#"{{"dim_opacity": {value}}}"#)).dim_opacity;
        assert_eq!(dim("-0.1"), defaults.dim_opacity);
        assert_eq!(dim("0.4"), 0.4);
        assert_eq!(dim("2"), 1.);
    }

    #[test]
    fn font_features_are_checked() {
        assert_eq!(parse_font_feature("-calt"), Some(("calt".into(), 0)));
        assert_eq!(parse_font_feature("+liga"), Some(("liga".into(), 1)));
        assert_eq!(parse_font_feature("ss01"), Some(("ss01".into(), 1)));
        assert_eq!(parse_font_feature("cv01=2"), Some(("cv01".into(), 2)));
        for bad in [
            "",
            "-",
            "cal",
            "ligatures",
            "cv01=x",
            "cv01=-1",
            "ss 1",
            "-+calt",
        ] {
            assert_eq!(parse_font_feature(bad), None, "{bad:?}");
        }
        let settings = Settings::parse(
            r#"{"terminal": {"font_features": ["-calt", "bad feature", "ss01", "cv01=2"]}}"#,
        )
        .unwrap();
        assert_eq!(settings.terminal.font_features, ["-calt", "ss01", "cv01=2"]);
    }

    #[test]
    fn missing_style_families_and_fallbacks_are_dropped() {
        let mut settings = Settings::parse(
            r#"{"terminal": {
                "font": {"bold": {"family": "Agent Bold"}, "italic": {"family": "Agent Gone"}},
                "font_fallbacks": ["Agent Gone", "Agent Emoji"]
            }}"#,
        )
        .unwrap();
        let installed = [
            Settings::default().terminal.font.family,
            "Agent Bold".to_string(),
            "Agent Emoji".to_string(),
        ];
        settings.use_installed_fonts(&installed);
        let terminal = &settings.terminal;
        assert_eq!(terminal.font.bold.family.as_deref(), Some("Agent Bold"));
        assert_eq!(terminal.font.italic.family, None);
        assert_eq!(terminal.font_fallbacks, ["Agent Emoji"]);
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
mod window_settings {
    use super::*;

    fn window(json: &str) -> WindowSettings {
        Settings::parse(&format!(r#"{{"window": {json}}}"#))
            .unwrap()
            .window
    }

    #[test]
    fn bundled_settings_have_window_keys() {
        let full = config::defaults();
        let window = full["window"].as_object().expect("no window section");
        for key in [
            "padding",
            "padding_balance",
            "opacity",
            "blur",
            "decorations",
            "columns",
            "rows",
            "startup",
            "remember_size",
        ] {
            assert!(window.contains_key(key), "window.{key} is missing");
        }
        assert!(full["confirm_quit"].is_string());
        // bundled values are already within limits
        assert_eq!(
            Settings::default().window,
            Settings::from_full(full).unwrap().window
        );
    }

    #[test]
    fn parses_window_values() {
        let w = window(
            r#"{"padding": {"x": 12, "y": 0}, "padding_balance": true, "opacity": 0.5,
                "blur": true, "decorations": "client", "columns": 120, "rows": 40,
                "startup": "maximized", "remember_size": true}"#,
        );
        assert_eq!(w.padding, WindowPadding { x: 12., y: 0. });
        assert!(w.padding_balance && w.blur && w.remember_size);
        assert_eq!(w.opacity, 0.5);
        assert_eq!(w.decorations, Decorations::Client);
        assert_eq!((w.columns, w.rows), (120, 40));
        assert_eq!(w.startup, StartupMode::Maximized);
        for (name, mode) in [
            ("full", Decorations::Full),
            ("none", Decorations::None),
            ("client", Decorations::Client),
        ] {
            assert_eq!(
                window(&format!(r#"{{"decorations": "{name}"}}"#)).decorations,
                mode
            );
        }
        for (name, mode) in [
            ("windowed", StartupMode::Windowed),
            ("fullscreen", StartupMode::Fullscreen),
        ] {
            assert_eq!(window(&format!(r#"{{"startup": "{name}"}}"#)).startup, mode);
        }
        for (name, mode) in [
            ("always", ConfirmQuit::Always),
            ("running", ConfirmQuit::Running),
            ("never", ConfirmQuit::Never),
        ] {
            let json = format!(r#"{{"confirm_quit": "{name}"}}"#);
            assert_eq!(Settings::parse(&json).unwrap().confirm_quit, mode);
        }
    }

    #[test]
    fn partial_window_section_keeps_other_defaults() {
        let defaults = Settings::default().window;
        let w = window(r#"{"padding": {"x": 9}}"#);
        assert_eq!(w.padding.x, 9.);
        assert_eq!(w.padding.y, defaults.padding.y);
        assert_eq!(w.opacity, defaults.opacity);
        assert_eq!(w.decorations, defaults.decorations);
    }

    #[test]
    fn window_limits() {
        let defaults = Settings::default().window;
        assert_eq!(
            window(r#"{"padding": {"x": 0, "y": 200}}"#).padding,
            WindowPadding { x: 0., y: 200. }
        );
        assert_eq!(
            window(r#"{"padding": {"x": 500, "y": -1}}"#).padding,
            WindowPadding {
                x: 200.,
                y: defaults.padding.y
            }
        );
        assert_eq!(window(r#"{"opacity": 0}"#).opacity, 0.);
        assert_eq!(window(r#"{"opacity": 3}"#).opacity, 1.);
        assert_eq!(window(r#"{"opacity": -0.5}"#).opacity, defaults.opacity);
        assert_eq!(window(r#"{"columns": 10, "rows": 2}"#).columns, 10);
        assert_eq!(window(r#"{"columns": 5000, "rows": 9000}"#).rows, 500);
        assert_eq!(window(r#"{"columns": 5000}"#).columns, 1000);
        let small = window(r#"{"columns": 3, "rows": 1}"#);
        assert_eq!(
            (small.columns, small.rows),
            (defaults.columns, defaults.rows)
        );
    }

    #[test]
    fn rejects_bad_window_values() {
        for json in [
            r#"{"window": {"decorations": "fancy"}}"#,
            r#"{"window": {"decorations": true}}"#,
            r#"{"window": {"startup": "minimized"}}"#,
            r#"{"window": {"columns": -1}}"#,
            r#"{"window": {"padding": 4}}"#,
            r#"{"window": {"blur": "yes"}}"#,
            r#"{"confirm_quit": true}"#,
            r#"{"confirm_quit": "sometimes"}"#,
        ] {
            assert!(Settings::parse(json).is_err(), "{json}");
        }
    }
}

#[cfg(test)]
mod clipboard_settings {
    use super::{ClipboardAccess, Settings};

    #[test]
    fn clipboard_access_and_paste_protection_parse() {
        let settings = Settings::parse(
            r#"{"terminal": {"clipboard": {"write": "ask", "read": "allow"}, "paste_protection": false}}"#,
        )
        .unwrap();
        assert_eq!(settings.terminal.clipboard.write, ClipboardAccess::Ask);
        assert_eq!(settings.terminal.clipboard.read, ClipboardAccess::Allow);
        assert!(!settings.terminal.paste_protection);
        // a partial block keeps the other value
        let settings = Settings::parse(r#"{"terminal": {"clipboard": {"read": "deny"}}}"#).unwrap();
        assert_eq!(settings.terminal.clipboard.read, ClipboardAccess::Deny);
        assert_eq!(
            settings.terminal.clipboard.write,
            Settings::default().terminal.clipboard.write
        );
        assert!(Settings::parse(r#"{"terminal": {"clipboard": {"read": "maybe"}}}"#).is_err());
    }
}
