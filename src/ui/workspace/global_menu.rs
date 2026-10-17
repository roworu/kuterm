//! menus of `app_menus()` offered over dbus as a gtk menu model (`org.gtk.Menus` and
//! `org.gtk.Actions`), so global menu applets like plasma's show them in a panel.
//! applets find them through x11 window properties. kuterm sets those itself through the
//! window id gpui hands out, so gpui needs no hook. wayland is not done: plasma reads them
//! there through its `org_kde_kwin_appmenu` protocol, and gpui gives no way to bind it

use std::{collections::HashMap, time::Duration};

use futures::{StreamExt, channel::mpsc::UnboundedSender};
use gpui::{Action, App, AppContext, Global, KeyBinding, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use x11rb::{
    protocol::xproto::{ConnectionExt as _, PropMode},
    wrapper::ConnectionExt as _,
};
use zbus::zvariant::{OwnedValue, Signature, Value};

use super::{
    app_menus,
    menu_bar::{BarItem, bar_menus},
    with_workspace,
};

const MENUBAR_PATH: &str = "/org/kuterm/menubar";
const APP_PATH: &str = "/org/kuterm/app";
// how long setting the window properties waits for the window to exist, a second
const WINDOW_WAIT_TRIES: u32 = 100;
const WINDOW_WAIT_STEP: Duration = Duration::from_millis(10);

/// one entry of a gtk menu, sent as the `a{sv}` of `org.gtk.Menus`
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Item {
        label: String,
        /// name in `org.gtk.Actions`, without the "app." prefix
        action: String,
        accel: Option<String>,
    },
    /// the items of this menu, separators sit between sections
    Section(u32),
    Submenu {
        label: String,
        menu: u32,
    },
}

impl Entry {
    fn to_dbus(&self) -> DbusEntry {
        let mut entry = HashMap::new();
        match self {
            Entry::Item {
                label,
                action,
                accel,
            } => {
                entry.insert("label".into(), Value::from(label.clone()));
                entry.insert("action".into(), Value::from(format!("app.{action}")));
                if let Some(accel) = accel {
                    entry.insert("accel".into(), Value::from(accel.clone()));
                }
            }
            Entry::Section(menu) => {
                entry.insert(":section".into(), Value::from((0u32, *menu)));
            }
            Entry::Submenu { label, menu } => {
                entry.insert("label".into(), Value::from(label.clone()));
                entry.insert(":submenu".into(), Value::from((0u32, *menu)));
            }
        }
        entry
    }
}

/// menus of group 0 by id, 0 being the menu bar, and the action of each "itemN"
fn gtk_menus(
    accel: impl Fn(&dyn Action) -> Option<String>,
) -> (Vec<Vec<Entry>>, Vec<Box<dyn Action>>) {
    let mut model = vec![Vec::new()];
    let mut actions = Vec::new();
    for menu in bar_menus(app_menus()) {
        let id = model.len();
        model.push(Vec::new());
        model[0].push(Entry::Submenu {
            label: menu.name.to_string(),
            menu: id as u32,
        });
        let mut section = Vec::new();
        for item in menu.items.into_iter().chain([BarItem::Separator]) {
            match item {
                BarItem::Action { name, action } => {
                    section.push(Entry::Item {
                        label: name.to_string(),
                        action: format!("item{}", actions.len()),
                        accel: accel(action.as_ref()),
                    });
                    actions.push(action);
                }
                BarItem::Separator => {
                    let section_id = model.len() as u32;
                    model[id].push(Entry::Section(section_id));
                    model.push(std::mem::take(&mut section));
                }
            }
        }
    }
    (model, actions)
}

