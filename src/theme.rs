//! theme colors, picked by name from the "theme" block of settings or read from files

use std::path::{Path, PathBuf};

use alacritty_terminal::vte::ansi::{Color, NamedColor};
use gpui::{App, Global, Hsla, WindowAppearance, rgb};
use serde::Deserialize;
use serde_json_lenient::Value;

use crate::settings::{Settings, ThemeMode, ThemeSettings, bundled_settings, merge};

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Theme {
    pub tab_bar_background: Hsla,
    pub tab_active_background: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub terminal_background: Hsla,
    pub terminal_foreground: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
    pub scrollbar: Hsla,
    pub search_match: Hsla,
    pub search_match_active: Hsla,
    pub danger_button: Hsla,
    pub danger_button_text: Hsla,
    /// 8 normal colors followed by 8 bright colors
    pub ansi: [Hsla; 16],
    pub ansi_dim: [Hsla; 8],
    pub bright_foreground: Hsla,
    pub dim_foreground: Hsla,
}

impl Default for Theme {
    fn default() -> Self {
        Self::bundled(true)
    }
}

impl Global for Theme {}

impl Theme {
    // colors of the bundled theme for dark or light mode, they live in the bundled settings
    fn bundled_value(dark: bool) -> Value {
        let name = if dark { "dark" } else { "light" };
        bundled_settings()["theme"]["themes"][name].clone()
    }

    /// bundled dark or light theme
    pub fn bundled(dark: bool) -> Self {
        serde_json_lenient::from_value(Self::bundled_value(dark)).expect("bundled theme is invalid")
    }

    /// parse theme, missing colors come from the bundled theme of the same mode
    pub fn parse(json: &str, dark: bool) -> serde_json_lenient::Result<Self> {
        Self::from_value(serde_json_lenient::from_str(json)?, dark)
    }

    fn from_value(colors: Value, dark: bool) -> serde_json_lenient::Result<Self> {
        let mut value = Self::bundled_value(dark);
        merge(&mut value, colors);
        serde_json_lenient::from_value(value)
    }

    // relative paths start from the folder with settings.jsonc
    pub(crate) fn resolve(path: &Path) -> PathBuf {
        Settings::path()
            .and_then(|settings| settings.parent().map(|dir| dir.join(path)))
            .unwrap_or_else(|| path.to_path_buf())
    }

    // a name with a slash or a json extension is a file, anything else names a theme in `themes`
    fn path_of(name: &str) -> Option<PathBuf> {
        let path = Path::new(name);
        let extension = path.extension().and_then(|extension| extension.to_str());
        let file = name.contains('/') || matches!(extension, Some("json" | "jsonc"));
        file.then(|| Self::resolve(path))
    }

    /// theme `name` from `settings.themes`, or from a file, the error tells what is wrong
    pub fn named(settings: &ThemeSettings, name: &str, dark: bool) -> Result<Self, String> {
        if let Some(path) = Self::path_of(name) {
            return Self::read(&path, dark);
        }
        let Some(colors) = settings.themes.get(name) else {
            return match Self::folder_file(name) {
                Some(path) => Self::read(&path, dark),
                None => Err(format!(
                    "unknown theme {name:?}, add it to \"theme.themes\" or as {name}.jsonc in the themes folder"
                )),
            };
        };
        Self::from_value(colors.clone(), dark)
            .map_err(|error| format!("invalid theme {name:?}: {error}"))
    }

    // `themes/<name>.jsonc` or `.json` next to settings.jsonc, for names not in `themes`
    fn folder_file(name: &str) -> Option<PathBuf> {
        ["jsonc", "json"]
            .into_iter()
            .map(|extension| Self::resolve(Path::new(&format!("themes/{name}.{extension}"))))
            .find(|path| path.exists())
    }

    /// load dark or light theme set in settings, using the bundled one when not set or invalid
    pub fn load(settings: &ThemeSettings, dark: bool) -> Self {
        let name = if dark {
            &settings.dark
        } else {
            &settings.light
        };
        let Some(name) = name else {
            return Self::bundled(dark);
        };
        Self::named(settings, name, dark).unwrap_or_else(|error| {
            eprintln!("{error}");
            Self::bundled(dark)
        })
    }

