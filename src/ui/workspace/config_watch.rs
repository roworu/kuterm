//! reloading the config in every window when one of its files changes

use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

use futures::{StreamExt, channel::mpsc::unbounded};
use gpui::{App, Task};
use notify::Watcher as _;

use super::Workspace;
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
