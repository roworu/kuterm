//! last window size, remembered between launches when `window.remember_size` is on

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{load_json, save_json, state_dir};

/// window size in logical pixels
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct WindowSize {
    pub width: f32,
    pub height: f32,
}

impl WindowSize {
    /// `window_size.json` in the state dir
    pub fn path() -> Option<PathBuf> {
        Some(state_dir()?.join("window_size.json"))
    }

    /// size from the state file, none when it is missing or invalid
    pub fn load() -> Option<Self> {
        Self::load_from(&Self::path()?)
    }

    /// write the size to the state file
    pub fn save(&self) {
        if let Some(path) = Self::path() {
            self.save_to(&path);
        }
    }

    fn load_from(path: &Path) -> Option<Self> {
        let size: Self = load_json(path, "window size")?;
        let valid = |v: f32| v.is_finite() && v > 0.;
        (valid(size.width) && valid(size.height)).then_some(size)
    }

    fn save_to(&self, path: &Path) {
        save_json(path, self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::tests::temp_dir;

    #[test]
    fn size_round_trips_through_the_file() {
        let dir = temp_dir("window_size_round_trip");
        let path = dir.join("state/window_size.json");
        assert_eq!(WindowSize::load_from(&path), None);
        let size = WindowSize {
            width: 812.5,
            height: 400.,
        };
        size.save_to(&path);
        assert_eq!(WindowSize::load_from(&path), Some(size));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_or_empty_sizes_are_ignored() {
        let dir = temp_dir("window_size_broken");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("window_size.json");
        for json in [
            "not json",
            r#"{"width": 100}"#,
            r#"{"width": 0, "height": 100}"#,
            r#"{"width": 100, "height": -5}"#,
        ] {
            std::fs::write(&path, json).unwrap();
            assert_eq!(WindowSize::load_from(&path), None, "{json}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
