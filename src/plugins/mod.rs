//! plugins as processes: programs listed in "plugins" that kuterm starts and talks to over
//! stdio with json-rpc, framed like lsp. at `initialize` a plugin tells what it adds: palette
//! commands, link patterns, tab title blocks and panels (a tui of its own in a tab, so kuterm
//! needs no widget api). it asks kuterm for things with the same requests `kuterm msg` and
//! mcp send (`kuterm/request`), plus `set_title_block`, `decorate`, `open_panel` and
//! `notify`, and gets every event of the event bus while it may see panes.
//!
//! a crash or hang only kills its own process: kuterm never waits for a plugin, writes go
//! through a thread of their own, and a plugin that exits is started again after a growing
//! wait. nothing of a plugin runs on the render path. each plugin asks once for its
//! permissions, the answer is kept with a hash of the plugin.
//!
//! wasm plugins (P3) are left out until a real use needs them: a process can do all of the
//! above in any language.

pub mod framing;
pub mod permissions;
pub mod process;
pub mod spec;

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};

use futures::{
    StreamExt,
    channel::mpsc::{UnboundedSender, unbounded},
};
use gpui::{App, AsyncApp, Global};
use serde::Deserialize;
use serde_json_lenient::{Value, json};

use crate::{
    events_bus::EventBus,
    remote::{Reply, Request},
    settings::{Command, CommandAction, Inputs, PatternAction, Settings, TextPattern},
};
use permissions::TrustStore;
use process::{FromPlugin, PluginProcess, backoff};
use spec::{PluginEntry, PluginSpec};

// id of the initialize request, plugins' own requests get answers with their own ids
const INITIALIZE_ID: u64 = 0;
// a plugin up this long counts as healthy again, its next crash restarts it right away
const HEALTHY_AFTER: Duration = Duration::from_secs(60);

/// what a plugin said it adds, in its answer to `initialize`
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Capabilities {
    pub commands: Vec<PluginCommand>,
    /// names of tab title blocks it keeps up to date with `set_title_block`
    pub title_blocks: Vec<String>,
    pub patterns: Vec<PluginPattern>,
    pub panels: Vec<Panel>,
}

/// a palette command of a plugin
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PluginCommand {
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
}

/// a link pattern whose clicks go to the plugin
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PluginPattern {
    pub name: String,
    pub regex: String,
}

/// a tui the plugin brings, opened in a tab of its own
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Panel {
    pub name: String,
    pub command: Vec<String>,
}

/// where a plugin is
#[derive(Clone, Debug, PartialEq)]
pub enum PluginState {
    /// asked about, the user did not answer yet
    Untrusted,
    Running,
    /// waits to start again after it exited
    Restarting,
    /// will not run, with why
    Failed(String),
}

struct Plugin {
    spec: PluginSpec,
    state: PluginState,
    /// the process of this generation is the live one, messages of older ones are dropped
    generation: u64,
    process: Option<PluginProcess>,
    capabilities: Capabilities,
    /// title block text by block name, and by block name and pane for one pane only
    blocks: HashMap<String, String>,
    pane_blocks: HashMap<(String, u64), String>,
    /// restarts in a row since it last ran healthy
    attempts: u32,
    started: Instant,
}

/// the running plugins of the app
pub struct PluginHost {
    plugins: Vec<Plugin>,
    tx: UnboundedSender<(u64, FromPlugin)>,
    trust: TrustStore,
    next_generation: u64,
}

impl Global for PluginHost {}

/// the plugin specs settings name, git ones fetched first. problems are told on stderr
fn specs(entries: &[PluginEntry]) -> Vec<Result<PluginSpec, String>> {
    let plugins_dir = data_dir().map(|dir| dir.join("plugins"));
    entries
        .iter()
        .map(|entry| match entry {
            PluginEntry::Local(spec) => Ok(spec.clone()),
            PluginEntry::Git(text) => {
                let source = spec::parse_git(text)
                    .ok_or_else(|| format!("{text:?} is no \"github:user/repo[@rev]\""))?;
                let dir = plugins_dir.as_deref().ok_or("no data folder for plugins")?;
                spec::fetch(&source, dir)
            }
        })
        .collect()
}

fn data_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
        })?;
    Some(dir.join("kuterm"))
}

/// a json-rpc notification
fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

