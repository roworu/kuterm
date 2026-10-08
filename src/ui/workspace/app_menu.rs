//! menu bar, macos shows it at the top of the screen

use gpui::{Menu, MenuItem, OsAction, SystemMenuType};

use super::{
    CloseTab, Hide, HideOthers, Minimize, NewTab, NextTab, OpenAbout, Quit, ShowAll,
    ToggleCommandPalette, ToggleFullscreen, Zoom,
};
use crate::ui::terminal_view::{Copy, Paste};

/// menus of the app menu bar, items show the keys bound to their actions
pub fn app_menus() -> Vec<Menu> {
    vec![
        Menu::new("kuterm").items([
            MenuItem::action("About kuterm", OpenAbout),
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
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
        ]),
        Menu::new("View").items([
            MenuItem::action("Command Palette", ToggleCommandPalette),
            MenuItem::action("Toggle Full Screen", ToggleFullscreen),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Next Tab", NextTab),
        ]),
    ]
}
