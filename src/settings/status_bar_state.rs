//! status bar choices made with clicks, remembered between launches

use std::path::{Path, PathBuf};

use gpui::{App, Global};
use serde::{Deserialize, Serialize};

use super::{MemoryUnit, state_dir};

/// what clicks on the status bar picked
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct StatusBarState {
    /// unit picked with a click on memory
    #[serde(default)]
    pub memory_unit: Option<MemoryUnit>,
    /// `status_bar.memory_unit` when the unit was picked, so editing that setting wins
    #[serde(default)]
    pub memory_unit_default: Option<MemoryUnit>,
}

impl Global for StatusBarState {}

impl StatusBarState {
    /// `status_bar.json` in the state dir
    pub fn path() -> Option<PathBuf> {
        Some(state_dir()?.join("status_bar.json"))
    }

    /// state from the file, empty when it is missing or invalid
    pub fn load() -> Self {
        Self::path()
            .map(|path| Self::load_from(&path))
            .unwrap_or_default()
    }

    /// write the state to the state dir
    pub fn save(&self) {
        if let Some(path) = Self::path() {
            self.save_to(&path);
        }
    }

    /// unit memory shows in: the picked one, unless the setting changed since
    pub fn memory_unit(&self, default: MemoryUnit) -> MemoryUnit {
        match self.memory_unit {
            Some(unit) if self.memory_unit_default == Some(default) => unit,
            _ => default,
        }
    }

    /// pick the unit after the one shown now
    pub fn next_memory_unit(&mut self, default: MemoryUnit) {
        self.memory_unit = Some(self.memory_unit(default).next());
        self.memory_unit_default = Some(default);
    }

    /// state loaded at startup, empty when none was
    pub fn get(cx: &App) -> &Self {
        static EMPTY: StatusBarState = StatusBarState {
            memory_unit: None,
            memory_unit_default: None,
        };
        cx.try_global::<Self>().unwrap_or(&EMPTY)
    }

    fn load_from(path: &Path) -> Self {
        let Ok(json) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json_lenient::from_str(&json).unwrap_or_else(|error| {
            eprintln!("invalid status bar state in {}: {error}", path.display());
            Self::default()
        })
    }

    fn save_to(&self, path: &Path) {
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                serde_json_lenient::to_string_pretty(self).map_err(std::io::Error::other)
            })
            .and_then(|json| std::fs::write(path, json));
        if let Err(error) = result {
            eprintln!("failed to write {}: {error}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::tests::temp_dir;

    #[test]
    fn picked_unit_round_trips_through_the_file() {
        let dir = temp_dir("status_bar_state_round_trip");
        let path = dir.join("state/status_bar.json");
        assert_eq!(StatusBarState::load_from(&path), StatusBarState::default());
        let mut state = StatusBarState::default();
        state.next_memory_unit(MemoryUnit::Mb);
        state.save_to(&path);
        assert_eq!(StatusBarState::load_from(&path), state);
        assert_eq!(state.memory_unit(MemoryUnit::Mb), MemoryUnit::Gb);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(StatusBarState::load_from(&path), StatusBarState::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clicks_cycle_units_until_the_setting_changes() {
        let mut state = StatusBarState::default();
        assert_eq!(state.memory_unit(MemoryUnit::Auto), MemoryUnit::Auto);
        let mut seen = Vec::new();
        for _ in 0..4 {
            state.next_memory_unit(MemoryUnit::Auto);
            seen.push(state.memory_unit(MemoryUnit::Auto));
        }
        use MemoryUnit::*;
        assert_eq!(seen, [Kb, Mb, Gb, Auto]);
        state.next_memory_unit(MemoryUnit::Auto);
        // a new default in the settings is a newer choice than the last click
        assert_eq!(state.memory_unit(MemoryUnit::Gb), MemoryUnit::Gb);
    }
}
