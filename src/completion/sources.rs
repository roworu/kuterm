//! names completions come from that no spec lists: programs in PATH and files on disk.
//! both are only read, nothing is run

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

/// shell builtins and keywords that are no file in PATH
const BUILTINS: &[&str] = &[
    "alias", "bg", "builtin", "cd", "command", "echo", "eval", "exec", "exit", "export", "fg",
    "history", "jobs", "kill", "popd", "printf", "pushd", "pwd", "read", "set", "source", "time",
    "type", "unalias", "unset", "wait", "which",
];

// a huge folder must not stall the popup
const MAX_ENTRIES: usize = 5000;

/// PATH the cached list was read from, the folders with when they last changed, the list
type ProgramCache = (String, Vec<(PathBuf, Option<SystemTime>)>, Arc<Vec<String>>);

static PROGRAMS: Mutex<Option<ProgramCache>> = Mutex::new(None);

/// names of the programs in `path_var` and of shell builtins, sorted, without repeats.
/// read again only when PATH or one of its folders changed
pub fn programs(path_var: &str) -> Arc<Vec<String>> {
    let dirs: Vec<(PathBuf, Option<SystemTime>)> = std::env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .map(|dir| {
            let modified = std::fs::metadata(&dir).and_then(|m| m.modified()).ok();
            (dir, modified)
        })
        .collect();
    let mut cache = PROGRAMS.lock().unwrap();
    if let Some((cached_var, cached_dirs, names)) = &*cache
        && cached_var == path_var
        && *cached_dirs == dirs
    {
        return names.clone();
    }
    let mut names: Vec<String> = BUILTINS.iter().map(|name| name.to_string()).collect();
    for (dir, _) in &dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten().take(MAX_ENTRIES) {
            if is_executable(&entry.path())
                && let Some(name) = entry.file_name().to_str()
            {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    let names = Arc::new(names);
    *cache = Some((path_var.to_string(), dirs, names.clone()));
    names
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    // follows links, most of PATH is links on some systems
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// a file or folder found for a typed path
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// the typed path up to its last `/`, then the name
    pub path: String,
    pub name: String,
    pub dir: bool,
}

/// entries of the folder `typed` points into whose names could complete it. relative paths
/// start at `cwd`, `~` is the home folder. hidden files only when the name starts with a dot
pub fn entries(typed: &str, cwd: &Path, home: Option<&Path>) -> Vec<Entry> {
    let (dir_part, name_part) = match typed.rfind('/') {
        Some(ix) => typed.split_at(ix + 1),
        None => ("", typed),
    };
    let dir = if dir_part.is_empty() {
        cwd.to_path_buf()
    } else if let Some(rest) = dir_part.strip_prefix("~/") {
        let Some(home) = home else {
            return Vec::new();
        };
        home.join(rest)
    } else {
        cwd.join(dir_part)
    };
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let hidden = name_part.starts_with('.');
    let mut found = Vec::new();
    for entry in read.flatten().take(MAX_ENTRIES) {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        // a newline or other control character can't be typed back at a prompt
        if (name.starts_with('.') && !hidden) || name.chars().any(char::is_control) {
            continue;
        }
        // the listing tells the type, only links need a look at their target
        let dir = match entry.file_type() {
            Ok(kind) if kind.is_symlink() => {
                std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
            }
            Ok(kind) => kind.is_dir(),
            Err(_) => false,
        };
        found.push(Entry {
            path: format!("{dir_part}{name}"),
            name,
            dir,
        });
    }
    found
}

/// `text` as one shell word, continuing `quote` when the typed word opened one. `close`
/// ends the word, a folder is left open so its contents can follow
pub fn escape(text: &str, quote: Option<char>, close: bool) -> String {
    let mut out = String::new();
    match quote {
        Some('\'') => {
            out.push('\'');
            out.push_str(&text.replace('\'', "'\\''"));
            if close {
                out.push('\'');
            }
        }
        Some(quote) => {
            out.push(quote);
            for c in text.chars() {
                if matches!(c, '"' | '\\' | '$' | '`') {
                    out.push('\\');
                }
                out.push(c);
            }
            if close {
                out.push(quote);
            }
        }
        None => {
            for (ix, c) in text.chars().enumerate() {
                // a leading ~ is the home folder, the shell has to see it bare
                let home = ix == 0 && c == '~';
                if !home && " \t'\"\\$`&|;<>()*?[]{}!#~=%^".contains(c) {
                    out.push('\\');
                }
                out.push(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kuterm-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn entries_of_the_typed_folder() {
        let dir = temp_dir("entries");
        std::fs::create_dir_all(dir.join("sub dir/inner")).unwrap();
        std::fs::write(dir.join("sub dir/file.txt"), "").unwrap();
        std::fs::write(dir.join(".hidden"), "").unwrap();
        std::fs::write(dir.join("top"), "").unwrap();

        let mut found = entries("", &dir, None);
        found.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<_> = found.iter().map(|e| (e.name.as_str(), e.dir)).collect();
        assert_eq!(names, [("sub dir", true), ("top", false)]);

        let mut found = entries("sub dir/", &dir, None);
        found.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(found[0].path, "sub dir/file.txt");
        assert_eq!(found[1].path, "sub dir/inner");
        assert!(found[1].dir);

        // names are matched by the caller, a dot only lets hidden ones in
        assert!(
            entries(".h", &dir, None)
                .iter()
                .any(|e| e.name == ".hidden")
        );
        let home = entries("~/sub dir/in", Path::new("/"), Some(&dir));
        assert!(home.iter().any(|e| e.path == "~/sub dir/inner" && e.dir));
        assert!(entries("missing/", &dir, None).is_empty());
        // a link is what it points at
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("sub dir"), dir.join("link")).unwrap();
            let found = entries("li", &dir, None);
            assert!(found.iter().any(|e| e.name == "link" && e.dir));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn programs_come_from_path_and_builtins() {
        let dir = temp_dir("programs");
        let tool = dir.join("kuterm-test-tool");
        std::fs::write(&tool, "").unwrap();
        std::fs::write(dir.join("not-executable"), "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path_var = format!("{}:relative", dir.display());
        let names = programs(&path_var);
        assert!(names.contains(&"kuterm-test-tool".to_string()));
        assert!(names.contains(&"cd".to_string()));
        #[cfg(unix)]
        assert!(!names.contains(&"not-executable".to_string()));
        // a new program in the folder shows without a restart
        std::fs::remove_file(&tool).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("touch-dir"), "").unwrap();
        assert!(!programs(&path_var).contains(&"kuterm-test-tool".to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn words_are_escaped_for_their_quote() {
        assert_eq!(escape("my file", None, true), "my\\ file");
        assert_eq!(escape("~/a b", None, true), "~/a\\ b");
        assert_eq!(escape("a~b", None, true), "a\\~b");
        assert_eq!(escape("it's", Some('\''), true), "'it'\\''s'");
        assert_eq!(escape("dir/", Some('\''), false), "'dir/");
        assert_eq!(escape("a \"$x\"", Some('"'), true), "\"a \\\"\\$x\\\"\"");
        assert_eq!(escape("plain-name.txt", None, true), "plain-name.txt");
    }
}
