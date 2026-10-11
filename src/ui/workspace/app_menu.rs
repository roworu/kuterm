//! menu bar and dock menu, macos shows them, plus folders the system asks to open

use std::{
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
};

use gpui::{App, Menu, MenuItem, OsAction, SystemMenuType};

use super::{
    CloseTab, Hide, HideOthers, Minimize, NewTab, NextTab, OpenAbout, Quit, RunAction, ShowAll,
    ToggleCommandPalette, ToggleFullscreen, Zoom, with_workspace,
};
use crate::{
    settings::CommandAction,
    ui::terminal_view::{Copy, Paste},
};

/// menus of the app menu bar, items show the keys bound to their actions
pub fn app_menus() -> Vec<Menu> {
    let run = |name: &'static str, action: CommandAction| MenuItem::action(name, RunAction(action));
    vec![
        Menu::new("kuterm").items([
            MenuItem::action("About kuterm", OpenAbout),
            MenuItem::separator(),
            run("Settings…", CommandAction::OpenSettings),
            run("Reload All Configs", CommandAction::ReloadAll),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide kuterm", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit kuterm", Quit),
        ]),
        Menu::new("Shell").items([
            MenuItem::action("New Tab", NewTab),
            MenuItem::action("Close Tab", CloseTab),
            MenuItem::separator(),
            run("Split Right", CommandAction::SplitRight),
            run("Split Left", CommandAction::SplitLeft),
            run("Split Down", CommandAction::SplitDown),
            run("Split Up", CommandAction::SplitUp),
            MenuItem::separator(),
            run("Clear History", CommandAction::Clear),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action(
                "Select All",
                RunAction(CommandAction::SelectAll),
                OsAction::SelectAll,
            ),
        ]),
        Menu::new("View").items([
            MenuItem::action("Command Palette", ToggleCommandPalette),
            MenuItem::action("Toggle Full Screen", ToggleFullscreen),
            MenuItem::separator(),
            run("Bigger", CommandAction::IncreaseFontSize),
            run("Smaller", CommandAction::DecreaseFontSize),
            run("Default Font Size", CommandAction::ResetFontSize),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            run("Previous Tab", CommandAction::PrevTab),
            MenuItem::action("Next Tab", NextTab),
        ]),
    ]
}

/// items shown when right clicking the dock icon
pub fn dock_menu() -> Vec<MenuItem> {
    vec![MenuItem::action("New Tab", NewTab)]
}

/// open a tab in each folder the system asked to open, like one dropped on the dock icon
pub fn open_urls(urls: &[String], cx: &mut App) {
    let folders: Vec<PathBuf> = urls.iter().filter_map(|url| folder_of(url)).collect();
    if folders.is_empty() {
        return;
    }
    with_workspace(cx, |workspace, window, cx| {
        for folder in folders {
            workspace.open_folder(folder, window, cx);
        }
    });
}

// a file opens in its folder
fn folder_of(url: &str) -> Option<PathBuf> {
    let path = PathBuf::from(OsString::from_vec(percent_decode(
        url.strip_prefix("file://")?,
    )?));
    if path.is_dir() {
        Some(path)
    } else {
        path.parent().map(Path::to_path_buf)
    }
}

fn percent_decode(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        if byte == b'%' {
            let hex = std::str::from_utf8(rest.get(..2)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            rest = &rest[2..];
        } else {
            bytes.push(byte);
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_escapes_are_decoded() {
        assert_eq!(
            percent_decode("/Users/me/My%20Dir/%C3%A9t%C3%A9").unwrap(),
            "/Users/me/My Dir/été".as_bytes()
        );
        assert_eq!(percent_decode("/a%2"), None);
        assert_eq!(percent_decode("/a%zz"), None);
    }

    #[test]
    fn folder_urls_open_the_folder_and_file_urls_their_parent() {
        let dir = std::env::temp_dir().join(format!("kuterm url {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "").unwrap();
        let url = |path: &Path| format!("file://{}", path.display()).replace(' ', "%20");
        assert_eq!(folder_of(&format!("{}/", url(&dir))), Some(dir.clone()));
        assert_eq!(folder_of(&url(&file)), Some(dir.clone()));
        assert_eq!(folder_of("https://example.com"), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
