//! reloading the config in every window when one of its files changes

use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

use futures::{StreamExt, channel::mpsc::unbounded};
use gpui::{App, Task};
use notify::Watcher as _;

use super::*;
use crate::{
    settings::{ConfigFiles, Settings},
    theme::Theme,
};

// how often config files are checked for changes when no file watcher could be made
const CONFIG_POLL: Duration = Duration::from_millis(250);
// editors save in steps, like truncate then write, so a change is read once it settles
const CONFIG_SETTLE: Duration = Duration::from_millis(100);

// config files and theme files in use, empty when the config was not loaded from files
fn watched_files(cx: &App) -> Vec<PathBuf> {
    let Some(files) = cx.try_global::<ConfigFiles>() else {
        return Vec::new();
    };
    let themes = Theme::files(Settings::get(cx));
    files.paths.iter().cloned().chain(themes).collect()
}

// modified time and size of each file, none for missing ones
fn file_stamps(paths: &[PathBuf]) -> Vec<Option<(SystemTime, u64)>> {
    paths
        .iter()
        .map(|path| {
            let meta = std::fs::metadata(path).ok()?;
            Some((meta.modified().ok()?, meta.len()))
        })
        .collect()
}

// folders holding `paths`, where the events of saving them show up: their own folder, sees
// editors that replace files on save, the folder a symlink points into, and for a missing
// folder the nearest existing one, which sees it being created
fn watch_folders(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut folders = Vec::new();
    for path in paths {
        let real = std::fs::canonicalize(path).ok();
        for path in std::iter::once(path).chain(real.as_ref()) {
            let mut folder = path.parent();
            while let Some(dir) = folder
                && !dir.is_dir()
            {
                folder = dir.parent();
            }
            if let Some(dir) = folder
                && !folders.iter().any(|known: &PathBuf| known == dir)
            {
                folders.push(dir.to_path_buf());
            }
        }
    }
    folders
}

/// reload the config in every window when one of its files changes, until the task drops.
/// one watcher serves all windows, a user gets only 128 inotify instances by default
pub fn watch_config(cx: &mut App) -> Task<()> {
    cx.spawn(async move |cx| {
        let (changed, mut changes) = unbounded();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<_>| {
            // reading the files, like a reload does, is no change
            if event.is_ok_and(|event: notify::Event| !event.kind.is_access()) {
                changed.unbounded_send(()).ok();
            }
        })
        .inspect_err(|error| eprintln!("config files are polled, no file watcher: {error}"))
        .ok();
        let mut watched = Vec::new();
        let mut last = None;
        loop {
            let paths = cx.update(|cx| watched_files(cx));
            // file stats may block on network folders. folders are watched before the
            // stats, so a change in between is not missed
            let (stamps, kept) = cx
                .background_executor()
                .spawn(async move {
                    if let Some(watcher) = &mut watcher {
                        let folders = watch_folders(&paths);
                        for folder in watched.iter().filter(|f| !folders.contains(*f)) {
                            watcher.unwatch(folder).ok();
                        }
                        for folder in folders.iter().filter(|f| !watched.contains(*f)) {
                            watcher
                                .watch(folder, notify::RecursiveMode::NonRecursive)
                                .ok();
                        }
                        watched = folders;
                    }
                    ((file_stamps(&paths), paths), (watcher, watched))
                })
                .await;
            (watcher, watched) = kept;
            if last.as_ref().is_some_and(|last| *last != stamps) {
                cx.background_executor().timer(CONFIG_SETTLE).await;
                cx.update(|cx| {
                    for handle in cx.windows() {
                        if let Some(handle) = handle.downcast::<Workspace>() {
                            handle
                                .update(cx, |this, window, cx| this.reload_config(window, cx))
                                .ok();
                        }
                    }
                });
                // the reload may watch other files, so stamps start over
                last = None;
                continue;
            }
            last = Some(stamps);
            if watcher.is_some() {
                if changes.next().await.is_none() {
                    break;
                }
                // one save makes several events, one look covers them all
                while changes.try_recv().is_ok() {}
            } else {
                cx.background_executor().timer(CONFIG_POLL).await;
            }
        }
    })
}

