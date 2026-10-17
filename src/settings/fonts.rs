//! font names and opentype features from the settings

use super::*;

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

impl Settings {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