    /// read a theme file, the error tells what is wrong with it
    pub fn read(path: &Path, dark: bool) -> Result<Self, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|error| format!("failed to read theme {}: {error}", path.display()))?;
        Self::parse(&json, dark)
            .map_err(|error| format!("invalid theme in {}: {error}", path.display()))
    }

    /// theme files picked in settings and in profiles, watched for changes
    pub fn files(settings: &Settings) -> Vec<PathBuf> {
        let profiles = settings.profiles.iter().filter_map(|p| p.theme.as_ref());
        let profiles = profiles.flat_map(|theme| [&theme.dark, &theme.light]);
        [&settings.theme.dark, &settings.theme.light]
            .into_iter()
            .chain(profiles)
            .filter_map(|name| {
                let name = name.as_deref()?;
                Self::path_of(name).or_else(|| {
                    // a theme file in the themes folder is watched too
                    (!settings.theme.themes.contains_key(name))
                        .then(|| Self::folder_file(name))
                        .flatten()
                })
            })
            .collect()
    }

    /// make sure every theme of `settings` can be used, the named ones and the picked ones
    pub fn check(settings: &Settings) -> Result<(), String> {
        let theme = &settings.theme;
        for (name, colors) in &theme.themes {
            // both bundled themes have every color, so either one shows a bad value
            Self::from_value(colors.clone(), true)
                .map_err(|error| format!("invalid theme {name:?}: {error}"))?;
        }
        let profiles = settings.profiles.iter().filter_map(|p| p.theme.as_ref());
        let picks = std::iter::once(theme.clone()).chain(profiles.map(|p| theme.with_profile(p)));
        for pick in picks {
            for (name, dark) in [(&pick.dark, true), (&pick.light, false)] {
                if let Some(name) = name {
                    Self::named(theme, name, dark)?;
                }
            }
        }
        Ok(())
    }

    /// set active theme from settings mode and system appearance
    pub fn apply(appearance: WindowAppearance, cx: &mut App) {
        cx.set_global(Self::for_appearance(&Settings::get(cx).theme, appearance));
    }

    /// theme picked by settings mode and system appearance
    pub fn for_appearance(settings: &ThemeSettings, appearance: WindowAppearance) -> Self {
        let dark = match settings.mode {
            ThemeMode::System => {
                matches!(
                    appearance,
                    WindowAppearance::Dark | WindowAppearance::VibrantDark
                )
            }
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
        };
        Self::load(settings, dark)
    }

    /// active theme
    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// convert alacritty color to gpui color
    pub fn convert_color(&self, color: &Color) -> Hsla {
        match color {
            Color::Named(NamedColor::DimForeground) => self.dim_foreground,
            // named colors share their index with the 0-15 and 256-267 slots
            Color::Named(named) => self.get_color_at_index(*named as usize),
            Color::Spec(rgb) => rgba_color(rgb.r, rgb.g, rgb.b),
            Color::Indexed(i) => self.get_color_at_index(*i as usize),
        }
    }

    /// convert 8 bit ansi color index to gpui color
    pub fn get_color_at_index(&self, index: usize) -> Hsla {
        match index {
            0..=15 => self.ansi[index],
            // 6x6x6 rgb cube, using xterm steps
            16..=231 => {
                let i = index as u8 - 16;
                let step = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                rgba_color(step(i / 36), step((i % 36) / 6), step(i % 6))
            }
            // 24 step grayscale ramp
            232..=255 => {
                let value = (index as u8 - 232) * 10 + 8;
                rgba_color(value, value, value)
            }
            256 => self.terminal_foreground,
            257 => self.terminal_background,
            258 => self.cursor,
            259..=266 => self.ansi_dim[index - 259],
            267 => self.bright_foreground,
            268 => self.ansi[0],
            _ => gpui::black(),
        }
    }
}

fn rgba_color(r: u8, g: u8, b: u8) -> Hsla {
    rgb(u32::from_be_bytes([0, r, g, b])).into()
}

#[cfg(test)]
mod tests {
    use gpui::Rgba;

    use super::*;

    #[test]
    fn indexed_colors_match_xterm() {
        let theme = Theme::default();
        assert_eq!(theme.get_color_at_index(1), rgb(0xe06c75).into());
        // 6x6x6 cube corners and a middle step
        assert_eq!(theme.get_color_at_index(16), rgb(0x000000).into());
        assert_eq!(theme.get_color_at_index(231), rgb(0xffffff).into());
        assert_eq!(
            theme.get_color_at_index(16 + 36 + 6 * 2 + 3),
            rgb(0x5f87af).into()
        );
        // grayscale ramp
        assert_eq!(theme.get_color_at_index(232), rgb(0x080808).into());
        assert_eq!(theme.get_color_at_index(255), rgb(0xeeeeee).into());
    }

