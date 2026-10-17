//! where config and state files live, and reading and writing them

use std::path::{Path, PathBuf};

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

/// `value` read from json at `path`, none when the file is missing or invalid; `what` names
/// it in the error on stderr
pub fn load_json<T: serde::de::DeserializeOwned>(path: &Path, what: &str) -> Option<T> {
    let json = std::fs::read_to_string(path).ok()?;
    serde_json_lenient::from_str(&json)
        .inspect_err(|error| eprintln!("invalid {what} in {}: {error}", path.display()))
        .ok()
}

/// write `value` as json to `path`, creating its folder, telling failures on stderr
pub fn save_json(path: &Path, value: &impl serde::Serialize) {
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| serde_json_lenient::to_string_pretty(value).map_err(std::io::Error::other))
        .and_then(|json| std::fs::write(path, json));
    if let Err(error) = result {
        eprintln!("failed to write {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::tests::temp_dir;

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
}
