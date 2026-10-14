mod cli;
mod remote;
mod settings;
mod terminal;
mod theme;
mod ui;

use std::borrow::Cow;

use futures::{StreamExt, channel::mpsc::unbounded};

use gpui::App;
use gpui_platform::application;

use crate::{
    cli::{Cli, USAGE},
    remote::Request,
    settings::{Config, ConfigFiles, Pins, RemoteControl, Settings, StatusBarState},
    theme::{PortalAppearance, Theme},
    ui::{
        text_input,
        windows::open_workspace_window,
        workspace::{
            Hide, HideOthers, NewTab, Quit, ShowAll, app_menus, dock_menu, open_urls, watch_config,
            with_workspace,
        },
    },
};

/// bundled regular face, also used to measure tab icons
pub(crate) const FONT_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/jetbrains/JetBrainsMonoNLNerdFontMono-Regular.ttf");

fn init(portal: PortalAppearance, cx: &mut App) {
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
    let load = Config::load();
    let mut problems = load.warnings;
    let config = load.config.unwrap_or_else(|error| {
        problems.push(format!("{error}, using defaults"));
        Config::default()
    });
    let mut settings = config.settings;
    cli.apply(&mut settings);
    if let Err(error) = Theme::check(&settings) {
        problems.push(format!("{error}, using the bundled theme"));
    }
    for problem in &problems {
        eprintln!("{problem}");
    }
    settings.use_installed_fonts(&cx.text_system().all_font_names());
    cx.set_cursor_hide_mode(settings.terminal.cursor_hide_mode());
    cx.set_global(settings);
    cx.set_global(config.tab_icons);
    cx.set_global(config.commands);
    cx.set_global(Pins::load());
    cx.set_global(StatusBarState::load());
    cx.set_global(ConfigFiles {
        paths: load.files,
        problems,
    });
    let appearance = portal.wait(cx);
    Theme::apply(appearance, cx);
    cx.bind_keys(config.keybindings.bindings());
    cx.bind_keys(text_input::bindings());
    if Settings::get(cx).remote_control == RemoteControl::Socket {
        #[cfg(unix)]
        if let Some(path) = remote::listen(cx, ui::windows::handle_request) {
            cx.set_global(ui::workspace::RemoteSocket(path));
        }
    }
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
    // after the keys are bound, the menus show them
    #[cfg(target_os = "linux")]
    if Settings::get(cx).window.global_menu {
        ui::workspace::start_global_menu(cx);
    }
}

fn main() {
    match Cli::parse(std::env::args_os().skip(1)) {
        Ok(cli) if cli.help => {
            println!("{USAGE}");
            return;
        }
        Ok(Cli {
            msg: Some(args), ..
        }) => {
            if args.iter().any(|arg| arg == "-h" || arg == "--help") {
                println!("{}", remote::MSG_USAGE);
                return;
            }
            std::process::exit(remote::run_msg(&args));
        }
        Ok(cli) => {
            // a running kuterm takes over, this process has nothing left to do
            if cli.single_instance {
                let request = Request::NewWindow {
                    cwd: std::env::current_dir().ok(),
                };
                if remote::send(&request).is_ok_and(|reply| reply.ok) {
                    return;
                }
            }
            cli.init()
        }
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            std::process::exit(2);
        }
    }
    // the color scheme is read while the platform starts
    let portal = PortalAppearance::read();
    let app = application();
    // folders dropped on the dock icon or opened with "open -a kuterm", they may come before
    // the window exists, so they wait in a channel
    let (urls_tx, mut urls_rx) = unbounded::<Vec<String>>();
    app.on_open_urls(move |urls| {
        urls_tx.unbounded_send(urls).ok();
    });
    app.run(move |cx: &mut App| {
        init(portal, cx);
        watch_config(cx).detach();
        // the app ends with its last window
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.spawn(async move |cx| {
            while let Some(urls) = urls_rx.next().await {
                cx.update(|cx| open_urls(&urls, cx));
            }
        })
        .detach();

        if open_workspace_window(None, cx).is_none() {
            std::process::exit(1);
        }
        cx.activate(true);
    });
}
