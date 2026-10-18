//! the whole config: settings.jsonc with the files it imports and the block for this os

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gpui::Global;
use serde_json_lenient::{Map, Value};

use super::{Commands, Keybindings, Settings, TabIcons, create_default_file};
use crate::cli::Cli;

/// every section of the config, parsed and checked
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub settings: Settings,
    pub keybindings: Keybindings,
    pub commands: Commands,
    pub tab_icons: TabIcons,
}

/// what reading the config files gave
pub struct ConfigLoad {
    /// the error tells what is wrong and in which file
    pub config: Result<Config, String>,
    /// every file read or looked for, so a missing file showing up is noticed too
    pub files: Vec<PathBuf>,
    /// problems that did not stop loading, like a missing import
    pub warnings: Vec<String>,
}

/// files the running config came from, watched for changes
#[derive(Clone, Debug, Default)]
pub struct ConfigFiles {
    pub paths: Vec<PathBuf>,
    /// errors and warnings from launch, shown once the window is open
    pub problems: Vec<String>,
}

impl Global for ConfigFiles {}

// one file's values with its path, merged in the order they are listed
type Layer = (PathBuf, Value);

/// bundled settings with the block for this os merged in
pub(crate) fn defaults() -> Value {
    static DEFAULTS: OnceLock<Value> = OnceLock::new();
    DEFAULTS
        .get_or_init(|| {
            let mut value = serde_json_lenient::from_str(DEFAULT_SETTINGS)
                .expect("bundled default settings are invalid");
            apply_os(&mut value);
            value
        })
        .clone()
}

/// user values merged over the bundled ones, the block for this os goes last on both
pub(crate) fn combine(mut user: Value) -> Value {
    let mut full = defaults();
    // done on each side alone, so a bundled macos key never wins over a user key
    apply_os(&mut user);
    move_flat_font_keys(&mut user);
    merge(&mut full, user);
    full
}

/// `value` as the `name` section of settings
pub(crate) fn section(name: &str, value: Value) -> Value {
    Value::Object(Map::from_iter([(name.to_string(), value)]))
}

fn keybindings_section(value: Value) -> Value {
    section("keybindings", value)
}

/// `value` as the "icons.dynamic_app_icons" section of settings
pub(crate) fn icons_section(value: Value) -> Value {
    section("icons", section("dynamic_app_icons", value))
}

// old configs set "terminal.font_family" and "font_size", they now live in "terminal.font".
// a key already set in "font" wins
fn move_flat_font_keys(user: &mut Value) {
    let Some(terminal) = user.get_mut("terminal").and_then(Value::as_object_mut) else {
        return;
    };
    for (old, new) in [("font_family", "family"), ("font_size", "size")] {
        let Some(value) = terminal.remove(old) else {
            continue;
        };
        let font = terminal
            .entry("font")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(font) = font.as_object_mut() {
            font.entry(new).or_insert(value);
        }
    }
}

// "os" holds blocks for single systems, only the one for this system is used
fn apply_os(value: &mut Value) {
    let Some(Value::Object(mut blocks)) =
        value.as_object_mut().and_then(|object| object.remove("os"))
    else {
        return;
    };
    if let Some(block) = blocks.remove(std::env::consts::OS) {
        merge(value, block);
    }
}

impl Config {
    /// read settings with everything they import, creating the settings file first
    pub fn load() -> ConfigLoad {
        let mut load = ConfigLoad {
            config: Ok(Self::default()),
            files: Vec::new(),
            warnings: Vec::new(),
        };
        load.config = read_layers(&mut load).and_then(|layers| build(&layers));
        load
    }
}

fn read_layers(load: &mut ConfigLoad) -> Result<Vec<Layer>, String> {
    let mut layers = Vec::new();
    let Some(main) = Settings::path() else {
        return Ok(layers);
    };
    create_default_file(&main, DEFAULT_SETTINGS);
    read_with_imports(&main, &mut Vec::new(), &mut layers, load)?;
    let cli = Cli::get();
    for (path, wrap) in [
        (
            &cli.keybindings_file,
            keybindings_section as fn(Value) -> Value,
        ),
        (&cli.tab_icons_file, icons_section),
    ] {
        let Some(path) = path else { continue };
        load.files.push(path.clone());
        if !path.exists() {
            load.warnings
                .push(format!("{} not found, skipped", path.display()));
            continue;
        }
        layers.push((path.clone(), wrap(read(path)?)));
    }
    Ok(layers)
}

