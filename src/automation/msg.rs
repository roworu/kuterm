//! `kuterm msg`: the command line client of the socket

use std::path::Path;

use super::{
    protocol::Request,
    socket::{send, subscribe},
};
use crate::settings::Values;

/// usage of `kuterm msg`
pub const MSG_USAGE: &str = "usage: kuterm msg <command>

commands:
  new-tab [--cwd <dir>] [--profile <name>] [--name <name>]   open a tab in the active window
  send-text <text>                    type text into a pane
  list-tabs [--json]                  print the open tabs
  new-window [--cwd <dir>]            open a window
  get-text [--scrollback]             print what is shown in a pane, or its whole history
  focus-tab                           switch to a pane
  close-tab                           close a pane, asking first like a click on its x
  split <right|left|up|down>          split a pane
  set-tab-title <title>               give a tab a title, empty takes it back
  set-tab-color [#rrggbb]             draw a tab in a color, none takes it back
  run-command <name> [--input k=v...] run a command palette command, like \"clear history\".
                                      --input fills one of its inputs, a command needing
                                      more than its inputs give fails
  query [--last-output] [--block <n>] [--all] [--json]
                                      the commands a pane ran: command, exit, folder, times
                                      and output. --block 3 is the third, --block -2 the one
                                      before the last finished one
  subscribe [<event>...] [--json]     print events as they happen, like command_finished or
                                      bell, all of them without names
  queue <command>                     run a command once the prompt is back, after the ones
                                      queued before. \"@18:00 <command>\" also waits for 18:00

commands acting on a pane take --pane <name, title or id>, the active tab without it. ids
are in list-tabs and in $KUTERM_PANE of the pane's shell. text people don't see, like
concealed text, is left out of get-text and query.

kuterm needs \"remote_control\": \"socket\" in its settings. $KUTERM_SOCKET picks the
socket, otherwise the newest running kuterm is used";

/// the request `kuterm msg` arguments ask for, and whether to print json
pub fn parse_msg(args: &[String]) -> Result<(Request, bool), String> {
    let (command, rest) = args
        .split_first()
        .ok_or_else(|| "msg needs a command".to_string())?;
    let mut cwd = None;
    let mut profile = None;
    let mut name = None;
    let mut pane = None;
    let mut json = false;
    let mut scrollback = false;
    let mut block = None;
    let mut all = false;
    let mut inputs = Values::new();
    let mut words = Vec::new();
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        let mut value = || {
            rest.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--cwd" => {
                let dir = value()?;
                cwd = Some(std::path::absolute(&dir).map_err(|error| format!("{dir}: {error}"))?);
            }
            "--profile" => profile = Some(value()?),
            "--name" => name = Some(value()?),
            "--pane" => pane = Some(value()?),
            "--json" => json = true,
            "--scrollback" => scrollback = true,
            "--all" => all = true,
            "--last-output" => block = Some(-1),
            "--input" => {
                let pair = value()?;
                let Some((name, value)) = pair.split_once('=') else {
                    return Err(format!("--input takes name=value, not {pair:?}"));
                };
                inputs.insert(name.to_string(), value.to_string());
            }
            "--block" => {
                let number = value()?;
                block = Some(
                    number
                        .parse()
                        .map_err(|_| format!("--block takes a number, not {number:?}"))?,
                );
            }
            _ => words.push(arg.clone()),
        }
    }
    let text = words.join(" ");
    let request = match command.as_str() {
        "new-tab" => Request::NewTab { cwd, profile, name },
        "send-text" => Request::SendText { text, pane },
        "list-tabs" => Request::ListTabs,
        "new-window" => Request::NewWindow { cwd },
        "get-text" => Request::GetText { pane, scrollback },
        "focus-tab" => Request::FocusTab { pane },
        "close-tab" => Request::CloseTab { pane },
        "split" => match text.as_str() {
            "right" | "left" | "up" | "down" => Request::Split {
                pane,
                direction: text,
            },
            _ => return Err("split takes right, left, up or down".into()),
        },
        "set-tab-title" => Request::SetTabTitle { pane, title: text },
        "set-tab-color" => Request::SetTabColor {
            pane,
            color: (!text.is_empty()).then_some(text),
        },
        "run-command" if !text.is_empty() => Request::RunCommand {
            pane,
            name: text,
            inputs,
        },
        "run-command" => return Err("run-command needs the name of a command".into()),
        "query" => Request::Query { pane, block, all },
        "queue" if !text.is_empty() => Request::Queue {
            pane,
            command: text,
        },
        "queue" => return Err("queue needs a command".into()),
        "subscribe" => Request::Subscribe { events: words },
        other => return Err(format!("unknown msg command {other:?}")),
    };
    Ok((request, json))
}