/// start the plugins of settings and the task that hears from them
pub fn start(cx: &mut App) {
    let (tx, mut rx) = unbounded::<(u64, FromPlugin)>();
    cx.set_global(PluginHost {
        plugins: Vec::new(),
        tx,
        trust: TrustStore::load(),
        next_generation: 1,
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Some((generation, message)) = rx.next().await {
            cx.update(|cx| handle(generation, message, cx));
        }
    })
    .detach();
    // plugins that may see panes get every event, as `kuterm msg subscribe` prints them
    let mut events = EventBus::subscribe();
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Some(event) = events.next().await {
            let Ok(params) = serde_json_lenient::to_value(&event) else {
                continue;
            };
            cx.update(|cx| {
                let message = notification("event", params);
                for plugin in &cx.global::<PluginHost>().plugins {
                    if plugin.spec.permissions.panes
                        && let Some(process) = &plugin.process
                    {
                        process.send(&message);
                    }
                }
            });
        }
    })
    .detach();
    reload(cx);
}

/// stop every plugin and start the ones settings list now, fetching git ones off the main
/// thread
pub fn reload(cx: &mut App) {
    if !cx.has_global::<PluginHost>() {
        return;
    }
    cx.global_mut::<PluginHost>().plugins.clear();
    let entries = Settings::get(cx).plugins.clone();
    if entries.is_empty() {
        return;
    }
    cx.spawn(async move |cx: &mut AsyncApp| {
        let specs = cx
            .background_executor()
            .spawn(async move { specs(&entries) })
            .await;
        cx.update(|cx| {
            for spec in specs {
                match spec {
                    Ok(spec) => add(spec, cx),
                    Err(error) => eprintln!("plugin: {error}"),
                }
            }
        });
    })
    .detach();
}

/// add a plugin, running it when it is trusted as it is, else asking first
fn add(spec: PluginSpec, cx: &mut App) {
    let host = cx.global_mut::<PluginHost>();
    let trusted = host.trust.trusts(&spec.name, &spec.hash());
    host.plugins.push(Plugin {
        spec,
        state: PluginState::Untrusted,
        generation: 0,
        process: None,
        capabilities: Capabilities::default(),
        blocks: HashMap::new(),
        pane_blocks: HashMap::new(),
        attempts: 0,
        started: Instant::now(),
    });
    let ix = host.plugins.len() - 1;
    if trusted {
        run(ix, cx);
    } else {
        ask_trust(ix, cx);
    }
}

/// ask once whether plugin `ix` may run with its permissions, in a window when one is open
fn ask_trust(ix: usize, cx: &mut App) {
    let plugin = &cx.global::<PluginHost>().plugins[ix];
    let (name, hash) = (plugin.spec.name.clone(), plugin.spec.hash());
    let text = format!(
        "plugin \"{name}\" wants to run, it may {}",
        plugin.spec.permissions.describe()
    );
    let command = plugin.spec.command.join(" ");
    cx.spawn(async move |cx: &mut AsyncApp| {
        // the first window may still be opening at startup
        for _ in 0..100 {
            let asked = cx.update(|cx| {
                let mut asked = false;
                crate::ui::workspace::with_workspace(cx, |workspace, _, cx| {
                    let (name, hash) = (name.clone(), hash.clone());
                    workspace.ask_plugin_trust(
                        text.clone(),
                        command.clone(),
                        move |cx| {
                            let host = cx.global_mut::<PluginHost>();
                            host.trust.trust(&name, &hash);
                            host.trust.save();
                            if let Some(ix) = host.plugins.iter().position(|p| p.spec.name == name)
                            {
                                run(ix, cx);
                            }
                        },
                        cx,
                    );
                    asked = true;
                });
                asked
            });
            if asked {
                return;
            }
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
        }
    })
    .detach();
}

/// start plugin `ix` and ask what it adds
fn run(ix: usize, cx: &mut App) {
    let host = cx.global_mut::<PluginHost>();
    let generation = host.next_generation;
    host.next_generation += 1;
    let tx = host.tx.clone();
    let plugin = &mut host.plugins[ix];
    plugin.generation = generation;
    plugin.started = Instant::now();
    match PluginProcess::spawn(&plugin.spec, generation, tx) {
        Ok(process) => {
            process.send(&json!({
                "jsonrpc": "2.0",
                "id": INITIALIZE_ID,
                "method": "initialize",
                "params": {
                    "kuterm": env!("CARGO_PKG_VERSION"),
                    "permissions": plugin.spec.permissions,
                },
            }));
            plugin.process = Some(process);
            plugin.state = PluginState::Running;
        }
        Err(error) => {
            eprintln!("plugin {}: {error}", plugin.spec.name);
            plugin.state = PluginState::Failed(error.to_string());
        }
    }
}