// imports go before the file itself, so its own values win
fn read_with_imports(
    path: &Path,
    chain: &mut Vec<PathBuf>,
    layers: &mut Vec<Layer>,
    load: &mut ConfigLoad,
) -> Result<(), String> {
    load.files.push(path.to_path_buf());
    let mut value = read(path)?;
    let imports = match value
        .as_object_mut()
        .and_then(|object| object.remove("import"))
    {
        None | Some(Value::Null) => Vec::new(),
        Some(imports) => serde_json_lenient::from_value::<Vec<PathBuf>>(imports)
            .map_err(|error| format!("invalid import in {}: {error}", path.display()))?,
    };
    // real paths, so "a.jsonc" and "./a.jsonc" are the same file
    chain.push(canonical(path));
    for import in imports {
        let import = resolve(&import);
        if !import.exists() {
            load.files.push(import.clone());
            load.warnings.push(format!(
                "{} imports {}, which is not found, skipped",
                path.display(),
                import.display()
            ));
            continue;
        }
        if chain.contains(&canonical(&import)) {
            load.warnings.push(format!(
                "{} imports {}, which is already being read, skipped to stop the loop",
                path.display(),
                import.display()
            ));
            continue;
        }
        read_with_imports(&import, chain, layers, load)?;
    }
    chain.pop();
    layers.push((path.to_path_buf(), value));
    Ok(())
}

