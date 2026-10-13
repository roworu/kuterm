//! the windows of the app: opening workspace windows, plus answering remote control
//! requests about them

use std::{
    path::PathBuf,
    sync::{Arc, LazyLock},
};

use gpui::{App, AppContext, TitlebarOptions, WindowBounds, WindowHandle, WindowOptions, px, size};

use crate::{
    remote::{Reply, Request},
    settings::Settings,
    ui::workspace::{
        Workspace, background_appearance, startup_bounds, window_decorations, with_workspace,
    },
};

// decoded once, every new window shares it
static ICON: LazyLock<Arc<image::RgbaImage>> = LazyLock::new(|| {
    Arc::new(
        image::load_from_memory(include_bytes!("../../assets/logo/icon_256.png"))
            .expect("failed to load bundled icon")
            .into_rgba8(),
    )
});

/// open a workspace window, its first tab in `folder` when given
pub fn open_workspace_window(
    folder: Option<PathBuf>,
    cx: &mut App,
) -> Option<WindowHandle<Workspace>> {
    let bounds = startup_bounds(cx);
    // a folder tab spawns in the window like later tabs
    let first = folder
        .is_none()
        .then(|| Workspace::spawn_first_tab(bounds.size, cx))
        .flatten();
    let settings = Settings::get(cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_background: background_appearance(settings),
        window_decorations: Some(window_decorations(settings)),
        window_min_size: Some(size(px(400.), px(250.))),
        titlebar: Some(TitlebarOptions {
            title: Some(settings.default_title.clone().into()),
            ..Default::default()
        }),
        app_id: Some("kuterm".into()),
        icon: Some(ICON.clone()),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        cx.new(|cx| Workspace::with_start(folder, first, window, cx))
    }) {
        Ok(handle) => Some(handle),
        Err(error) => {
            eprintln!("failed to open window: {error:#}");
            None
        }
    }
}

/// answer a remote control request
pub fn handle_request(request: Request, cx: &mut App) -> Reply {
    match request {
        Request::NewTab { cwd, profile } => {
            let mut result = Err("no window is open".to_string());
            with_workspace(cx, |workspace, window, cx| {
                result = workspace.open_tab(profile.as_deref(), cwd, window, cx);
            });
            match result {
                Ok(()) => Reply::done(),
                Err(error) => Reply::failed(error),
            }
        }
        Request::SendText { text } => {
            let mut sent = false;
            with_workspace(cx, |workspace, _, cx| {
                workspace.send_text(&text, cx);
                sent = true;
            });
            if sent {
                Reply::done()
            } else {
                Reply::failed("no window is open")
            }
        }
        Request::ListTabs => {
            let mut tabs = Vec::new();
            for handle in cx.windows() {
                let Some(handle) = handle.downcast::<Workspace>() else {
                    continue;
                };
                let id = handle.window_id().as_u64();
                if let Ok(infos) = handle.read_with(cx, |workspace, cx| workspace.tab_infos(id, cx))
                {
                    tabs.extend(infos);
                }
            }
            Reply {
                ok: true,
                tabs,
                ..Reply::default()
            }
        }
        Request::NewWindow { cwd } => match open_workspace_window(cwd, cx) {
            Some(handle) => {
                handle
                    .update(cx, |_, window, _| window.activate_window())
                    .ok();
                cx.activate(true);
                Reply::done()
            }
            None => Reply::failed("failed to open a window"),
        },
    }
}
