//! command line options, they override config files and settings

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json_lenient::Value;

use crate::settings::{Pins, Settings, ThemeMode, WindowSize};

pub const USAGE: &str = "usage: kuterm [options]

options:
  --recreate-confs            delete config files and write defaults
  --config-file <path>        settings file to use instead of settings.jsonc
  --keybindings-file <path>   keybindings file, overrides the keybindings section
  --tab-icons-file <path>     tab icons file, overrides the tab_icons section
  --dark-theme-file <path>    dark theme file, overrides settings
  --light-theme-file <path>   light theme file, overrides settings
  --theme-mode <mode>         system, dark or light, overrides settings
  -h, --help                  print this help";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cli {
    pub help: bool,
    pub recreate_confs: bool,
    pub config_file: Option<PathBuf>,
    pub keybindings_file: Option<PathBuf>,
    pub tab_icons_file: Option<PathBuf>,
    pub dark_theme_file: Option<PathBuf>,
    pub light_theme_file: Option<PathBuf>,
    pub theme_mode: Option<ThemeMode>,
}

static CLI: OnceLock<Cli> = OnceLock::new();

impl Cli {
    /// parse arguments without the program name
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut cli = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let arg = arg.to_string_lossy().into_owned();
            let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "-h" | "--help" => cli.help = true,
                "--recreate-confs" => cli.recreate_confs = true,
                "--config-file" => cli.config_file = Some(absolute(value()?)?),
                "--keybindings-file" => cli.keybindings_file = Some(absolute(value()?)?),
                "--tab-icons-file" => cli.tab_icons_file = Some(absolute(value()?)?),
                "--dark-theme-file" => cli.dark_theme_file = Some(absolute(value()?)?),
                "--light-theme-file" => cli.light_theme_file = Some(absolute(value()?)?),
                "--theme-mode" => {
                    let mode = value()?.to_string_lossy().into_owned();
                    cli.theme_mode = Some(
                        serde_json_lenient::from_value(Value::String(mode.clone()))
                            .map_err(|_| format!("unknown theme mode {mode:?}"))?,
                    );
                }
                _ => return Err(format!("unknown option {arg:?}")),
            }
        }
        Ok(cli)
    }

    /// store options for the whole process, only the first call wins
    pub fn init(self) {
        let _ = CLI.set(self);
    }

    /// options given at startup, defaults when not set
    pub fn get() -> &'static Self {
        CLI.get_or_init(Self::default)
    }

    /// apply overrides to loaded settings
    pub fn apply(&self, settings: &mut Settings) {
        let theme = &mut settings.theme;
        if let Some(mode) = self.theme_mode {
            theme.mode = mode;
        }
        // absolute paths have a slash, so they are read as files, not theme names
        if let Some(path) = &self.dark_theme_file {
            theme.dark = Some(path.to_string_lossy().into_owned());
        }
        if let Some(path) = &self.light_theme_file {
            theme.light = Some(path.to_string_lossy().into_owned());
        }
    }

    /// delete config files, so loading them writes defaults again
    pub fn remove_configs(&self) {
        for path in [Settings::path(), Pins::path(), WindowSize::path()]
            .into_iter()
            .flatten()
        {
            if let Err(error) = std::fs::remove_file(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                eprintln!("failed to remove {}: {error}", path.display());
            }
        }
    }
}