/// gtk accelerator of a single keystroke binding, like "<Control><Shift>t"
fn gtk_accel(binding: &KeyBinding) -> Option<String> {
    let [keystroke] = binding.keystrokes() else {
        return None;
    };
    let modifiers = keystroke.modifiers();
    let mut accel = String::new();
    for (held, name) in [
        (modifiers.control, "<Control>"),
        (modifiers.alt, "<Alt>"),
        (modifiers.shift, "<Shift>"),
        (modifiers.platform, "<Super>"),
    ] {
        if held {
            accel.push_str(name);
        }
    }
    let key = keystroke.key();
    // gtk names keys by their x11 keysym
    let name = match key {
        "enter" => "Return",
        "escape" => "Escape",
        "tab" => "Tab",
        "space" => "space",
        "backspace" => "BackSpace",
        "delete" => "Delete",
        "insert" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" => "Page_Up",
        "pagedown" => "Page_Down",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "-" => "minus",
        "=" => "equal",
        "+" => "plus",
        "," => "comma",
        "." => "period",
        "/" => "slash",
        "\\" => "backslash",
        ";" => "semicolon",
        "'" => "apostrophe",
        "`" => "grave",
        "[" => "bracketleft",
        "]" => "bracketright",
        _ if key.starts_with('f') && key[1..].parse::<u8>().is_ok() => {
            accel.push('F');
            &key[1..]
        }
        _ => key,
    };
    accel.push_str(name);
    Some(accel)
}

// the a{sv} of one entry, and a menu of a group as `Start` sends it
type DbusEntry = HashMap<String, Value<'static>>;
type DbusMenu = (u32, u32, Vec<DbusEntry>);

struct Menus(Vec<Vec<Entry>>);

#[zbus::interface(name = "org.gtk.Menus")]
impl Menus {
    /// every menu of the asked groups, there is only group 0
    fn start(&self, groups: Vec<u32>) -> Vec<DbusMenu> {
        if !groups.contains(&0) {
            return Vec::new();
        }
        self.0
            .iter()
            .enumerate()
            .map(|(id, entries)| (0, id as u32, entries.iter().map(Entry::to_dbus).collect()))
            .collect()
    }

    // the menus never change, so there are no subscriptions to drop
    fn end(&self, _groups: Vec<u32>) {}
}

struct Actions {
    count: usize,
    activated: UnboundedSender<usize>,
}

impl Actions {
    fn index(&self, action: &str) -> Option<usize> {
        let ix = action.strip_prefix("item")?.parse().ok()?;
        (ix < self.count).then_some(ix)
    }
}

// actions take no parameter and have no state
type Description = (bool, Signature, Vec<OwnedValue>);

#[zbus::interface(name = "org.gtk.Actions")]
impl Actions {
    fn list(&self) -> Vec<String> {
        (0..self.count).map(|ix| format!("item{ix}")).collect()
    }

    fn describe(&self, action: String) -> zbus::fdo::Result<Description> {
        self.index(&action)
            .map(|_| (true, Signature::Unit, Vec::new()))
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no action {action}")))
    }

    fn describe_all(&self) -> HashMap<String, Description> {
        self.list()
            .into_iter()
            .map(|name| (name, (true, Signature::Unit, Vec::new())))
            .collect()
    }

    fn activate(
        &self,
        action: String,
        _parameter: Vec<OwnedValue>,
        _platform_data: HashMap<String, OwnedValue>,
    ) {
        if let Some(ix) = self.index(&action) {
            self.activated.unbounded_send(ix).ok();
        }
    }

    fn set_state(
        &self,
        _action: String,
        _value: OwnedValue,
        _platform_data: HashMap<String, OwnedValue>,
    ) {
    }
}

/// the dbus connection serving the menus, kept open for the life of the app
struct GlobalMenu {
    connection: zbus::blocking::Connection,
}

impl Global for GlobalMenu {}

