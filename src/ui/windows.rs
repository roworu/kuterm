//! the windows of the app: opening workspace windows, plus answering remote control
//! requests about them

use std::{
    path::PathBuf,
    sync::{Arc, LazyLock},
};

use futures::channel::oneshot;
use gpui::{
    App, AppContext, Bounds, Context, EntityId, Pixels, TitlebarOptions, Window, WindowBounds,
    WindowHandle, WindowOptions, px, size,
};

use crate::{
    automation::{Reply, Request},
    settings::Settings,
    ui::workspace::{
        MovedTab, PaneMatch, Workspace, background_appearance, parse_run_link, startup_bounds,
        window_decorations, with_workspace,
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
    open_window(bounds, cx, |window, cx| {
        Workspace::with_start(folder, first, window, cx)
    })
}

/// open a window at `bounds` holding a tab moved out of another one
pub fn open_moved_tab_window(
    moved: MovedTab,
    bounds: Bounds<Pixels>,
    cx: &mut App,
) -> Option<WindowHandle<Workspace>> {
    open_window(bounds, cx, |window, cx| {
        Workspace::with_moved_tab(moved, window, cx)
    })
}

/// open a window at `bounds` showing the workspace `build` makes
fn open_window(
    bounds: Bounds<Pixels>,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut Context<Workspace>) -> Workspace + 'static,
) -> Option<WindowHandle<Workspace>> {
    cx.open_window(window_options(bounds, cx), |window, cx| {
        cx.new(|cx| build(window, cx))
    })
    .inspect_err(|error| eprintln!("failed to open window: {error:#}"))
    .ok()
}

fn window_options(bounds: Bounds<Pixels>, cx: &App) -> WindowOptions {
    let settings = Settings::get(cx);
    WindowOptions {
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
    }
}

/// the window and tab `spec` names: a tab name first, then a title, then an id from
/// list-tabs. without one, the active tab of the window in front
fn resolve_pane(spec: Option<&str>, cx: &mut App) -> Option<(WindowHandle<Workspace>, EntityId)> {
    let workspaces: Vec<WindowHandle<Workspace>> = cx
        .active_window()
        .and_then(|window| window.downcast::<Workspace>())
        .into_iter()
        .chain(
            cx.windows()
                .into_iter()
                .filter_map(|window| window.downcast()),
        )
        .collect();
    let Some(spec) = spec else {
        let handle = *workspaces.first()?;
        let id = handle.read(cx).ok()?.active_tab_id()?;
        return Some((handle, id));
    };
    [PaneMatch::Name, PaneMatch::Title, PaneMatch::Id]
        .into_iter()
        .find_map(|by| {
            workspaces.iter().find_map(|handle| {
                let id = handle.read(cx).ok()?.find_pane(spec, by, cx)?;
                Some((*handle, id))
            })
        })
}

/// answer a remote control request, now or once the user decided, through `reply`
pub fn handle_request(request: Request, reply: oneshot::Sender<Reply>, cx: &mut App) {
    let answer = match request {
        Request::NewTab { cwd, profile, name } => {
            let result = with_workspace(cx, |workspace, window, cx| {
                let result = workspace.open_tab(profile.as_deref(), cwd, window, cx);
                if result.is_ok()
                    && let Some(name) = name
                {
                    workspace.name_active_tab(name);
                }
                result
            });
            match result {
                Some(Ok(())) => Reply::done(),
                Some(Err(error)) => Reply::failed(error),
                None => Reply::failed("no window is open"),
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
        Request::OpenUrl { url } => {
            let Some(link) = parse_run_link(&url) else {
                reply
                    .send(Reply::failed(format!(
                        "not a kuterm://run link with a command: {url}"
                    )))
                    .ok();
                return;
            };
            match with_workspace(cx, |workspace, window, cx| {
                workspace.open_run_link(link, window, cx)
            }) {
                Some(()) => Reply::done(),
                None => Reply::failed("no window is open"),
            }
        }
        // the socket thread streams these itself
        Request::Subscribe { .. } => Reply::failed("subscribe is answered by the socket"),
        request => {
            let spec = request.pane().map(str::to_string);
            match resolve_pane(spec.as_deref(), cx) {
                Some((handle, id)) => {
                    // the workspace answers, maybe only after the user allowed it
                    let mut reply = Some(reply);
                    handle
                        .update(cx, |workspace, window, cx| {
                            if let Some(reply) = reply.take() {
                                workspace.remote(id, request, reply, window, cx);
                            }
                        })
                        .ok();
                    if let Some(reply) = reply {
                        reply.send(Reply::failed("the window is gone")).ok();
                    }
                    return;
                }
                None => match spec {
                    Some(spec) => Reply::failed(format!("no pane named {spec:?}")),
                    None => Reply::failed("no window is open"),
                },
            }
        }
    };
    reply.send(answer).ok();
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    /// what kuterm answers `request` with right away
    fn ask(request: Request, cx: &mut TestAppContext) -> Reply {
        let (tx, mut rx) = oneshot::channel();
        cx.update(|cx| handle_request(request, tx, cx));
        rx.try_recv().unwrap().expect("no reply yet")
    }

    #[gpui::test]
    fn open_url_refuses_what_is_no_run_link(cx: &mut TestAppContext) {
        // the starting process opens these itself when the running one says no
        for url in [
            "file:///tmp",
            "kuterm://run?cwd=%2Ftmp",
            "https://example.com",
        ] {
            let reply = ask(Request::OpenUrl { url: url.into() }, cx);
            assert!(!reply.ok, "{url} was taken");
            assert_eq!(
                reply.error.as_deref(),
                Some(format!("not a kuterm://run link with a command: {url}").as_str())
            );
        }
    }

    #[gpui::test]
    fn requests_without_a_window_fail_with_a_reason(cx: &mut TestAppContext) {
        let no_window = |request: Request, cx: &mut TestAppContext| {
            let reply = ask(request, cx);
            assert!(!reply.ok);
            reply.error.unwrap()
        };
        let new_tab = Request::NewTab {
            cwd: None,
            profile: None,
            name: None,
        };
        assert_eq!(no_window(new_tab, cx), "no window is open");
        let url = "kuterm://run?cmd=ls".to_string();
        assert_eq!(no_window(Request::OpenUrl { url }, cx), "no window is open");
        let focus = Request::FocusTab { pane: None };
        assert_eq!(no_window(focus, cx), "no window is open");
        let focus = Request::FocusTab {
            pane: Some("build".into()),
        };
        assert_eq!(no_window(focus, cx), "no pane named \"build\"");
        // listing works without windows, it just lists nothing
        let reply = ask(Request::ListTabs, cx);
        assert!(reply.ok && reply.tabs.is_empty());
    }
}
