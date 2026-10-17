//! menus of `app_menus()` drawn as a bar at the top of the window, for systems without a
//! global menu bar

use std::{cell::Cell, rc::Rc};

use gpui::{
    Action, App, Bounds, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    Menu, MenuItem, Modifiers, MouseButton, MouseDownEvent, Pixels, SharedString, Window, anchored,
    canvas, deferred, div, prelude::*, rems,
};

use super::{Hide, HideOthers, ShowAll, app_menus};
use crate::theme::Theme;

/// menu bar height in rems, as high as the tab bar
pub(super) const MENU_BAR_HEIGHT: f32 = 2.;

/// one item of a menu in the bar
pub(super) enum BarItem {
    Separator,
    Action {
        name: SharedString,
        action: Box<dyn Action>,
    },
}

/// one menu of the bar
pub(super) struct BarMenu {
    pub name: SharedString,
    pub items: Vec<BarItem>,
}

/// emitted with the action of the picked item
pub struct MenuPicked(pub Box<dyn Action>);

/// `menus` without the items only macos has, like hiding the app and the services menu
pub(super) fn bar_menus(menus: Vec<Menu>) -> Vec<BarMenu> {
    let mac_only: [&dyn Action; 3] = [&Hide, &HideOthers, &ShowAll];
    menus
        .into_iter()
        .map(|menu| {
            let mut items = Vec::new();
            for item in menu.items {
                match item {
                    MenuItem::Action { name, action, .. }
                        if !mac_only.iter().any(|skip| skip.partial_eq(action.as_ref())) =>
                    {
                        items.push(BarItem::Action { name, action })
                    }
                    // the separators around skipped items would pile up
                    MenuItem::Separator if matches!(items.last(), Some(BarItem::Action { .. })) => {
                        items.push(BarItem::Separator)
                    }
                    _ => {}
                }
            }
            if matches!(items.last(), Some(BarItem::Separator)) {
                items.pop();
            }
            BarMenu {
                name: menu.name,
                items,
            }
        })
        .collect()
}

