//! icons of programs shown in tabs, "icons.dynamic_app_icons" in settings

use gpui::{App, Global};
use serde::Deserialize;
use serde_json_lenient::Value;

use super::config;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct IconGroup {
    pub icon: String,
    pub commands: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TabIcons {
    /// programs matched by their script name instead of their own
    pub interpreters: Vec<String>,
    /// programs like sudo whose child is the real command
    pub wrappers: Vec<String>,
    pub groups: Vec<IconGroup>,
}

impl Default for TabIcons {
    fn default() -> Self {
        Self::parse("{}").expect("bundled tab icons are invalid")
    }
}

impl Global for TabIcons {}

impl TabIcons {
    /// parse a dynamic app icons section over the bundled one
    pub fn parse(json: &str) -> serde_json_lenient::Result<Self> {
        let section = serde_json_lenient::from_str(json)?;
        Self::from_full(&config::combine(config::icons_section(section)))
    }

    /// dynamic app icons section of settings merged over the bundled ones
    pub fn from_full(full: &Value) -> serde_json_lenient::Result<Self> {
        Self::deserialize(
            full.pointer("/icons/dynamic_app_icons")
                .unwrap_or(&Value::Null),
        )
    }

    /// tab icons loaded at startup or on the last reload
    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// icon of the first group listing `command`
    pub fn icon(&self, command: &str) -> Option<&str> {
        self.groups
            .iter()
            .find(|group| group.commands.iter().any(|name| name == command))
            .map(|group| group.icon.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn bundled_icons_are_single_nerd_glyphs() {
        let icons = TabIcons::default();
        let mut seen = HashSet::new();
        for group in &icons.groups {
            let mut chars = group.icon.chars();
            let glyph = chars.next().unwrap();
            assert!(chars.next().is_none(), "{:?} is not one glyph", group.icon);
            // nerd font icons live in the private use areas
            assert!(
                ('\u{e000}'..='\u{f8ff}').contains(&glyph)
                    || ('\u{f0000}'..='\u{ffffd}').contains(&glyph),
                "{glyph:?} is not a nerd font icon"
            );
            for command in &group.commands {
                assert!(seen.insert(command), "{command} is in two groups");
            }
        }
    }

    #[test]
    fn user_groups_replace_bundled_ones_whole() {
        let icons =
            TabIcons::parse(r#"{"groups": [{"icon": "H", "commands": ["htop", "mc"]}]}"#).unwrap();
        assert_eq!(icons.icon("htop"), Some("H"));
        assert_eq!(icons.icon("mc"), Some("H"));
        assert_eq!(icons.icon("ssh"), None);
        // left out keys keep their defaults
        assert_eq!(icons.interpreters, TabIcons::default().interpreters);
    }

    #[test]
    fn first_group_wins() {
        let icons = TabIcons::parse(
            r#"{"groups": [{"icon": "A", "commands": ["x"]}, {"icon": "B", "commands": ["x", "y"]}]}"#,
        )
        .unwrap();
        assert_eq!(icons.icon("x"), Some("A"));
        assert_eq!(icons.icon("y"), Some("B"));
    }

    #[test]
    fn rejects_invalid_icons() {
        for json in [
            "not json",
            r#"{"groups": {"icon": "A"}}"#,
            r#"{"groups": [{"icon": "A"}]}"#,
            r#"{"groups": [{"commands": ["x"]}]}"#,
            r#"{"groups": [{"icon": 1, "commands": ["x"]}]}"#,
            r#"{"interpreters": "node"}"#,
            r#"{"wrappers": "sudo"}"#,
        ] {
            assert!(
                TabIcons::parse(json).is_err(),
                "expected error for {json:?}"
            );
        }
    }
}