impl Workspace {
    /// read theme files again and recolor the ui and every terminal
    pub(super) fn reload_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let appearance = theme::appearance(window, cx);
        self.appearance = appearance;
        Theme::apply(appearance, cx);
        for tab in self.layout.panes().into_iter().flat_map(|pane| &pane.tabs) {
            let terminal = tab.view.read(cx).terminal().clone();
            terminal.update(cx, |terminal, cx| {
                terminal.apply_theme(appearance, &Settings::get(cx).theme)
            });
        }
        // terminal views may be cached, force redraw everything with new colors
        window.refresh();
    }

    /// read the config and theme files again and apply them, a broken file keeps the
    /// running config and shows what is wrong
    pub(super) fn reload_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let load = Config::load();
        // errors from an earlier try are fixed or shown again below
        self.notifications
            .retain(|notification| !notification.error);
        for warning in load.warnings {
            eprintln!("{warning}");
            self.show_error(warning, cx);
        }
        cx.set_global(ConfigFiles {
            paths: load.files,
            problems: Vec::new(),
        });
        let checked = load.config.and_then(|mut config| {
            Cli::get().apply(&mut config.settings);
            Theme::check(&config.settings)?;
            Ok(config)
        });
        let config = match checked {
            Ok(config) => config,
            Err(error) => {
                eprintln!("{error}");
                self.show_error(format!("{error}, keeping the last good config"), cx);
                return;
            }
        };
        let mut settings = config.settings;
        settings.use_installed_fonts(&cx.text_system().all_font_names());
        // switching the frame of an open window leaves wayland compositors with a stale
        // window geometry, so it only changes on restart
        settings.window.decorations = Settings::get(cx).window.decorations;
        cx.set_cursor_hide_mode(settings.terminal.cursor_hide_mode());
        for tab in self.layout.panes().into_iter().flat_map(|pane| &pane.tabs) {
            let terminal = tab.view.read(cx).terminal().clone();
            terminal.update(cx, |terminal, _| {
                terminal.apply_settings(&settings.terminal)
            });
        }
        cx.set_global(settings);
        cx.set_global(config.tab_icons);
        cx.set_global(config.commands);
        cx.clear_key_bindings();
        cx.bind_keys(config.keybindings.bindings());
        // text fields keep their editing keys, they are not part of the user keybindings
        cx.bind_keys(crate::ui::text_input::bindings());
        // menus show the keys bound to their actions, so they are rebuilt with the new keys
        cx.set_menus(app_menus());
        self.apply_window_settings(window, cx);
        // theme mode and theme files are picked in settings
        self.reload_themes(window, cx);
        self.refresh_titles();
        self.show_notification("config reloaded", None, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::test_support::*;

    #[gpui::test]
    fn reload_config_applies_every_section_and_keeps_the_last_good_one(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("workspace_reload");
        let config = dir.join("kuterm");
        std::fs::create_dir_all(config.join("themes")).unwrap();
        crate::settings::tests::with_config_home(&dir, || {
            let (ws, cx) = open(cx, 1);
            let reload = |cx: &mut VisualTestContext| {
                ws.update_in(cx, |ws, window, cx| ws.reload_config(window, cx));
                cx.run_until_parked();
            };
            let write = |name: &str, json: &str| std::fs::write(config.join(name), json).unwrap();
            let border = |cx: &mut VisualTestContext| cx.update(|_, cx| Theme::get(cx).border);
            let palette_enabled = |cx: &mut VisualTestContext| {
                cx.update(|_, cx| Settings::get(cx).command_palette.enable)
            };

            write(
                "settings.jsonc",
                r#"{
                    "keybindings": {"command_palette": "ctrl-shift-k"},
                    "commands": [{"name": "only", "actions": ["new_tab"]}],
                    "theme": {"mode": "dark", "dark": "themes/mine.jsonc"},
                }"#,
            );
            write("themes/mine.jsonc", r##"{"border": "#010203"}"##);
            reload(cx);
            assert_eq!(border(cx), gpui::rgb(0x010203).into());
            type_keys(cx, "ctrl-shift-p");
            assert!(!palette_open(&ws, cx));
            type_keys(cx, "ctrl-shift-k");
            assert_eq!(palette_names(&ws, cx), ["only"]);
            type_keys(cx, "escape");

            // theme files are read again too
            write("themes/mine.jsonc", r##"{"border": "#040506"}"##);
            reload(cx);
            assert_eq!(border(cx), gpui::rgb(0x040506).into());
            assert!(error_texts(&ws, cx).is_empty());

            // a broken settings file keeps everything as it was and tells what is wrong
            write(
                "settings.jsonc",
                r#"{"command_palette": {"enable": false}, "tab_width": "wide"}"#,
            );
            reload(cx);
            assert!(palette_enabled(cx));
            assert_eq!(border(cx), gpui::rgb(0x040506).into());
            let errors = error_texts(&ws, cx);
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(errors[0].contains("settings.jsonc"), "{errors:?}");
            assert!(errors[0].contains("wide"), "{errors:?}");

            // so does a broken theme file, the old error is replaced
            write(
                "settings.jsonc",
                r#"{"command_palette": {"enable": false},
                    "theme": {"mode": "dark", "dark": "themes/mine.jsonc"}}"#,
            );
            write("themes/mine.jsonc", r#"{"border": "nope"}"#);
            reload(cx);
            assert!(palette_enabled(cx));
            let errors = error_texts(&ws, cx);
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(errors[0].contains("mine.jsonc"), "{errors:?}");

            // fixed files apply and clear the error
            write("themes/mine.jsonc", r##"{"border": "#070809"}"##);
            reload(cx);
            assert!(!palette_enabled(cx));
            assert_eq!(border(cx), gpui::rgb(0x070809).into());
            assert!(error_texts(&ws, cx).is_empty());
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn reload_config_keeps_the_startup_decorations(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("workspace_reload_decorations");
        let config = dir.join("kuterm");
        std::fs::create_dir_all(&config).unwrap();
        crate::settings::tests::with_config_home(&dir, || {
            let settings = window_settings(r#"{"decorations": "server"}"#);
            let (ws, cx) = open_with_settings(cx, 1, "{}", settings);
            std::fs::write(
                config.join("settings.jsonc"),
                r#"{"tab_width": 101, "window": {"decorations": "client"}}"#,
            )
            .unwrap();
            ws.update_in(cx, |ws, window, cx| ws.reload_config(window, cx));
            cx.run_until_parked();
            let width = cx.update(|_, cx| Settings::get(cx).tab_width);
            assert_eq!(width, 101, "the rest of the file is applied");
            let decorations = cx.update(|_, cx| Settings::get(cx).window.decorations);
            assert_eq!(decorations, crate::settings::Decorations::Server);
            assert!(cx.debug_bounds("window-close").is_none());
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn reload_config_action_reloads(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("workspace_reload_action");
        let config = dir.join("kuterm");
        std::fs::create_dir_all(&config).unwrap();
        crate::settings::tests::with_config_home(&dir, || {
            let (ws, cx) = open(cx, 1);
            for (width, action) in [(101, "reload_config"), (102, "reload_config")] {
                std::fs::write(
                    config.join("settings.jsonc"),
                    format!(r#"{{"tab_width": {width}}}"#),
                )
                .unwrap();
                let json = format!(r#"{{"commands": [{{"name": "x", "actions": ["{action}"]}}]}}"#);
                let command = Commands::parse(&json).unwrap().commands.remove(0);
                ws.update_in(cx, |ws, window, cx| ws.run_command(&command, window, cx));
                cx.run_until_parked();
                assert_eq!(
                    cx.update(|_, cx| Settings::get(cx).tab_width),
                    width,
                    "{action}"
                );
            }
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn saved_config_files_are_reloaded(cx: &mut TestAppContext) {
        let dir = crate::settings::tests::temp_dir("workspace_watch");
        let config = dir.join("kuterm");
        std::fs::create_dir_all(&config).unwrap();
        crate::settings::tests::with_config_home(&dir, || {
            let (ws, cx) = open(cx, 1);
            let tab_width =
                |cx: &mut VisualTestContext| cx.update(|_, cx| Settings::get(cx).tab_width);
            // the watcher's events come from its own thread
            let reloads_to = |cx: &mut VisualTestContext, width: u32| {
                wait_until(cx, "config was not reloaded", |cx| tab_width(cx) == width)
            };
            let settings = config.join("settings.jsonc");
            std::fs::write(&settings, r#"{"tab_width": 123}"#).unwrap();
            cx.update(|_, cx| {
                cx.set_global(ConfigFiles {
                    paths: vec![settings.clone()],
                    problems: Vec::new(),
                })
            });
            let _watch = cx.update(|_, cx| watch_config(cx));
            pump(cx);

            std::fs::write(&settings, r#"{"tab_width": 124}"#).unwrap();
            reloads_to(cx, 124);

            std::fs::write(&settings, r#"{"tab_width": 1234}"#).unwrap();
            reloads_to(cx, 1234);

            // editors that save to a new file and rename it over the old one
            let saved = config.join("settings.jsonc.tmp");
            std::fs::write(&saved, r#"{"tab_width": 321}"#).unwrap();
            std::fs::rename(&saved, &settings).unwrap();
            reloads_to(cx, 321);

            // an import that shows up later is noticed, it is watched while missing
            std::fs::write(&settings, r#"{"import": ["later.jsonc"], "tab_width": 77}"#).unwrap();
            reloads_to(cx, 77);
            assert_eq!(error_texts(&ws, cx).len(), 1);
            std::fs::write(config.join("later.jsonc"), r#"{"ui_font_size": 13}"#).unwrap();
            wait_until(cx, "import was not read", |cx| {
                cx.update(|_, cx| Settings::get(cx).ui_font_size) == 13.
            });
            assert!(error_texts(&ws, cx).is_empty());

            // a file in a folder made later, the nearest existing folder is watched
            let nested = config.join("more/nested.jsonc");
            std::fs::write(
                &settings,
                r#"{"import": ["more/nested.jsonc"], "tab_width": 78}"#,
            )
            .unwrap();
            reloads_to(cx, 78);
            std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
            std::fs::write(&nested, r#"{"ui_font_size": 14}"#).unwrap();
            wait_until(cx, "nested import was not read", |cx| {
                cx.update(|_, cx| Settings::get(cx).ui_font_size) == 14.
            });
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