/// the plugin running as `generation`
fn find(generation: u64, cx: &App) -> Option<usize> {
    cx.global::<PluginHost>()
        .plugins
        .iter()
        .position(|plugin| plugin.generation == generation && plugin.process.is_some())
}

fn handle(generation: u64, message: FromPlugin, cx: &mut App) {
    let Some(ix) = find(generation, cx) else {
        return;
    };
    match message {
        FromPlugin::Message(message) => handle_message(ix, message, cx),
        FromPlugin::Broken(why) => {
            let name = &cx.global::<PluginHost>().plugins[ix].spec.name;
            eprintln!("plugin {name} sent a broken message: {why}");
        }
        FromPlugin::Exited => exited(ix, cx),
    }
}

/// a plugin ended, it starts again after a wait growing with each crash in a row
fn exited(ix: usize, cx: &mut App) {
    let plugin = &mut cx.global_mut::<PluginHost>().plugins[ix];
    plugin.process = None;
    if plugin.started.elapsed() >= HEALTHY_AFTER {
        plugin.attempts = 0;
    }
    let wait = backoff(plugin.attempts);
    plugin.attempts += 1;
    plugin.state = PluginState::Restarting;
    plugin.capabilities = Capabilities::default();
    let (name, generation) = (plugin.spec.name.clone(), plugin.generation);
    eprintln!("plugin {name} exited, starting it again in {wait:?}");
    cx.spawn(async move |cx: &mut AsyncApp| {
        cx.background_executor().timer(wait).await;
        cx.update(|cx| {
            // a reload in the meantime replaced it
            let host = cx.global::<PluginHost>();
            if let Some(ix) = host.plugins.iter().position(|plugin| {
                plugin.generation == generation && plugin.state == PluginState::Restarting
            }) {
                run(ix, cx);
            }
        });
    })
    .detach();
}

/// reply to request `id` of plugin `ix`
fn respond(ix: usize, id: Value, result: Result<Value, String>, cx: &App) {
    let message = match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32000, "message": error},
        }),
    };
    if let Some(process) = &cx.global::<PluginHost>().plugins[ix].process {
        process.send(&message);
    }
}

fn handle_message(ix: usize, message: Value, cx: &mut App) {
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // the answer to initialize tells what the plugin adds
        if id.as_ref().and_then(Value::as_u64) == Some(INITIALIZE_ID)
            && let Some(result) = message.get("result")
        {
            let capabilities =
                serde_json_lenient::from_value(result.clone()).unwrap_or_else(|error| {
                    eprintln!("plugin capabilities: {error}");
                    Capabilities::default()
                });
            cx.global_mut::<PluginHost>().plugins[ix].capabilities = capabilities;
            cx.refresh_windows();
        }
        return;
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let permissions = cx.global::<PluginHost>().plugins[ix].spec.permissions;
    let answer = |result: Result<Value, String>, cx: &App| {
        if let Some(id) = id.clone() {
            respond(ix, id, result, cx);
        }
    };
    match method {
        "kuterm/request" => {
            let request: Request = match serde_json_lenient::from_value(params) {
                Ok(request) => request,
                Err(error) => return answer(Err(format!("bad request: {error}")), cx),
            };
            if let Some(why) = permissions.refuse(&request) {
                return answer(Err(why), cx);
            }
            let (tx, rx) = futures::channel::oneshot::channel::<Reply>();
            crate::ui::windows::handle_request(request, tx, cx);
            // the reply may wait for the user, like an allowed run, so it is sent when it comes
            cx.spawn(async move |cx: &mut AsyncApp| {
                let reply = rx.await.unwrap_or_else(|_| Reply::failed("no answer"));
                cx.update(|cx| {
                    if let Some(id) = id {
                        let result =
                            serde_json_lenient::to_value(&reply).map_err(|error| error.to_string());
                        respond(ix, id, result, cx);
                    }
                });
            })
            .detach();
        }
        "set_title_block" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let text = params
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let pane = params.get("pane").and_then(Value::as_u64);
            let plugin = &mut cx.global_mut::<PluginHost>().plugins[ix];
            match pane {
                Some(pane) => plugin
                    .pane_blocks
                    .insert((name.to_string(), pane), text.to_string()),
                None => plugin.blocks.insert(name.to_string(), text.to_string()),
            };
            crate::ui::workspace::refresh_all_titles(cx);
            answer(Ok(Value::Null), cx);
        }
        "decorate" => {
            if !permissions.panes {
                return answer(Err("the plugin has no \"panes\" permission".into()), cx);
            }
            let result = crate::ui::windows::decorate_block(&params, cx);
            answer(result.map(|()| Value::Null), cx);
        }
        "open_panel" => {
            if !permissions.run {
                return answer(Err("the plugin has no \"run\" permission".into()), cx);
            }
            let command: Vec<String> = params
                .get("command")
                .and_then(|command| serde_json_lenient::from_value(command.clone()).ok())
                .unwrap_or_default();
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string);
            answer(open_panel(command, name, cx), cx);
        }
        "notify" => {
            let text = params
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let text = format!(
                "{}: {text}",
                cx.global::<PluginHost>().plugins[ix].spec.name
            );
            crate::ui::workspace::with_workspace(cx, |workspace, _, cx| {
                workspace.show_plugin_notice(text, cx)
            });
            answer(Ok(Value::Null), cx);
        }
        other => answer(Err(format!("unknown method {other:?}")), cx),
    }
}