/// keys bound to `action`, formatted like gpui shows them, none when unbound
fn key_text(action: &dyn Action, cx: &App) -> Option<String> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let binding = keymap.bindings_for_action(action).last()?;
    Some(
        binding
            .keystrokes()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// row of menu titles, a click opens a dropdown with the items and their keys
pub struct MenuBar {
    menus: Vec<BarMenu>,
    /// open menu, none while all are closed
    open: Option<usize>,
    /// highlighted item of the open menu
    selected: Option<usize>,
    /// held by the open dropdown, so a click on the bar alone takes no focus from the terminal
    focus_handle: FocusHandle,
    /// where the bar was drawn, a click there switches menus instead of closing them
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// alt is held with no other key yet, letting it go opens the first menu
    alt_tap: bool,
}

impl EventEmitter<DismissEvent> for MenuBar {}
impl EventEmitter<MenuPicked> for MenuBar {}

impl MenuBar {
    /// bar with the menus of `app_menus()`, all closed
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            menus: bar_menus(app_menus()),
            open: None,
            selected: None,
            focus_handle: cx.focus_handle(),
            bounds: Rc::default(),
            alt_tap: false,
        }
    }

    /// open menu `ix`, with its first item highlighted when opened from the keyboard
    fn open_menu(
        &mut self,
        ix: usize,
        select_first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open = Some(ix);
        self.selected = None;
        if select_first {
            self.step(true);
        }
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.selected = None;
        cx.emit(DismissEvent);
        cx.notify();
    }

    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(BarItem::Action { action, .. }) =
            self.open.and_then(|open| self.menus[open].items.get(ix))
        else {
            return;
        };
        let action = action.boxed_clone();
        self.open = None;
        self.selected = None;
        cx.emit(MenuPicked(action));
        cx.notify();
    }

    /// highlight the next or previous item of the open menu, wrapping around
    fn step(&mut self, forward: bool) {
        let Some(open) = self.open else {
            return;
        };
        let actions: Vec<usize> = self.menus[open]
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, BarItem::Action { .. }))
            .map(|(ix, _)| ix)
            .collect();
        if actions.is_empty() {
            return;
        }
        let at = self
            .selected
            .and_then(|selected| actions.iter().position(|&ix| ix == selected));
        let next = match (at, forward) {
            (None, true) => 0,
            (None, false) => actions.len() - 1,
            (Some(at), true) => (at + 1) % actions.len(),
            (Some(at), false) => at.checked_sub(1).unwrap_or(actions.len() - 1),
        };
        self.selected = Some(actions[next]);
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open else {
            return;
        };
        let count = self.menus.len();
        match event.keystroke.key.as_str() {
            "escape" => self.close(cx),
            "enter" => {
                if let Some(selected) = self.selected {
                    self.pick(selected, cx);
                }
            }
            "left" => self.open_menu((open + count - 1) % count, true, window, cx),
            "right" => self.open_menu((open + 1) % count, true, window, cx),
            "up" => self.step(false),
            "down" => self.step(true),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// alt pressed and let go with no other key in between opens the first menu
    pub fn modifiers_changed(
        &mut self,
        modifiers: &Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if *modifiers == Modifiers::alt() {
            self.alt_tap = true;
            return;
        }
        if self.alt_tap && !modifiers.modified() && self.open.is_none() {
            self.open_menu(0, true, window, cx);
        }
        self.alt_tap = false;
    }

    /// a key went down, so a held alt is part of a shortcut and opens nothing
    pub fn key_pressed(&mut self) {
        self.alt_tap = false;
    }

    fn render_dropdown(&self, menu: usize, theme: &Theme, cx: &Context<Self>) -> impl IntoElement {
        let items = self.menus[menu]
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| match item {
                BarItem::Separator => div().my_1().h_px().bg(theme.border).into_any_element(),
                BarItem::Action { name, action } => div()
                    .id(("menu-item", ix))
                    .debug_selector(move || format!("menu-item-{ix}"))
                    .flex()
                    .gap_6()
                    .px_3()
                    .py_1()
                    .whitespace_nowrap()
                    .when(self.selected == Some(ix), |item| {
                        item.bg(theme.tab_active_background)
                    })
                    .hover(|item| item.bg(theme.tab_active_background))
                    .child(name.clone())
                    .children(
                        key_text(action.as_ref(), cx)
                            .map(|keys| div().ml_auto().text_color(theme.text_muted).child(keys)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(ix, cx)))
                    .into_any_element(),
            })
            .collect::<Vec<_>>();
        let dropdown = div()
            .id("menu-dropdown")
            .debug_selector(|| "menu-dropdown".into())
            .track_focus(&self.focus_handle)
            .key_context("MenuBar")
            .on_key_down(cx.listener(Self::key_down))
            .flex()
            .flex_col()
            .min_w(rems(12.))
            .py_1()
            .bg(theme.tab_bar_background)
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .text_color(theme.text)
            // keeps clicks from reaching the terminal below
            .occlude()
            .children(items)
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if !this.bounds.get().contains(&event.position) {
                    this.close(cx);
                }
            }));
        div()
            .absolute()
            .top_full()
            .left_0()
            .child(deferred(anchored().snap_to_window().child(dropdown)))
    }
}