/// offer the menus over dbus, picked items run in the active window
pub fn start(cx: &mut App) {
    let (model, actions) = gtk_menus(|action| {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        keymap
            .bindings_for_action(action)
            .last()
            .and_then(gtk_accel)
    });
    let (activated, mut picks) = futures::channel::mpsc::unbounded();
    let actions_iface = Actions {
        count: actions.len(),
        activated,
    };
    // the session bus is local and answers fast, and the window properties need the bus
    // name before applets look at the first window
    let connection = zbus::blocking::connection::Builder::session()
        .and_then(|builder| builder.serve_at(MENUBAR_PATH, Menus(model)))
        .and_then(|builder| builder.serve_at(APP_PATH, actions_iface))
        .and_then(|builder| builder.build());
    let connection = match connection {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("global menu: no session bus: {error}");
            return;
        }
    };
    cx.set_global(GlobalMenu { connection });
    cx.spawn(async move |cx| {
        while let Some(ix) = picks.next().await {
            let action = actions[ix].boxed_clone();
            cx.update(|cx| {
                with_workspace(cx, |workspace, window, cx| {
                    workspace.run_menu_action(action, window, cx)
                })
            });
        }
    })
    .detach();
}

/// point global menu applets at the menus from this window, a no-op without the export
pub fn export_window(window: &Window, cx: &App) {
    let Some(menu) = cx.try_global::<GlobalMenu>() else {
        return;
    };
    let Some(bus_name) = menu.connection.unique_name() else {
        return;
    };
    let Ok(RawWindowHandle::Xcb(handle)) =
        HasWindowHandle::window_handle(window).map(|handle| handle.as_raw())
    else {
        return;
    };
    let (id, bus_name) = (handle.window.get(), bus_name.to_string());
    cx.background_spawn(async move {
        if let Err(error) = set_x11_properties(id, &bus_name) {
            eprintln!("global menu: {error}");
        }
    })
    .detach();
}

