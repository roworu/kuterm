mod cli;
mod settings;
mod terminal;
mod theme;
mod ui;

use std::{borrow::Cow, sync::Arc};

use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use gpui_platform::application;

use crate::{
    cli::{Cli, USAGE},
    settings::{Commands, Keybindings, Pins, Settings, TabIcons},
    theme::Theme,
    ui::{
        text_input,
        workspace::{Hide, HideOthers, ShowAll, Workspace, app_menus},
    },
};

/// bundled regular face, also used to measure tab icons
pub(crate) const FONT_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/jetbrains/JetBrainsMonoNLNerdFontMono-Regular.ttf");

fn init(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!(
            "../assets/fonts/jetbrains/JetBrainsMonoNLNerdFontMono-Bold.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../assets/fonts/jetbrains/JetBrainsMonoNLNerdFontMono-BoldItalic.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../assets/fonts/jetbrains/JetBrainsMonoNLNerdFontMono-Italic.ttf"
        )),
        Cow::Borrowed(FONT_REGULAR),
    ];
    cx.text_system()
        .add_fonts(fonts)
        .expect("failed to load bundled fonts");

    let cli = Cli::get();
    if cli.recreate_confs {
        cli.remove_configs();
    }
    let mut settings = Settings::load();
    cli.apply(&mut settings);
    settings.use_installed_fonts(&cx.text_system().all_font_names());
    cx.set_global(settings);
    cx.set_global(TabIcons::load());
    cx.set_global(Commands::load());
    cx.set_global(Pins::load());
    Theme::apply(cx.window_appearance(), cx);
    cx.bind_keys(Keybindings::load().bindings());
    cx.bind_keys(text_input::bindings());
    // app wide, they work with no window focused
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.set_menus(app_menus());
}

fn main() {
    match Cli::parse(std::env::args_os().skip(1)) {
        Ok(cli) if cli.help => {
            println!("{USAGE}");
            return;
        }
        Ok(cli) => cli.init(),
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            std::process::exit(2);
        }
    }
    application().run(|cx: &mut App| {
        init(cx);
        cx.on_window_closed(|cx, _| cx.quit()).detach();

        let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(400.), px(250.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(Settings::get(cx).default_title.clone().into()),
                    ..Default::default()
                }),
                app_id: Some("kuterm".into()),
                icon: Some(Arc::new(
                    image::load_from_memory(include_bytes!("../assets/logo/icon_256.png"))
                        .expect("failed to load bundled icon")
                        .into_rgba8(),
                )),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
