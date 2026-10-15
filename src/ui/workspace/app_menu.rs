//! menu bar and dock menu, macos shows them, plus folders the system asks to open

use std::path::{Path, PathBuf};

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
            run("Reload Config", CommandAction::ReloadConfig),
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

/// open a tab in each folder the system asked to open, like one dropped on the dock icon, and
/// one for each `kuterm://run` link with its command typed
pub fn open_urls(urls: &[String], cx: &mut App) {
    let folders: Vec<PathBuf> = urls.iter().filter_map(|url| folder_of(url)).collect();
    let links: Vec<RunLink> = urls.iter().filter_map(|url| parse_run_link(url)).collect();
    for (block, lines) in urls.iter().filter_map(|url| parse_block_link(url)) {
        reveal_block(block, lines, cx);
    }
    if folders.is_empty() && links.is_empty() {
        return;
    }
    with_workspace(cx, |workspace, window, cx| {
        for folder in folders {
            workspace.open_folder(folder, window, cx);
        }
        for link in links {
            workspace.open_run_link(link, window, cx);
        }
    });
}

/// block and 1 based output lines a `kuterm://block/<id>#L12-L20` link points at
pub fn parse_block_link(url: &str) -> Option<(u64, Option<(usize, usize)>)> {
    let rest = url.strip_prefix("kuterm://block/")?;
    let (id, lines) = rest.split_once('#').unwrap_or((rest, ""));
    let id = id.trim_end_matches('/').parse().ok()?;
    let line = |text: &str| text.strip_prefix('L')?.parse::<usize>().ok();
    let lines = match lines.split_once('-') {
        Some((first, last)) => Some((line(first)?, line(last)?)),
        None if lines.is_empty() => None,
        None => {
            let only = line(lines)?;
            Some((only, only))
        }
    };
    Some((id, lines))
}

/// switch to the tab that ran block `id`, in whichever window, and select its output lines
fn reveal_block(id: u64, lines: Option<(usize, usize)>, cx: &mut App) {
    for handle in cx.windows() {
        let Some(handle) = handle.downcast::<super::Workspace>() else {
            continue;
        };
        let found = handle
            .update(cx, |workspace, window, cx| {
                workspace.reveal_block(id, lines, window, cx)
            })
            .unwrap_or(false);
        if found {
            return;
        }
    }
}

/// a `kuterm://run?cwd=...&cmd=...` link: a command typed into a new tab, never run, so the
/// user reads it and presses enter
#[derive(Clone, Debug, PartialEq)]
pub struct RunLink {
    pub cwd: Option<PathBuf>,
    pub command: String,
}

/// the run link `url` is, none for other urls or one without a command. control characters
/// are taken out of the command, so a link can't press enter or send escapes
pub fn parse_run_link(url: &str) -> Option<RunLink> {
    let rest = url.strip_prefix("kuterm://run")?;
    let query = rest.strip_prefix('/').unwrap_or(rest).strip_prefix('?')?;
    let mut link = RunLink {
        cwd: None,
        command: String::new(),
    };
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode(&value.replace('+', " "))?;
        match key {
            "cmd" => {
                link.command = String::from_utf8_lossy(&value)
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect()
            }
            "cwd" => {
                link.cwd = path_from_bytes(value).filter(|path| path.is_absolute());
            }
            _ => {}
        }
    }
    (!link.command.trim().is_empty()).then_some(link)
}

// a file opens in its folder
fn folder_of(url: &str) -> Option<PathBuf> {
    let path = path_from_bytes(percent_decode(url.strip_prefix("file://")?)?)?;
    if path.is_dir() {
        Some(path)
    } else {
        path.parent().map(Path::to_path_buf)
    }
}

#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    Some(PathBuf::from(OsString::from_vec(bytes)))
}

// windows paths are not raw bytes, so only utf-8 ones can come from a url
#[cfg(not(unix))]
fn path_from_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    String::from_utf8(bytes).ok().map(PathBuf::from)
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
    fn block_links_name_a_block_and_its_lines() {
        assert_eq!(parse_block_link("kuterm://block/12"), Some((12, None)));
        assert_eq!(
            parse_block_link("kuterm://block/12#L3-L9"),
            Some((12, Some((3, 9))))
        );
        assert_eq!(
            parse_block_link("kuterm://block/7#L4"),
            Some((7, Some((4, 4))))
        );
        assert_eq!(parse_block_link("kuterm://block/x"), None);
        assert_eq!(parse_block_link("kuterm://block/1#3-4"), None);
    }

    #[test]
    fn run_links_carry_a_command_never_run() {
        let link = parse_run_link("kuterm://run?cwd=%2Ftmp%2Fa%20b&cmd=echo+hi%20there").unwrap();
        assert_eq!(link.cwd, Some(PathBuf::from("/tmp/a b")));
        assert_eq!(link.command, "echo hi there");
        // newlines, carriage returns and escapes are dropped, so nothing runs or moves
        let link = parse_run_link("kuterm://run/?cmd=ls%0Arm%20-rf%20x%0D%1B%5B2J").unwrap();
        assert_eq!(link.command, "lsrm -rf x[2J");
        // a relative folder is ignored
        assert_eq!(
            parse_run_link("kuterm://run?cwd=tmp&cmd=ls").unwrap().cwd,
            None
        );
        assert_eq!(parse_run_link("kuterm://run?cwd=/tmp"), None);
        assert_eq!(parse_run_link("kuterm://run?cmd=%0A%0D"), None);
        assert_eq!(parse_run_link("https://run?cmd=ls"), None);
        assert_eq!(parse_run_link("kuterm://other?cmd=ls"), None);
        assert_eq!(parse_run_link("kuterm://run?cmd=%zz"), None);
    }

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