fn set_x11_properties(window: u32, bus_name: &str) -> anyhow::Result<()> {
    // gpui keeps its own connection to itself, a short lived one only sets the properties
    let (conn, _) = x11rb::connect(None)?;
    // the server orders nothing between connections, so gpui's request creating the window
    // may not be read yet
    let mut tries = 0;
    while conn.get_window_attributes(window)?.reply().is_err() {
        tries += 1;
        if tries == WINDOW_WAIT_TRIES {
            anyhow::bail!("window {window} never showed up");
        }
        std::thread::sleep(WINDOW_WAIT_STEP);
    }
    let utf8 = conn.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
    for (name, value) in [
        ("_GTK_UNIQUE_BUS_NAME", bus_name),
        ("_GTK_APPLICATION_OBJECT_PATH", APP_PATH),
        ("_GTK_MENUBAR_OBJECT_PATH", MENUBAR_PATH),
    ] {
        let atom = conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
        conn.change_property8(PropMode::REPLACE, window, atom, utf8, value.as_bytes())?
            .check()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::{NewTab, Quit};

    fn labels(model: &[Vec<Entry>], menu: u32) -> Vec<String> {
        model[menu as usize]
            .iter()
            .flat_map(|entry| match entry {
                Entry::Section(section) => labels(model, *section),
                Entry::Item { label, .. } | Entry::Submenu { label, .. } => vec![label.clone()],
            })
            .collect()
    }

    #[test]
    fn menu_bar_holds_one_submenu_per_app_menu_split_in_sections() {
        let (model, actions) = gtk_menus(|_| None);
        let bar = bar_menus(app_menus());
        assert_eq!(model[0].len(), bar.len());
        for (entry, menu) in model[0].iter().zip(&bar) {
            let Entry::Submenu { label, menu: id } = entry else {
                panic!("menu bar entry {entry:?} is not a submenu");
            };
            assert_eq!(label, menu.name.as_ref());
            // a submenu holds sections only, one more than its separators
            let separators = menu
                .items
                .iter()
                .filter(|item| matches!(item, BarItem::Separator))
                .count();
            let sections = &model[*id as usize];
            assert_eq!(sections.len(), separators + 1);
            assert!(sections.iter().all(|e| matches!(e, Entry::Section(_))));
            let names: Vec<String> = menu
                .items
                .iter()
                .filter_map(|item| match item {
                    BarItem::Action { name, .. } => Some(name.to_string()),
                    BarItem::Separator => None,
                })
                .collect();
            assert_eq!(labels(&model, *id), names);
        }
        // every item names its own action, in order
        let items: Vec<&str> = model
            .iter()
            .flatten()
            .filter_map(|entry| match entry {
                Entry::Item { action, .. } => Some(action.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(items.len(), actions.len());
        for (ix, name) in items.iter().enumerate() {
            assert_eq!(*name, format!("item{ix}"));
        }
        assert!(actions.iter().any(|action| action.partial_eq(&NewTab)));
        assert!(!labels(&model, 0).is_empty());
    }

    #[test]
    fn items_carry_the_accelerator_of_their_action() {
        let (model, actions) = gtk_menus(|action| {
            action
                .partial_eq(&NewTab)
                .then(|| "<Control><Shift>t".to_string())
        });
        let new_tab = actions.iter().position(|a| a.partial_eq(&NewTab)).unwrap();
        let quit = actions.iter().position(|a| a.partial_eq(&Quit)).unwrap();
        let item = |ix: usize| {
            model
                .iter()
                .flatten()
                .find(|e| matches!(e, Entry::Item { action, .. } if *action == format!("item{ix}")))
                .unwrap()
                .clone()
        };
        let Entry::Item { accel, .. } = item(new_tab) else {
            unreachable!()
        };
        assert_eq!(accel.as_deref(), Some("<Control><Shift>t"));
        let Entry::Item { accel, .. } = item(quit) else {
            unreachable!()
        };
        assert_eq!(accel, None);
        // over dbus the action gets the app prefix, sections and submenus point at group 0
        let dbus = item(new_tab).to_dbus();
        assert_eq!(
            dbus["action"],
            Value::from("app.item".to_string() + &new_tab.to_string())
        );
        assert_eq!(dbus["accel"], Value::from("<Control><Shift>t"));
        assert_eq!(
            Entry::Section(3).to_dbus()[":section"],
            Value::from((0u32, 3u32))
        );
        assert!(
            !Entry::Submenu {
                label: "a".into(),
                menu: 1
            }
            .to_dbus()
            .contains_key("action")
        );
    }

    #[test]
    fn keystrokes_become_gtk_accelerators() {
        let accel = |keys: &str| gtk_accel(&KeyBinding::new(keys, NewTab, None));
        assert_eq!(accel("ctrl-shift-t").as_deref(), Some("<Control><Shift>t"));
        assert_eq!(accel("alt-enter").as_deref(), Some("<Alt>Return"));
        assert_eq!(accel("super-pageup").as_deref(), Some("<Super>Page_Up"));
        assert_eq!(accel("ctrl-=").as_deref(), Some("<Control>equal"));
        assert_eq!(accel("ctrl--").as_deref(), Some("<Control>minus"));
        assert_eq!(accel("f11").as_deref(), Some("F11"));
        assert_eq!(accel("f").as_deref(), Some("f"));
        // gtk accelerators are one keystroke
        assert_eq!(accel("ctrl-a b"), None);
    }

    #[test]
    fn unknown_actions_are_refused() {
        let (activated, _picks) = futures::channel::mpsc::unbounded();
        let actions = Actions {
            count: 2,
            activated,
        };
        assert_eq!(actions.index("item1"), Some(1));
        assert_eq!(actions.index("item2"), None);
        assert_eq!(actions.index("quit"), None);
        assert!(actions.describe("item9".into()).is_err());
        assert_eq!(actions.describe_all().len(), 2);
    }

    #[test]
    fn start_sends_every_menu_of_group_zero_only() {
        let (model, _) = gtk_menus(|_| None);
        let count = model.len();
        let menus = Menus(model);
        // the only group there is, its menus numbered in order
        let sent = menus.start(vec![0]);
        assert_eq!(sent.len(), count);
        assert!(
            sent.iter()
                .enumerate()
                .all(|(ix, menu)| menu.0 == 0 && menu.1 == ix as u32)
        );
        assert_eq!(sent[0].2.len(), menus.0[0].len());
        assert!(menus.start(vec![1, 2]).is_empty());
    }
}