/// open a tab running `command`, the tui of a plugin
fn open_panel(command: Vec<String>, name: Option<String>, cx: &mut App) -> Result<Value, String> {
    let Some((program, args)) = command.split_first() else {
        return Err("open_panel needs a command".into());
    };
    let (program, args) = (program.clone(), args.to_vec());
    let mut opened = false;
    crate::ui::workspace::with_workspace(cx, |workspace, window, cx| {
        workspace.open_panel(program, args, name, window, cx);
        opened = true;
    });
    if opened {
        Ok(Value::Null)
    } else {
        Err("no window is open".into())
    }
}

/// palette commands of every running plugin, and one per panel
pub fn commands(cx: &App) -> Vec<Command> {
    let Some(host) = cx.try_global::<PluginHost>() else {
        return Vec::new();
    };
    let mut commands = Vec::new();
    for plugin in host.plugins.iter().filter(|p| p.process.is_some()) {
        let name = &plugin.spec.name;
        let action = |command: &str| {
            vec![
                CommandAction::Plugin {
                    plugin: name.clone(),
                    command: command.to_string(),
                }
                .into(),
            ]
        };
        for command in &plugin.capabilities.commands {
            commands.push(Command {
                name: command.name.clone(),
                category: Some(command.category.clone().unwrap_or_else(|| name.clone())),
                pinned: false,
                inputs: Inputs::new(),
                actions: action(&command.name),
            });
        }
        for panel in &plugin.capabilities.panels {
            commands.push(Command {
                name: format!("open {}", panel.name),
                category: Some(name.clone()),
                pinned: false,
                inputs: Inputs::new(),
                actions: action(&format!("panel:{}", panel.name)),
            });
        }
    }
    commands
}

/// a palette command of `plugin` was picked with pane `pane` active
pub fn run_command(plugin: &str, command: &str, pane: Option<u64>, cx: &mut App) {
    let Some(host) = cx.try_global::<PluginHost>() else {
        return;
    };
    let Some(found) = host.plugins.iter().find(|p| p.spec.name == plugin) else {
        return;
    };
    if let Some(panel) = command.strip_prefix("panel:") {
        let Some(panel) = found.capabilities.panels.iter().find(|p| p.name == panel) else {
            return;
        };
        if found.spec.permissions.run {
            open_panel(panel.command.clone(), Some(panel.name.clone()), cx).ok();
        }
        return;
    }
    if let Some(process) = &found.process {
        process.send(&notification(
            "command",
            json!({"name": command, "pane": pane}),
        ));
    }
}

/// link patterns of every running plugin, their clicks go back to it
pub fn patterns(cx: &App) -> Vec<TextPattern> {
    let Some(host) = cx.try_global::<PluginHost>() else {
        return Vec::new();
    };
    host.plugins
        .iter()
        .filter(|plugin| plugin.process.is_some())
        .flat_map(|plugin| {
            plugin
                .capabilities
                .patterns
                .iter()
                .map(|pattern| TextPattern {
                    name: Some(pattern.name.clone()),
                    regex: pattern.regex.clone(),
                    action: PatternAction::Plugin(plugin.spec.name.clone()),
                })
        })
        .collect()
}