// relative paths start from the current folder, not from the settings folder
fn absolute(path: OsString) -> Result<PathBuf, String> {
    std::path::absolute(&path).map_err(|error| format!("bad path {path:?}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Config;
    use crate::settings::tests::{temp_dir, with_config_home};

    const PATH_OPTIONS: [&str; 5] = [
        "--config-file",
        "--keybindings-file",
        "--tab-icons-file",
        "--dark-theme-file",
        "--light-theme-file",
    ];

    fn parse(args: &[&str]) -> Result<Cli, String> {
        Cli::parse(args.iter().map(OsString::from))
    }

    fn path_field(cli: &Cli, option: &str) -> Option<PathBuf> {
        match option {
            "--config-file" => cli.config_file.clone(),
            "--keybindings-file" => cli.keybindings_file.clone(),
            "--tab-icons-file" => cli.tab_icons_file.clone(),
            "--dark-theme-file" => cli.dark_theme_file.clone(),
            "--light-theme-file" => cli.light_theme_file.clone(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn no_args_give_defaults() {
        assert_eq!(parse(&[]).unwrap(), Cli::default());
    }

    #[test]
    fn short_and_long_help() {
        assert!(parse(&["-h"]).unwrap().help);
        assert!(parse(&["--help"]).unwrap().help);
        assert!(!parse(&[]).unwrap().help);
    }

    #[test]
    fn recreate_confs_flag_takes_no_value() {
        let cli = parse(&["--recreate-confs", "--theme-mode", "dark"]).unwrap();
        assert!(cli.recreate_confs);
        assert_eq!(cli.theme_mode, Some(ThemeMode::Dark));
    }

    #[test]
    fn each_path_option_keeps_absolute_path() {
        for option in PATH_OPTIONS {
            let cli = parse(&[option, "/abs/dir/file.jsonc"]).unwrap();
            assert_eq!(
                path_field(&cli, option),
                Some(PathBuf::from("/abs/dir/file.jsonc")),
                "{option}"
            );
            // other path options stay unset
            for other in PATH_OPTIONS.iter().filter(|o| **o != option) {
                assert_eq!(path_field(&cli, other), None, "{option} set {other}");
            }
        }
    }

    #[test]
    fn each_path_option_makes_relative_path_absolute_from_cwd() {
        let cwd = std::env::current_dir().unwrap();
        for option in PATH_OPTIONS {
            let cli = parse(&[option, "sub/file.jsonc"]).unwrap();
            let path = path_field(&cli, option).unwrap();
            assert!(path.is_absolute(), "{option}: {path:?}");
            assert_eq!(path, cwd.join("sub/file.jsonc"), "{option}");
        }
    }

    #[test]
    fn theme_mode_values() {
        for (value, mode) in [
            ("system", ThemeMode::System),
            ("dark", ThemeMode::Dark),
            ("light", ThemeMode::Light),
        ] {
            assert_eq!(
                parse(&["--theme-mode", value]).unwrap().theme_mode,
                Some(mode)
            );
        }
    }

    #[test]
    fn theme_mode_rejects_unknown_value() {
        assert!(parse(&["--theme-mode", "blue"]).is_err());
        assert!(parse(&["--theme-mode", ""]).is_err());
    }

    #[test]
    fn missing_value_is_error() {
        for option in PATH_OPTIONS.iter().chain(&["--theme-mode"]) {
            let err = parse(&[option]).unwrap_err();
            assert!(err.contains(option), "{option}: {err}");
        }
        // missing value after other valid options too
        assert!(parse(&["--recreate-confs", "--config-file"]).is_err());
    }

    #[test]
    fn unknown_option_is_error() {
        let err = parse(&["--nope"]).unwrap_err();
        assert!(err.contains("--nope"), "{err}");
        assert!(parse(&["positional"]).is_err());
        assert!(parse(&["-x"]).is_err());
        assert!(parse(&["--help", "--nope"]).is_err());
    }

    #[test]
    fn usage_lists_every_option() {
        for option in
            PATH_OPTIONS
                .iter()
                .chain(&["--recreate-confs", "--theme-mode", "-h", "--help"])
        {
            assert!(USAGE.contains(option), "usage misses {option}");
        }
    }

    #[test]
    fn all_options_together() {
        let cli = parse(&[
            "--theme-mode",
            "system",
            "--recreate-confs",
            "--light-theme-file",
            "/l.jsonc",
            "--config-file",
            "/s.jsonc",
        ])
        .unwrap();
        assert!(cli.recreate_confs);
        assert!(!cli.help);
        assert_eq!(cli.theme_mode, Some(ThemeMode::System));
        assert_eq!(cli.light_theme_file, Some("/l.jsonc".into()));
        assert_eq!(cli.config_file, Some("/s.jsonc".into()));
        assert_eq!(cli.dark_theme_file, None);
    }

    #[test]
    fn apply_without_overrides_keeps_settings() {
        let mut settings = Settings::default();
        Cli::default().apply(&mut settings);
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn apply_overrides_mode_and_both_theme_files() {
        let cli = parse(&[
            "--theme-mode",
            "light",
            "--dark-theme-file",
            "/d.jsonc",
            "--light-theme-file",
            "/l.jsonc",
        ])
        .unwrap();
        let mut settings = Settings::default();
        cli.apply(&mut settings);
        assert_eq!(settings.theme.mode, ThemeMode::Light);
        assert_eq!(settings.theme.dark.as_deref(), Some("/d.jsonc"));
        assert_eq!(settings.theme.light.as_deref(), Some("/l.jsonc"));
        // only theme is touched
        let expected = Settings {
            theme: settings.theme.clone(),
            ..Settings::default()
        };
        assert_eq!(settings, expected);
    }

    #[test]
    fn init_stores_the_first_options() {
        let cli = Cli {
            recreate_confs: true,
            ..Cli::default()
        };
        cli.clone().init();
        let stored = Cli::get();
        assert!(stored == &cli || stored == &Cli::default());
        Cli {
            help: true,
            ..Cli::default()
        }
        .init();
        assert_eq!(Cli::get(), stored);
    }

    #[test]
    fn recreate_removes_settings_and_pins() {
        let dir = temp_dir("cli_recreate_defaults");
        with_config_home(&dir, || {
            let files = [Settings::path().unwrap(), Pins::path().unwrap()];
            for file in &files {
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(file, "broken").unwrap();
            }
            let keep = dir.join("kuterm").join("keep.txt");
            std::fs::write(&keep, "keep").unwrap();

            parse(&["--recreate-confs"]).unwrap().remove_configs();
            for file in &files {
                assert!(!file.exists(), "{file:?} not removed");
            }
            assert!(keep.exists());

            // loading writes defaults again
            assert_eq!(Config::load().config, Ok(Config::default()));
            assert!(Settings::path().unwrap().exists());
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recreate_keeps_theme_files_from_the_command_line() {
        let dir = temp_dir("cli_recreate_overrides");
        with_config_home(&dir, || {
            let custom_light = dir.join("elsewhere").join("light.jsonc");
            std::fs::create_dir_all(custom_light.parent().unwrap()).unwrap();
            std::fs::write(&custom_light, "x").unwrap();

            let arg = custom_light.to_string_lossy().into_owned();
            parse(&["--light-theme-file", &arg])
                .unwrap()
                .remove_configs();
            // the user's own file, not one kuterm wrote
            assert!(custom_light.exists());
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recreate_with_nothing_to_remove_is_fine() {
        let dir = temp_dir("cli_recreate_empty");
        with_config_home(&dir, || {
            Cli::default().remove_configs();
            parse(&["--recreate-confs"]).unwrap().remove_configs();
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_configs_reports_unremovable_files() {
        let dir = temp_dir("cli_remove_error");
        with_config_home(&dir, || {
            let settings = Settings::path().unwrap();
            // a directory can't be removed as a file, so the error path runs
            std::fs::create_dir_all(&settings).unwrap();
            Cli {
                recreate_confs: true,
                ..Cli::default()
            }
            .remove_configs();
            assert!(settings.is_dir());
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