impl Focusable for MenuBar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MenuBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let titles = (0..self.menus.len())
            .map(|ix| {
                let open = self.open == Some(ix);
                div()
                    .id(("menu", ix))
                    .debug_selector(move || format!("menu-{ix}"))
                    .relative()
                    .h_full()
                    .flex()
                    .items_center()
                    .px_3()
                    .when(open, |title| title.bg(theme.tab_active_background))
                    .hover(|title| title.bg(theme.tab_active_background))
                    .child(self.menus[ix].name.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if this.open == Some(ix) {
                                this.close(cx);
                            } else {
                                this.open_menu(ix, false, window, cx);
                            }
                        }),
                    )
                    // with a menu open, moving along the bar opens the menu under the pointer
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        if *hovered && this.open.is_some_and(|open| open != ix) {
                            this.open_menu(ix, false, window, cx);
                        }
                    }))
                    .when(open, |title| {
                        title.child(self.render_dropdown(ix, theme, cx))
                    })
            })
            .collect::<Vec<_>>();
        let bounds = self.bounds.clone();
        // the workspace draws the row around it, which also holds the window buttons
        div()
            .id("menu-titles")
            .relative()
            .flex()
            .flex_none()
            .h_full()
            .text_color(theme.text)
            .children(titles)
            .child(
                canvas(move |bar, _, _| bounds.set(bar), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace::test_support::*;
    use crate::ui::workspace::{NewTab, Quit};

    fn names(menu: &BarMenu) -> Vec<&str> {
        menu.items
            .iter()
            .map(|item| match item {
                BarItem::Separator => "-",
                BarItem::Action { name, .. } => name.as_ref(),
            })
            .collect()
    }

    #[test]
    fn menus_come_from_app_menus_without_macos_items() {
        let app = app_menus();
        let bar = bar_menus(app_menus());
        assert_eq!(
            bar.iter().map(|menu| &menu.name).collect::<Vec<_>>(),
            app.iter().map(|menu| &menu.name).collect::<Vec<_>>()
        );
        for (bar, app) in bar.iter().zip(&app) {
            let names = names(bar);
            // no two separators in a row and none at the ends
            assert!(
                !names.windows(2).any(|pair| pair == ["-", "-"]),
                "{names:?}"
            );
            assert_ne!(names.first(), Some(&"-"));
            assert_ne!(names.last(), Some(&"-"));
            for item in &app.items {
                let MenuItem::Action { name, action, .. } = item else {
                    continue;
                };
                let mac_only = [&Hide as &dyn Action, &HideOthers, &ShowAll]
                    .iter()
                    .any(|skip| skip.partial_eq(action.as_ref()));
                assert_eq!(names.contains(&name.as_ref()), !mac_only, "{name}");
            }
        }
        let first = names(&bar[0]);
        assert!(!first.contains(&"Services"));
        // quitting and new tabs stay, with the actions of the app menus
        let action_of = |wanted: &dyn Action| {
            bar.iter().flat_map(|menu| &menu.items).any(
                |item| matches!(item, BarItem::Action { action, .. } if action.partial_eq(wanted)),
            )
        };
        assert!(action_of(&Quit) && action_of(&NewTab));
        assert!(!action_of(&Hide) && !action_of(&HideOthers) && !action_of(&ShowAll));
    }

    #[test]
    fn separators_around_skipped_items_collapse() {
        let menus = vec![Menu::new("m").items([
            MenuItem::separator(),
            MenuItem::action("a", NewTab),
            MenuItem::separator(),
            MenuItem::action("hide", Hide),
            MenuItem::separator(),
            MenuItem::action("b", Quit),
            MenuItem::separator(),
            MenuItem::action("show", ShowAll),
        ])];
        assert_eq!(names(&bar_menus(menus)[0]), ["a", "-", "b"]);
    }

    #[gpui::test]
    fn menu_bar_is_shown_only_when_asked(cx: &mut TestAppContext) {
        let (_, cx) = open_with_menu_bar(cx, "never");
        assert!(cx.debug_bounds("menu-bar").is_none());
        let (_, cx) = open_with_menu_bar(cx, "always");
        let bar = cx.debug_bounds("menu-bar").unwrap();
        // the panes start below it
        assert!(cx.debug_bounds("pane-terminal-0").unwrap().top() >= bar.bottom());
    }

    #[cfg(not(target_os = "macos"))]
    #[gpui::test]
    fn menu_bar_click_runs_the_item(cx: &mut TestAppContext) {
        let (ws, cx) = open_with_menu_bar(cx, "always");
        // "Shell" > "New Tab"
        let shell = cx.debug_bounds("menu-1").unwrap().center();
        click(cx, shell, 1);
        release(cx, shell);
        cx.run_until_parked();
        assert!(menu_bar_open(&ws, cx));
        let item = cx.debug_bounds("menu-item-0").unwrap().center();
        click(cx, item, 1);
        release(cx, item);
        cx.run_until_parked();
        assert!(!menu_bar_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 2);
        // focus is back on the terminal
        assert_eq!(current(&ws, cx), 1);
    }

    #[cfg(not(target_os = "macos"))]
    #[gpui::test]
    fn menu_bar_click_outside_closes_it(cx: &mut TestAppContext) {
        let (ws, cx) = open_with_menu_bar(cx, "always");
        let shell = cx.debug_bounds("menu-1").unwrap().center();
        click(cx, shell, 1);
        cx.run_until_parked();
        assert!(menu_bar_open(&ws, cx));
        // a click on the open title closes it too
        click(cx, shell, 1);
        cx.run_until_parked();
        assert!(!menu_bar_open(&ws, cx));
        click(cx, shell, 1);
        cx.run_until_parked();
        let terminal = cx.debug_bounds("pane-terminal-0").unwrap().center();
        click(cx, terminal, 1);
        release(cx, terminal);
        cx.run_until_parked();
        assert!(!menu_bar_open(&ws, cx));
        assert_eq!(views(&ws, cx).len(), 1);
        assert_eq!(current(&ws, cx), 0);
    }

    #[cfg(not(target_os = "macos"))]
    #[gpui::test]
    fn menu_bar_alt_and_arrows_run_an_item(cx: &mut TestAppContext) {
        let (ws, cx) = open_with_menu_bar(cx, "always");
        // alt alone opens the first menu, escape closes it again
        cx.simulate_modifiers_change(Modifiers::alt());
        cx.simulate_modifiers_change(Modifiers::none());
        cx.run_until_parked();
        assert!(menu_bar_open(&ws, cx));
        type_keys(cx, "escape");
        assert!(!menu_bar_open(&ws, cx));
        assert_eq!(current(&ws, cx), 0);
        // alt as part of a shortcut opens nothing
        cx.simulate_modifiers_change(Modifiers::alt());
        type_keys(cx, "alt-x");
        cx.simulate_modifiers_change(Modifiers::none());
        cx.run_until_parked();
        assert!(!menu_bar_open(&ws, cx));
        // "View" > "Command Palette"
        cx.simulate_modifiers_change(Modifiers::alt());
        cx.simulate_modifiers_change(Modifiers::none());
        type_keys(cx, "right right right down up enter");
        assert!(!menu_bar_open(&ws, cx));
        assert!(palette_open(&ws, cx));
    }

    #[cfg(not(target_os = "macos"))]
    #[gpui::test]
    fn a_menu_opened_by_mouse_steps_from_the_end_and_follows_the_pointer(cx: &mut TestAppContext) {
        let (ws, cx) = open_with_menu_bar(cx, "always");
        let state = |ws: &Entity<Workspace>, cx: &mut VisualTestContext| {
            ws.update(cx, |ws, cx| {
                let bar = ws.menu_bar.read(cx);
                (bar.open, bar.selected)
            })
        };
        let shell = cx.debug_bounds("menu-1").unwrap().center();
        click(cx, shell, 1);
        release(cx, shell);
        cx.run_until_parked();
        // opened by the mouse nothing is highlighted, up starts from the last item
        assert_eq!(state(&ws, cx), (Some(1), None));
        let last = ws.update(cx, |ws, cx| {
            let items = &ws.menu_bar.read(cx).menus[1].items;
            items
                .iter()
                .rposition(|item| matches!(item, BarItem::Action { .. }))
        });
        type_keys(cx, "up");
        assert_eq!(state(&ws, cx), (Some(1), last));
        // other keys leave the menu as it is
        type_keys(cx, "x");
        assert_eq!(state(&ws, cx), (Some(1), last));
        // moving along the bar with a menu open opens the one under the pointer
        let view = cx.debug_bounds("menu-3").unwrap().center();
        cx.simulate_event(MouseMoveEvent {
            position: view,
            pressed_button: None,
            modifiers: Modifiers::default(),
        });
        cx.run_until_parked();
        assert_eq!(state(&ws, cx), (Some(3), None));
        assert_eq!(views(&ws, cx).len(), 1);
    }
}