fn read(path: &Path) -> Result<Value, String> {
    let json = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let value: Value = serde_json_lenient::from_str(&json)
        .map_err(|error| format!("invalid settings in {}: {error}", path.display()))?;
    // merging would quietly drop a list or a number, so only objects are allowed
    if !value.is_object() {
        return Err(format!(
            "invalid settings in {}: expected an object in {{ }}",
            path.display()
        ));
    }
    Ok(value)
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// "~/" is the home folder, relative paths start from the folder with settings.jsonc
pub(crate) fn resolve(path: &Path) -> PathBuf {
    if let Ok(rest) = path.strip_prefix("~")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    Settings::path()
        .and_then(|settings| settings.parent().map(|dir| dir.join(path)))
        .unwrap_or_else(|| path.to_path_buf())
}

fn build(layers: &[Layer]) -> Result<Config, String> {
    let mut user = Value::Object(Map::new());
    for (_, value) in layers {
        merge(&mut user, value.clone());
    }
    parse(user).map_err(|error| {
        // merged values don't tell where a bad value came from, so the first file that is
        // wrong on its own is named
        layers
            .iter()
            .find_map(|(path, value)| {
                let error = parse(value.clone()).err()?;
                Some(format!("invalid settings in {}: {error}", path.display()))
            })
            .unwrap_or_else(|| format!("invalid settings: {error}"))
    })
}

fn parse(user: Value) -> Result<Config, String> {
    let full = combine(user);
    Ok(Config {
        keybindings: Keybindings::from_full(&full)
            .map_err(|error| format!("keybindings: {error}"))?,
        commands: Commands::from_full(&full).map_err(|error| format!("commands: {error}"))?,
        tab_icons: TabIcons::from_full(&full)
            .map_err(|error| format!("icons.dynamic_app_icons: {error}"))?,
        settings: Settings::from_full(full).map_err(|error| error.to_string())?,
    })
}

/// commented settings file written on first launch
pub(crate) const DEFAULT_SETTINGS: &str = include_str!("../../assets/default_settings.jsonc");

// user values override defaults key by key, so a partial file keeps the other defaults.
// anything that is not an object on both sides (enum values, arrays) is replaced as a whole
pub(crate) fn merge(base: &mut Value, overrides: Value) {
    match (base, overrides) {
        (Value::Object(base), Value::Object(overrides)) => {
            for (key, value) in overrides {
                // a missing key starts as null, which the fallback arm replaces with the value
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, overrides) => *base = overrides,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::tests::{temp_dir, with_config_home};

    /// write files into the config folder of `dir`, then load the config
    fn load_with(dir: &Path, files: &[(&str, &str)]) -> ConfigLoad {
        let config = dir.join("kuterm");
        for (name, json) in files {
            let path = config.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, json).unwrap();
        }
        Config::load()
    }

    fn with_dir(name: &str, f: impl FnOnce(&Path)) {
        let dir = temp_dir(name);
        with_config_home(&dir, || f(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_launch_creates_only_settings() {
        with_dir("config_first_launch", |dir| {
            let load = Config::load();
            assert_eq!(load.config.unwrap(), Config::default());
            assert!(load.warnings.is_empty(), "{:?}", load.warnings);
            let created: Vec<_> = std::fs::read_dir(dir.join("kuterm"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert_eq!(created, ["settings.jsonc"]);
            assert_eq!(
                std::fs::read_to_string(dir.join("kuterm/settings.jsonc")).unwrap(),
                DEFAULT_SETTINGS
            );
        });
    }

    #[test]
    fn all_sections_live_in_settings() {
        with_dir("config_sections", |dir| {
            let load = load_with(
                dir,
                &[(
                    "settings.jsonc",
                    r#"{
                        "ui_font_size": 13,
                        "keybindings": {"new_tab": "ctrl-shift-y", "close_tab": null},
                        "commands": [{"name": "only", "actions": ["new_tab"]}],
                        "icons": {"dynamic_app_icons": {"interpreters": ["ruby"]}},
                    }"#,
                )],
            );
            let config = load.config.unwrap();
            assert_eq!(config.settings.ui_font_size, 13.);
            assert_eq!(
                config.keybindings,
                Keybindings::parse(r#"{"new_tab": "ctrl-shift-y", "close_tab": null}"#).unwrap()
            );
            assert_eq!(config.commands.commands.len(), 1);
            assert_eq!(config.tab_icons.interpreters, ["ruby"]);
            assert_eq!(config.tab_icons.groups, TabIcons::default().groups);
        });
    }

    #[test]
    fn imports_merge_in_order_and_the_main_file_wins() {
        with_dir("config_imports", |dir| {
            let colors = dir.join("elsewhere/colors.jsonc");
            std::fs::create_dir_all(colors.parent().unwrap()).unwrap();
            std::fs::write(&colors, r#"{"ui_font_size": 11, "tab_width": 111}"#).unwrap();
            let main = format!(
                r#"{{"import": [{:?}, "local.jsonc"], "ui_font_size": 13}}"#,
                colors.display().to_string()
            );
            let load = load_with(
                dir,
                &[
                    ("settings.jsonc", &main),
                    (
                        "local.jsonc",
                        r#"{"tab_width": 222, "default_title": "local"}"#,
                    ),
                ],
            );
            let settings = load.config.unwrap().settings;
            assert_eq!(settings.ui_font_size, 13.);
            // later imports win over earlier ones
            assert_eq!(settings.tab_width, 222);
            assert_eq!(settings.default_title, "local");
            assert!(load.files.contains(&colors));
            assert!(load.files.contains(&dir.join("kuterm/local.jsonc")));
            assert!(load.warnings.is_empty(), "{:?}", load.warnings);
        });
    }

    #[test]
    fn nested_imports_resolve_from_the_config_folder() {
        with_dir("config_nested", |dir| {
            let load = load_with(
                dir,
                &[
                    ("settings.jsonc", r#"{"import": ["parts/a.jsonc"]}"#),
                    (
                        "parts/a.jsonc",
                        r#"{"import": ["parts/b.jsonc"], "tab_width": 101}"#,
                    ),
                    ("parts/b.jsonc", r#"{"tab_width": 99, "ui_font_size": 12}"#),
                ],
            );
            let settings = load.config.unwrap().settings;
            assert_eq!(settings.tab_width, 101);
            assert_eq!(settings.ui_font_size, 12.);
        });
    }

    #[test]
    fn missing_import_is_a_warning_and_watched() {
        with_dir("config_missing_import", |dir| {
            let load = load_with(
                dir,
                &[(
                    "settings.jsonc",
                    r#"{"import": ["nope.jsonc"], "tab_width": 77}"#,
                )],
            );
            assert_eq!(load.config.unwrap().settings.tab_width, 77);
            let missing = dir.join("kuterm/nope.jsonc");
            assert!(load.files.contains(&missing));
            assert_eq!(load.warnings.len(), 1);
            assert!(
                load.warnings[0].contains("not found"),
                "{:?}",
                load.warnings
            );
        });
    }

    #[test]
    fn import_cycles_are_cut_with_a_warning() {
        with_dir("config_cycle", |dir| {
            let load = load_with(
                dir,
                &[
                    (
                        "settings.jsonc",
                        r#"{"import": ["a.jsonc", "./settings.jsonc"]}"#,
                    ),
                    ("a.jsonc", r#"{"import": ["b.jsonc"], "tab_width": 101}"#),
                    ("b.jsonc", r#"{"import": ["a.jsonc"], "ui_font_size": 12}"#),
                ],
            );
            let settings = load.config.unwrap().settings;
            assert_eq!(settings.tab_width, 101);
            assert_eq!(settings.ui_font_size, 12.);
            // b back to a, and settings importing itself
            assert_eq!(load.warnings.len(), 2, "{:?}", load.warnings);
            assert!(load.warnings.iter().all(|w| w.contains("loop")));
        });
    }

    #[test]
    fn same_file_imported_twice_is_not_a_cycle() {
        with_dir("config_diamond", |dir| {
            let load = load_with(
                dir,
                &[
                    ("settings.jsonc", r#"{"import": ["a.jsonc", "b.jsonc"]}"#),
                    ("a.jsonc", r#"{"import": ["c.jsonc"]}"#),
                    ("b.jsonc", r#"{"import": ["c.jsonc"]}"#),
                    ("c.jsonc", r#"{"tab_width": 55}"#),
                ],
            );
            assert_eq!(load.config.unwrap().settings.tab_width, 55);
            assert!(load.warnings.is_empty(), "{:?}", load.warnings);
        });
    }

    #[test]
    fn import_must_be_a_list_of_paths() {
        for import in [r#""a.jsonc""#, "5", "[1]"] {
            with_dir("config_bad_import", |dir| {
                let main = format!(r#"{{"import": {import}}}"#);
                let load = load_with(dir, &[("settings.jsonc", &main)]);
                let error = load.config.unwrap_err();
                assert!(error.contains("invalid import"), "{error}");
                assert!(error.contains("settings.jsonc"), "{error}");
            });
        }
    }

    #[test]
    fn home_imports_start_from_home() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(resolve(Path::new("~/a/b.jsonc")), home.join("a/b.jsonc"));
        assert_eq!(
            resolve(Path::new("/abs.jsonc")),
            PathBuf::from("/abs.jsonc")
        );
    }

    #[test]
    fn errors_name_the_file_and_the_problem() {
        with_dir("config_errors", |dir| {
            // a bad value in an imported file names that file, not settings.jsonc
            let load = load_with(
                dir,
                &[
                    ("settings.jsonc", r#"{"import": ["bad.jsonc"]}"#),
                    ("bad.jsonc", r#"{"terminal": {"cursor_shape": "triangle"}}"#),
                ],
            );
            let error = load.config.unwrap_err();
            assert!(error.contains("bad.jsonc"), "{error}");
            assert!(error.contains("triangle"), "{error}");
            // the files are still known, so fixing them can be noticed
            assert!(load.files.contains(&dir.join("kuterm/bad.jsonc")));

            let load = load_with(dir, &[("bad.jsonc", "{\n  \"tab_width\": 5,,\n}")]);
            let error = load.config.unwrap_err();
            assert!(error.contains("bad.jsonc"), "{error}");
            assert!(error.contains("line 2"), "{error}");

            let load = load_with(dir, &[("bad.jsonc", r#"{"keybindings": {"x": "ctrl-x"}}"#)]);
            let error = load.config.unwrap_err();
            assert!(error.contains("keybindings"), "{error}");
            assert!(error.contains("unknown action"), "{error}");

            for json in ["[]", "5", "null"] {
                let load = load_with(dir, &[("settings.jsonc", json)]);
                let error = load.config.unwrap_err();
                assert!(error.contains("settings.jsonc"), "{error}");
            }
        });
    }

    #[test]
    fn os_block_for_this_os_goes_last() {
        let this = std::env::consts::OS;
        let other = if this == "linux" { "macos" } else { "linux" };
        let json = format!(
            r#"{{
                "ui_font_size": 13,
                "os": {{
                    "{this}": {{"ui_font_size": 14, "keybindings": {{"new_tab": "ctrl-alt-y"}}}},
                    "{other}": {{"tab_width": 1}},
                }},
            }}"#
        );
        let settings = Settings::parse(&json).unwrap();
        assert_eq!(settings.ui_font_size, 14.);
        assert_eq!(settings.tab_width, Settings::default().tab_width);
        assert_eq!(
            Keybindings::from_full(&combine(serde_json_lenient::from_str(&json).unwrap())).unwrap(),
            Keybindings::parse(r#"{"new_tab": "ctrl-alt-y"}"#).unwrap()
        );
    }

    #[test]
    fn user_keys_win_over_bundled_os_keys() {
        // the bundled os block is merged into the bundled values only, never over user ones
        let bundled: Value = serde_json_lenient::from_str(DEFAULT_SETTINGS).unwrap();
        for block in bundled["os"].as_object().unwrap().values() {
            let actions = block["keybindings"]
                .as_object()
                .into_iter()
                .flat_map(|keys| keys.keys());
            for action in actions {
                let keys = section(action, "ctrl-alt-f12".into());
                let full = combine(section("keybindings", keys));
                assert_eq!(full["keybindings"][action], "ctrl-alt-f12", "{action}");
            }
        }
    }

    #[test]
    fn watched_files_are_settings_and_imports() {
        with_dir("config_watch_list", |dir| {
            let load = load_with(
                dir,
                &[
                    ("settings.jsonc", r#"{"import": ["x.jsonc"]}"#),
                    // split files older versions wrote are not read
                    ("keybindings.jsonc", r#"{"new_tab": 5}"#),
                ],
            );
            let config = dir.join("kuterm");
            assert_eq!(
                load.files,
                [config.join("settings.jsonc"), config.join("x.jsonc")]
            );
            assert_eq!(load.config.unwrap(), Config::default());
        });
    }
}