    #[test]
    fn special_indices_and_spec_colors() {
        let theme = Theme::default();
        assert_eq!(theme.get_color_at_index(256), theme.terminal_foreground);
        assert_eq!(theme.get_color_at_index(257), theme.terminal_background);
        assert_eq!(theme.get_color_at_index(258), theme.cursor);
        assert_eq!(theme.get_color_at_index(259), theme.ansi_dim[0]);
        assert_eq!(theme.get_color_at_index(266), theme.ansi_dim[7]);
        assert_eq!(theme.get_color_at_index(267), theme.bright_foreground);
        assert_eq!(theme.get_color_at_index(268), theme.ansi[0]);
        assert_eq!(theme.get_color_at_index(999), gpui::black());

        let spec = Color::Spec(alacritty_terminal::vte::ansi::Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        });
        assert_eq!(theme.convert_color(&spec), rgb(0x123456).into());
        assert_eq!(theme.convert_color(&Color::Indexed(1)), theme.ansi[1]);
    }

    #[test]
    fn named_colors_use_theme() {
        let theme = Theme::default();
        assert_eq!(
            theme.convert_color(&Color::Named(NamedColor::Foreground)),
            theme.terminal_foreground
        );
        assert_eq!(
            theme.convert_color(&Color::Named(NamedColor::BrightRed)),
            rgb(0xea858b).into()
        );
        assert_eq!(
            theme.convert_color(&Color::Named(NamedColor::DimForeground)),
            theme.dim_foreground
        );
    }

    #[test]
    fn bundled_themes_parse() {
        assert_eq!(Theme::default(), Theme::bundled(true));
        assert_ne!(Theme::bundled(true), Theme::bundled(false));
        for dark in [true, false] {
            assert_eq!(Theme::parse("{}", dark).unwrap(), Theme::bundled(dark));
        }
    }

    #[test]
    fn partial_theme_fills_from_bundled_of_same_mode() {
        for dark in [true, false] {
            let theme = Theme::parse("{\n // comment\n \"cursor\": \"#abcdef\",\n}", dark).unwrap();
            let mut expected = Theme::bundled(dark);
            expected.cursor = rgb(0xabcdef).into();
            assert_eq!(theme, expected);
        }
    }

    #[test]
    fn ansi_is_replaced_whole() {
        for dark in [true, false] {
            assert!(Theme::parse(r##"{"ansi": ["#ffffff"]}"##, dark).is_err());
            let ansi = vec!["\"#123456\""; 16].join(",");
            let theme = Theme::parse(&format!(r#"{{"ansi": [{ansi}]}}"#), dark).unwrap();
            assert!(theme.ansi.iter().all(|c| *c == rgb(0x123456).into()));
            assert_eq!(theme.ansi_dim, Theme::bundled(dark).ansi_dim);
        }
    }

    #[test]
    fn rejects_invalid_themes() {
        for json in [
            "{",
            "not json",
            "[]",
            "null",
            r#"{"cursor": "nope"}"#,
            r#"{"cursor": null}"#,
        ] {
            assert!(
                Theme::parse(json, true).is_err(),
                "expected error for {json:?}"
            );
        }
    }

    fn theme_settings(json: &str) -> ThemeSettings {
        Settings::parse(&format!(r#"{{"theme": {json}}}"#))
            .unwrap()
            .theme
    }

    #[test]
    fn load_picks_named_themes_and_theme_files() {
        use crate::settings::tests::{temp_dir, with_config_home};

        let dir = temp_dir("theme_load");
        let config = dir.join("kuterm");
        std::fs::create_dir_all(config.join("themes")).unwrap();
        with_config_home(&dir, || {
            let none = theme_settings(r#"{"dark": null, "light": null}"#);
            assert_eq!(Theme::load(&none, true), Theme::bundled(true));
            assert_eq!(Theme::load(&none, false), Theme::bundled(false));

            // a named theme fills missing colors from the bundled theme of the mode it is used for
            let named = theme_settings(
                r##"{"dark": "red", "light": "red", "themes": {"red": {"border": "#010203"}}}"##,
            );
            for dark in [true, false] {
                let mut expected = Theme::bundled(dark);
                expected.border = rgb(0x010203).into();
                assert_eq!(Theme::load(&named, dark), expected);
            }

            // relative paths start from the config folder
            std::fs::write(
                config.join("themes/mine.jsonc"),
                r##"{"border": "#040506"}"##,
            )
            .unwrap();
            let file = theme_settings(r#"{"dark": "themes/mine.jsonc"}"#);
            assert_eq!(Theme::load(&file, true).border, rgb(0x040506).into());
            // a bare name not in "themes" is a file of the themes folder
            let folder = theme_settings(r#"{"dark": "mine"}"#);
            assert_eq!(Theme::load(&folder, true).border, rgb(0x040506).into());
            std::fs::write(
                config.join("themes/other.json"),
                r##"{"border": "#070809"}"##,
            )
            .unwrap();
            let folder = theme_settings(r#"{"dark": "other"}"#);
            assert_eq!(Theme::load(&folder, true).border, rgb(0x070809).into());
            // "themes" wins over the folder
            let both =
                theme_settings(r##"{"dark": "mine", "themes": {"mine": {"border": "#0a0a0a"}}}"##);
            assert_eq!(Theme::load(&both, true).border, rgb(0x0a0a0a).into());

            // absolute paths are used as is, a json extension alone makes it a file too
            let abs = dir.join("abs.jsonc");
            std::fs::write(&abs, r##"{"text": "#0a0b0c"}"##).unwrap();
            let file = theme_settings(&format!(r#"{{"light": {:?}}}"#, abs.display().to_string()));
            assert_eq!(Theme::load(&file, false).text, rgb(0x0a0b0c).into());
            std::fs::write(config.join("plain.json"), r##"{"text": "#0d0e0f"}"##).unwrap();
            let file = theme_settings(r#"{"light": "plain.json"}"#);
            assert_eq!(Theme::load(&file, false).text, rgb(0x0d0e0f).into());

            // unknown names, missing files and broken ones fall back to bundled, nothing is created
            let broken = theme_settings(
                r#"{"dark": "nope", "light": "gone.jsonc", "themes": {"bad": {"cursor": 1}}}"#,
            );
            assert_eq!(Theme::load(&broken, true), Theme::bundled(true));
            assert_eq!(Theme::load(&broken, false), Theme::bundled(false));
            assert!(!config.join("gone.jsonc").exists());
            let broken = theme_settings(r#"{"dark": "bad", "themes": {"bad": {"cursor": 1}}}"#);
            assert_eq!(Theme::load(&broken, true), Theme::bundled(true));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_finds_bad_themes_of_settings_and_profiles() {
        use crate::settings::tests::{temp_dir, with_config_home};

        let dir = temp_dir("theme_check");
        let config = dir.join("kuterm");
        std::fs::create_dir_all(&config).unwrap();
        with_config_home(&dir, || {
            let parse = |theme: &str, profile_light: &str| {
                Settings::parse(&format!(
                    r#"{{
                        "theme": {theme},
                        "profiles": [{{"name": "p", "command": "system",
                                      "theme": {{"mode": "light", "light": {profile_light:?}}}}}],
                    }}"#
                ))
                .unwrap()
            };
            let settings = parse(r#"{"dark": "d.jsonc", "light": null}"#, "p.jsonc");
            assert_eq!(
                Theme::files(&settings),
                [config.join("d.jsonc"), config.join("p.jsonc")]
            );
            // missing files are errors naming them
            let error = Theme::check(&settings).unwrap_err();
            assert!(error.contains("d.jsonc"), "{error}");
            std::fs::write(config.join("d.jsonc"), "{}").unwrap();
            std::fs::write(config.join("p.jsonc"), r#"{"cursor": "nope"}"#).unwrap();
            let error = Theme::check(&settings).unwrap_err();
            assert!(error.contains("p.jsonc"), "{error}");
            std::fs::write(config.join("p.jsonc"), "{}").unwrap();
            assert_eq!(Theme::check(&settings), Ok(()));

            // names given in profiles come from the themes block
            let settings = parse(r#"{"themes": {"x": {}}}"#, "x");
            assert_eq!(Theme::check(&settings), Ok(()));
            assert!(Theme::files(&settings).is_empty());
            let error = Theme::check(&parse("{}", "x")).unwrap_err();
            assert!(error.contains("unknown theme \"x\""), "{error}");

            // a broken theme is found even when nothing uses it
            let settings = parse(r#"{"themes": {"unused": {"cursor": 1}}}"#, "light");
            let error = Theme::check(&settings).unwrap_err();
            assert!(error.contains("unused"), "{error}");
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundled_theme_colors_round_trip_exactly() {
        // color replies (osc 4/10/11/12) round the theme hsla back to 8 bit rgb, which must
        // give the exact hex from the theme file
        for dark in [true, false] {
            let value = Theme::bundled_value(dark);
            let theme = Theme::bundled(dark);
            let c = |v: f32| (v * 255.).round() as u8;
            let hex = |h: Hsla| {
                let rgba: Rgba = h.into();
                format!("#{:02x}{:02x}{:02x}", c(rgba.r), c(rgba.g), c(rgba.b))
            };
            for (i, color) in theme.ansi.iter().enumerate() {
                assert_eq!(
                    hex(*color),
                    value["ansi"][i].as_str().unwrap().to_lowercase(),
                    "dark={dark} ansi {i}"
                );
            }
            assert_eq!(
                hex(theme.terminal_background),
                value["terminal_background"]
                    .as_str()
                    .unwrap()
                    .to_lowercase()
            );
            assert_eq!(
                hex(theme.terminal_foreground),
                value["terminal_foreground"]
                    .as_str()
                    .unwrap()
                    .to_lowercase()
            );
            assert_eq!(
                hex(theme.cursor),
                value["cursor"].as_str().unwrap().to_lowercase()
            );
        }
    }
}
