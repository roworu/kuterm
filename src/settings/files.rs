//! where config and state files live, and reading and writing them

use std::{
    io::Read,
    path::{Path, PathBuf},
};

// themes and completion specs are far smaller, a bigger file is a wrong path
const MAX_FILE_BYTES: u64 = 8 << 20;

/// text of the file at `path`, refused when it is not a plain file or too big to be one
/// kuterm reads, like a device or a fifo that never ends
pub fn read_small_file(path: &Path) -> std::io::Result<String> {
    let file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return Err(std::io::Error::other(format!(
            "not a file of at most {} MB",
            MAX_FILE_BYTES >> 20
        )));
    }
    let mut text = String::new();
    file.take(MAX_FILE_BYTES).read_to_string(&mut text)?;
    Ok(text)
}

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

    #[test]
    fn only_small_plain_files_are_read() {
        let dir = temp_dir("small_file");
        std::fs::create_dir_all(&dir).unwrap();
        let small = dir.join("small.jsonc");
        std::fs::write(&small, "{}").unwrap();
        assert_eq!(read_small_file(&small).unwrap(), "{}");
        let big = dir.join("big.jsonc");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert!(read_small_file(&big).is_err());
        assert!(read_small_file(Path::new("/dev/zero")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
