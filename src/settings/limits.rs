//! numeric limits and other checks applied to parsed settings, a bad value falls back to its default

use alacritty_terminal::term::search::RegexSearch;

use super::*;

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

// Duration panics on negative or huge seconds, a day is already "never"
const NOTIFY_AFTER_RANGE: (f32, f32) = (0., 86400.);

const MAX_COMPLETION_ITEMS_RANGE: (f32, f32) = (1., 50.);

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

impl Settings {
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
        settings.autocomplete.max_items = limit(
            "autocomplete.max_items",
            settings.autocomplete.max_items as f32,
            MAX_COMPLETION_ITEMS_RANGE,
        )
        .map_or(defaults.autocomplete.max_items, |items| items as usize);
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
        letters.sort_unstable();
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

    /// keep a font size inside the allowed range
    pub fn clamp_font_size(size: f32) -> f32 {
        size.clamp(FONT_SIZE_RANGE.0, FONT_SIZE_RANGE.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn empty_profiles_use_bundled_ones() {
        assert_eq!(
            Settings::parse(r#"{"profiles": []}"#).unwrap().profiles,
            Settings::default().profiles
        );
    }

    #[test]
    fn invalid_regex_patterns_are_dropped() {
        // one bad pattern must not take the good ones with it
        let settings = Settings::parse(
            r#"{
                "links": {"patterns": [
                    {"regex": "(open", "action": "open"},
                    {"regex": "x+", "action": "copy"}
                ]},
                "hints": {"patterns": [{"name": "bad", "regex": "[", "action": "copy"}]}
            }"#,
        )
        .unwrap();
        let regexes: Vec<&str> = settings
            .links
            .patterns
            .iter()
            .map(|pattern| pattern.regex.as_str())
            .collect();
        assert_eq!(regexes, ["x+"]);
        assert!(settings.hints.patterns.is_empty());
    }

    #[test]
    fn unusable_hint_alphabets_use_the_default() {
        let alphabet = |json: &str| {
            Settings::parse(&format!(r#"{{"hints": {{"alphabet": {json}}}}}"#))
                .unwrap()
                .hints
                .alphabet
        };
        let default = Settings::default().hints.alphabet;
        // one letter, repeated letters anywhere, and invisible ones make no labels
        for bad in [
            r#""a""#,
            r#""aab""#,
            r#""aba""#,
            r#""a b""#,
            r#""a\tb""#,
            r#""""#,
        ] {
            assert_eq!(alphabet(bad), default, "{bad}");
        }
        assert_eq!(alphabet(r#""jk""#), "jk");
    }

    #[test]
    fn scrollback_pager_without_program_uses_the_default() {
        let default = Settings::default().scrollback_pager;
        for bad in ["[]", r#"[""]"#, r#"["", "-R"]"#] {
            let json = format!(r#"{{"scrollback_pager": {bad}}}"#);
            assert_eq!(Settings::parse(&json).unwrap().scrollback_pager, default);
        }
        let json = r#"{"scrollback_pager": ["most"]}"#;
        assert_eq!(Settings::parse(json).unwrap().scrollback_pager, ["most"]);
    }
}