/// a link of pattern `name` of `plugin` was clicked on `text`
pub fn pattern_clicked(plugin: &str, name: Option<&str>, text: &str, pane: u64, cx: &App) {
    let Some(host) = cx.try_global::<PluginHost>() else {
        return;
    };
    if let Some(process) = host
        .plugins
        .iter()
        .find(|p| p.spec.name == plugin)
        .and_then(|plugin| plugin.process.as_ref())
    {
        process.send(&notification(
            "pattern",
            json!({"name": name, "text": text, "pane": pane}),
        ));
    }
}

/// text of title block `block` of `plugin` for pane `pane`, empty when it set none
pub fn title_block(plugin: &str, block: &str, pane: u64, cx: &App) -> String {
    let Some(host) = cx.try_global::<PluginHost>() else {
        return String::new();
    };
    let Some(plugin) = host.plugins.iter().find(|p| p.spec.name == plugin) else {
        return String::new();
    };
    plugin
        .pane_blocks
        .get(&(block.to_string(), pane))
        .or_else(|| plugin.blocks.get(block))
        .cloned()
        .unwrap_or_default()
}

/// add `spec` as if the user allowed it, for tests that can't click
#[cfg(test)]
pub(crate) fn add_trusted(spec: PluginSpec, cx: &mut App) {
    let hash = spec.hash();
    cx.global_mut::<PluginHost>().trust.trust(&spec.name, &hash);
    add(spec, cx);
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Capabilities, PluginHost, PluginState, add_trusted, commands, title_block};
    use crate::{
        plugins::{permissions::Permissions, spec::PluginSpec},
        settings::Settings,
    };

    fn pump_until(cx: &mut gpui::TestAppContext, what: &str, done: impl Fn(&gpui::App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !cx.update(|cx| done(cx)) {
            assert!(Instant::now() < deadline, "timed out: {what}");
            std::thread::sleep(Duration::from_millis(15));
            cx.executor().advance_clock(Duration::from_millis(200));
            cx.run_until_parked();
        }
    }

    #[gpui::test]
    fn a_plugin_adds_commands_and_blocks_and_comes_back_after_a_crash(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.executor().allow_parking();
        let reply = r#"{"jsonrpc":"2.0","id":0,"result":{"commands":[{"name":"hello"}]}}"#;
        let block =
            r#"{"jsonrpc":"2.0","method":"set_title_block","params":{"name":"b","text":"hi"}}"#;
        let frame = |json: &str| format!("Content-Length: {}\\r\\n\\r\\n{json}", json.len());
        // answers initialize, sets a block, then crashes a moment later
        let script = format!(
            "printf '%b' '{}{}'; sleep 0.3; exit 1",
            frame(reply),
            frame(block)
        );
        cx.update(|cx| {
            cx.set_global(Settings::default());
            super::start(cx);
            add_trusted(
                PluginSpec {
                    name: "demo".into(),
                    command: vec!["sh".into(), "-c".into(), script],
                    permissions: Permissions::default(),
                    dir: None,
                },
                cx,
            );
        });
        pump_until(cx, "no command from the plugin", |cx| {
            commands(cx).iter().any(|command| command.name == "hello")
        });
        pump_until(cx, "no title block", |cx| {
            title_block("demo", "b", 1, cx) == "hi"
        });
        let generation = cx.update(|cx| cx.global::<PluginHost>().plugins[0].generation);
        // it exits, kuterm keeps running and starts it again
        pump_until(cx, "never restarted", |cx| {
            let plugin = &cx.global::<PluginHost>().plugins[0];
            plugin.generation > generation && plugin.state == PluginState::Running
        });
        pump_until(cx, "commands never came back", |cx| {
            !commands(cx).is_empty()
        });
    }

    #[test]
    fn capabilities_parse_with_parts_left_out() {
        let caps: Capabilities = serde_json_lenient::from_str(
            r#"{
                "commands": [{ "name": "hello", "category": "demo" }, { "name": "bye" }],
                "title_blocks": ["clock"],
                "patterns": [{ "name": "ticket", "regex": "JIRA-[0-9]+" }],
                "panels": [{ "name": "top", "command": ["htop"] }],
                "unknown": true
            }"#,
        )
        .unwrap();
        assert_eq!(caps.commands.len(), 2);
        assert_eq!(caps.commands[1].category, None);
        assert_eq!(caps.title_blocks, ["clock"]);
        assert_eq!(caps.patterns[0].regex, "JIRA-[0-9]+");
        assert_eq!(caps.panels[0].command, ["htop"]);
        let empty: Capabilities = serde_json_lenient::from_str("{}").unwrap();
        assert_eq!(empty, Capabilities::default());
    }
}
