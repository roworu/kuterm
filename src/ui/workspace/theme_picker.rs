//! picking a theme from the palette: moving through the list shows each one, enter keeps it
//! in settings.jsonc, escape goes back

use gpui::{AppContext, Context, DismissEvent, Window};

use super::Workspace;
use crate::{
    settings::{Command, Settings, edit},
    theme::{GALLERY, Theme},
    ui::dialogs::{CommandPalette, Highlighted},
};

impl Workspace {
    /// palette of the themes in settings and the bundled ones, previewed while moving
    pub(super) fn open_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = &Settings::get(cx).theme;
        let dark = Theme::is_dark(settings, self.appearance);
        let current = if dark {
            &settings.dark
        } else {
            &settings.light
        };
        let mut names: Vec<String> = settings.themes.keys().cloned().collect();
        names.extend(GALLERY.iter().map(|(name, _)| name.to_string()));
        names.sort();
        names.dedup();
        // the theme in use first, so enter right away keeps it
        if let Some(at) = current
            .as_ref()
            .and_then(|current| names.iter().position(|n| n == current))
        {
            let name = names.remove(at);
            names.insert(0, name);
        }
        let items = names
            .into_iter()
            .map(|name| Command::new(name, None, Vec::new()))
            .collect();
        let palette = cx.new(|cx| CommandPalette::picker(items, "pick a theme...", cx));
        self.open_overlay(&palette, window, cx);
        // escape or a click outside puts the theme from settings back. it replaces the plain
        // close, which drops the other subscriptions before they would run
        self._overlay_subscriptions =
            vec![
                cx.subscribe_in(&palette, window, |this, _, _: &DismissEvent, window, cx| {
                    this.close_overlay(window, cx);
                    this.reload_themes(window, cx);
                }),
            ];
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            move |_, _, picked: &Highlighted, window, cx| {
                let settings = &Settings::get(cx).theme;
                if let Ok(theme) = Theme::named(settings, &picked.0.name, dark) {
                    cx.set_global(theme);
                    window.refresh();
                }
            },
        ));
        self._overlay_subscriptions.push(cx.subscribe_in(
            &palette,
            window,
            move |this, _, picked: &Command, window, cx| {
                this.close_overlay(window, cx);
                this.keep_theme(&picked.name, dark, cx);
            },
        ));
        self.palette = Some(palette);
    }

    /// write `name` as the dark or light theme into settings.jsonc, its comments kept
    fn keep_theme(&mut self, name: &str, dark: bool, cx: &mut Context<Self>) {
        let Some(path) = Settings::path() else {
            return;
        };
        let slot = if dark { "dark" } else { "light" };
        let written = std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                edit::set_string(&text, &["theme", slot], name)
                    .ok_or_else(|| "it is no settings object".to_string())
            })
            .and_then(|text| std::fs::write(&path, text).map_err(|error| error.to_string()));
        if let Err(error) = written {
            self.show_error(
                format!("failed to save the theme to {}: {error}", path.display()),
                cx,
            );
        }
    }
}
