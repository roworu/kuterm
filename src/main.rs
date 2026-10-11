mod cli;
mod settings;
mod terminal;
mod theme;
mod ui;

use std::{borrow::Cow, sync::Arc};

use futures::{StreamExt, channel::mpsc::unbounded};

use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use gpui_platform::application;

use crate::{
    cli::{Cli, USAGE},
    settings::{Commands, Keybindings, Pins, Settings, TabIcons},
    theme::Theme,
    ui::{
        text_input,
        workspace::{
            Hide, HideOthers, NewTab, Quit, ShowAll, Workspace, app_menus, dock_menu, open_urls,
            with_workspace,
        },
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
    // the workspace handles these itself, these only run when no window is active, like
    // when it is minimized or the dock menu is used
    cx.on_action(|_: &Quit, cx| {
        with_workspace(cx, |workspace, window, cx| {
            workspace.request_quit(window, cx)
        })
    });
    cx.on_action(|_: &NewTab, cx| {
        with_workspace(cx, |workspace, window, cx| workspace.add_tab(window, cx))
    });
    cx.set_menus(app_menus());
    cx.set_dock_menu(dock_menu());
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
    let app = application();
    // folders dropped on the dock icon or opened with "open -a kuterm", they may come before
    // the window exists, so they wait in a channel
    let (urls_tx, mut urls_rx) = unbounded::<Vec<String>>();
    app.on_open_urls(move |urls| {
        urls_tx.unbounded_send(urls).ok();
    });
    app.run(move |cx: &mut App| {
        init(cx);
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.spawn(async move |cx| {
            while let Some(urls) = urls_rx.next().await {
                cx.update(|cx| open_urls(&urls, cx));
            }
        })
        .detach();

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
