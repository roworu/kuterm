//! which plugins to run: entries of the "plugins" setting, a program with arguments or a git
//! repository with a `kuterm-plugin.json` manifest, and the hash its trust is stored with

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::permissions::Permissions;

/// file a plugin repository describes itself in
pub const MANIFEST: &str = "kuterm-plugin.json";

/// one entry of the "plugins" setting
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PluginEntry {
    /// "github:user/repo" or "github:user/repo@rev"
    Git(String),
    Local(PluginSpec),
}

/// a plugin ready to start
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PluginSpec {
    pub name: String,
    /// program and its arguments
    pub command: Vec<String>,
    /// what it may do, nothing it doesn't name
    #[serde(default)]
    pub permissions: Permissions,
    /// folder it runs in, the repository of a git plugin
    #[serde(skip)]
    pub dir: Option<PathBuf>,
}

/// a repository on github and the revision to check out
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitSource {
    pub url: String,
    pub rev: Option<String>,
    /// folder name under the plugins folder
    pub folder: String,
}

/// the repository "github:user/repo[@rev]" names
pub fn parse_git(entry: &str) -> Option<GitSource> {
    let rest = entry.strip_prefix("github:")?;
    let (repo, rev) = match rest.split_once('@') {
        Some((repo, rev)) => (repo, Some(rev.to_string()).filter(|rev| !rev.is_empty())),
        None => (rest, None),
    };
    let (user, name) = repo.split_once('/')?;
    let fine = |part: &str| {
        !part.is_empty()
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    (fine(user) && fine(name)).then(|| GitSource {
        url: format!("https://github.com/{user}/{name}"),
        rev,
        folder: format!("{user}-{name}"),
    })
}

/// read the manifest of a cloned plugin in `dir`
pub fn load_manifest(dir: &Path) -> Result<PluginSpec, String> {
    let path = dir.join(MANIFEST);
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut spec: PluginSpec = serde_json_lenient::from_str(&text)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    spec.dir = Some(dir.to_path_buf());
    Ok(spec)
}

/// clone `source` into `plugins_dir` unless it is there, then read its manifest
pub fn fetch(source: &GitSource, plugins_dir: &Path) -> Result<PluginSpec, String> {
    let dir = plugins_dir.join(&source.folder);
    if !dir.join(MANIFEST).exists() {
        std::fs::create_dir_all(plugins_dir).map_err(|error| error.to_string())?;
        let status = std::process::Command::new("git")
            .args(["clone", "--quiet", &source.url])
            .arg(&dir)
            .status()
            .map_err(|error| format!("git clone {}: {error}", source.url))?;
        if !status.success() {
            return Err(format!("git clone {} failed", source.url));
        }
        if let Some(rev) = &source.rev {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["checkout", "--quiet", rev])
                .status()
                .map_err(|error| error.to_string())?;
            if !status.success() {
                return Err(format!("git checkout {rev} failed in {}", dir.display()));
            }
        }
    }
    load_manifest(&dir)
}

/// fnv-1a, stable across builds unlike std's hasher, so stored trust survives updates
fn fnv(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

impl PluginSpec {
    /// hash of what the plugin is: its command, permissions and the files it names, so a
    /// changed script or wider permissions ask again
    pub fn hash(&self) -> String {
        let mut hash = 0xcbf29ce484222325;
        let json =
            serde_json_lenient::to_string(&(&self.command, &self.permissions)).unwrap_or_default();
        fnv(&mut hash, json.as_bytes());
        for arg in &self.command {
            let path = match &self.dir {
                Some(dir) => dir.join(arg),
                None => PathBuf::from(arg),
            };
            if path.is_file()
                && let Ok(bytes) = std::fs::read(&path)
            {
                fnv(&mut hash, &bytes);
            }
        }
        format!("{hash:016x}")
    }
}

#[cfg(test)]
mod tests {
    use super::{GitSource, PluginEntry, PluginSpec, load_manifest, parse_git};

    #[test]
    fn entries_are_local_programs_or_git_repositories() {
        let entries: Vec<PluginEntry> = serde_json_lenient::from_str(
            r#"["github:me/tool@v1", { "name": "x", "command": ["python3", "x.py"] }]"#,
        )
        .unwrap();
        assert_eq!(entries[0], PluginEntry::Git("github:me/tool@v1".into()));
        let PluginEntry::Local(spec) = &entries[1] else {
            panic!("not local");
        };
        assert_eq!(spec.command, ["python3", "x.py"]);
        assert!(!spec.permissions.pty_write);

        assert_eq!(
            parse_git("github:me/tool@v1"),
            Some(GitSource {
                url: "https://github.com/me/tool".into(),
                rev: Some("v1".into()),
                folder: "me-tool".into(),
            })
        );
        assert_eq!(parse_git("github:me/tool").unwrap().rev, None);
        assert_eq!(parse_git("github:../x"), None);
        assert_eq!(parse_git("github:me/a b"), None);
        assert_eq!(parse_git("gitlab:me/x"), None);
    }

    #[test]
    fn hash_changes_with_the_script_and_the_permissions() {
        let dir = std::env::temp_dir().join(format!("kuterm_plugin_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(super::MANIFEST),
            r#"{ "name": "p", "command": ["python3", "p.py"], "permissions": { "panes": true } }"#,
        )
        .unwrap();
        std::fs::write(dir.join("p.py"), "print(1)").unwrap();
        let spec: PluginSpec = load_manifest(&dir).unwrap();
        assert!(spec.permissions.panes);
        let first = spec.hash();
        assert_eq!(first, spec.hash());
        std::fs::write(dir.join("p.py"), "print(2)").unwrap();
        let changed = spec.hash();
        assert_ne!(first, changed);
        let mut wider = spec.clone();
        wider.permissions.pty_write = true;
        assert_ne!(wider.hash(), changed);
        assert!(load_manifest(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