/// run `kuterm msg` with its arguments, the exit code says if it worked
pub fn run_msg(args: &[String]) -> i32 {
    let (request, json) = match parse_msg(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{error}\n\n{MSG_USAGE}");
            return 2;
        }
    };
    if let Request::Subscribe { .. } = &request {
        let result = subscribe(&request, |line| {
            use std::io::Write;
            let mut stdout = std::io::stdout();
            writeln!(stdout, "{line}")
                .and_then(|()| stdout.flush())
                .is_ok()
        });
        return match result {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{error}");
                1
            }
        };
    }
    let reply = match send(&request) {
        Ok(reply) => reply,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    if !reply.ok {
        eprintln!("{}", reply.error.unwrap_or_else(|| "failed".into()));
        return 1;
    }
    if request == Request::ListTabs {
        if json {
            println!(
                "{}",
                serde_json_lenient::to_string_pretty(&reply.tabs).unwrap_or_default()
            );
        } else {
            for tab in &reply.tabs {
                let mark = if tab.active { "*" } else { " " };
                let cwd = tab.cwd.as_deref().map(Path::display);
                let cwd = cwd.map(|cwd| format!("  {cwd}")).unwrap_or_default();
                println!(
                    "{mark} {} {}:{}:{} {}{cwd}",
                    tab.id, tab.window, tab.pane, tab.number, tab.title
                );
            }
        }
    }
    if let Some(text) = &reply.text {
        println!("{text}");
    }
    if let Request::Query { all, .. } = &request {
        if json {
            let blocks = if *all {
                serde_json_lenient::to_string_pretty(&reply.blocks)
            } else {
                serde_json_lenient::to_string_pretty(&reply.blocks.first())
            };
            println!("{}", blocks.unwrap_or_default());
        } else {
            for block in &reply.blocks {
                if *all {
                    let exit = block.exit.map_or("-".into(), |exit| exit.to_string());
                    println!("{} [{exit}] {}", block.number, block.command);
                } else if let Some(output) = &block.output {
                    println!("{output}");
                }
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Request, parse_msg};
    use crate::settings::Values;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn msg_arguments_become_requests() {
        assert_eq!(
            parse_msg(&args("new-tab --cwd /tmp --profile dev --name build")).unwrap(),
            (
                Request::NewTab {
                    cwd: Some(PathBuf::from("/tmp")),
                    profile: Some("dev".into()),
                    name: Some("build".into()),
                },
                false
            )
        );
        assert_eq!(
            parse_msg(&args("send-text --pane build echo hi"))
                .unwrap()
                .0,
            Request::SendText {
                text: "echo hi".into(),
                pane: Some("build".into()),
            }
        );
        let pane = || Some("3".to_string());
        for (line, want) in [
            (
                "get-text --pane 3 --scrollback",
                Request::GetText {
                    pane: pane(),
                    scrollback: true,
                },
            ),
            ("focus-tab --pane 3", Request::FocusTab { pane: pane() }),
            ("close-tab --pane 3", Request::CloseTab { pane: pane() }),
            (
                "split down --pane 3",
                Request::Split {
                    pane: pane(),
                    direction: "down".into(),
                },
            ),
            (
                "set-tab-title --pane 3 my build",
                Request::SetTabTitle {
                    pane: pane(),
                    title: "my build".into(),
                },
            ),
            (
                "set-tab-color --pane 3 #ff0000",
                Request::SetTabColor {
                    pane: pane(),
                    color: Some("#ff0000".into()),
                },
            ),
            (
                "queue --pane 3 @18:00 make deploy",
                Request::Queue {
                    pane: pane(),
                    command: "@18:00 make deploy".into(),
                },
            ),
            (
                "set-tab-color --pane 3",
                Request::SetTabColor {
                    pane: pane(),
                    color: None,
                },
            ),
            (
                "run-command --pane 3 clear history",
                Request::RunCommand {
                    pane: pane(),
                    name: "clear history".into(),
                    inputs: Values::new(),
                },
            ),
            (
                "run-command checkout branch --input branch=main",
                Request::RunCommand {
                    pane: None,
                    name: "checkout branch".into(),
                    inputs: Values::from([("branch".to_string(), "main".to_string())]),
                },
            ),
            (
                // only the first "=" splits, the rest belongs to the value
                "run-command x --input msg=a=b --input n=1",
                Request::RunCommand {
                    pane: None,
                    name: "x".into(),
                    inputs: Values::from([
                        ("msg".to_string(), "a=b".to_string()),
                        ("n".to_string(), "1".to_string()),
                    ]),
                },
            ),
            (
                "query --pane 3 --last-output",
                Request::Query {
                    pane: pane(),
                    block: Some(-1),
                    all: false,
                },
            ),
            (
                "query --block -2",
                Request::Query {
                    pane: None,
                    block: Some(-2),
                    all: false,
                },
            ),
            (
                "subscribe command_finished bell",
                Request::Subscribe {
                    events: vec!["command_finished".into(), "bell".into()],
                },
            ),
        ] {
            assert_eq!(parse_msg(&args(line)).unwrap().0, want, "{line}");
        }
        assert!(parse_msg(&args("split sideways")).is_err());
        assert!(parse_msg(&args("run-command")).is_err());
        assert!(parse_msg(&args("queue --pane 3")).is_err());
        assert!(parse_msg(&args("run-command x --input novalue")).is_err());
        assert!(parse_msg(&args("query --block x")).is_err());
        assert_eq!(
            parse_msg(&args("list-tabs --json")).unwrap(),
            (Request::ListTabs, true)
        );
        assert!(parse_msg(&args("fly")).is_err());
        assert!(parse_msg(&args("new-tab --cwd")).is_err());
        assert!(parse_msg(&[]).is_err());
    }
}
