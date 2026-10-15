//! `kuterm mcp`: a model context protocol server on stdin and stdout, json-rpc one message per
//! line, so agents use the remote socket of the running kuterm. every run waits for the user
//! to allow it in the window

use std::{
    io::{BufRead, Write},
    sync::mpsc,
    time::Duration,
};

use serde_json_lenient::{Value, json};

use crate::remote::{self, Reply, Request};

// how long the subscribe tool waits for an event when the agent gives no time
const SUBSCRIBE_SECONDS: u64 = 30;

/// the tools offered, with the json schema of their arguments
fn tools() -> Value {
    let pane = json!({
        "type": "string",
        "description": "tab name, title or id from list-tabs, the active tab when left out"
    });
    json!([
        {
            "name": "query",
            "description": "commands a kuterm pane ran: command line, exit code, folder, times and output as people saw it. the last finished one unless block or all is given",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane,
                    "block": {"type": "integer", "description": "1 based block number, negative counts back from the last finished one"},
                    "all": {"type": "boolean", "description": "every block still in the history"}
                }
            }
        },
        {
            "name": "get_text",
            "description": "the text a kuterm pane shows, or its whole history, as people see it",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane,
                    "scrollback": {"type": "boolean", "description": "the whole history, not only the screen"}
                }
            }
        },
        {
            "name": "run",
            "description": "type a command into a kuterm pane and press enter, once the user allowed it in the window",
            "inputSchema": {
                "type": "object",
                "properties": {"pane": pane, "command": {"type": "string"}},
                "required": ["command"]
            }
        },
        {
            "name": "subscribe",
            "description": "wait for the next kuterm event, like command_finished or bell, and return it",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "events": {"type": "array", "items": {"type": "string"}, "description": "event names, any event when left out"},
                    "timeout_seconds": {"type": "integer", "description": "how long to wait, 30 when left out"}
                }
            }
        }
    ])
}

fn string(arguments: &Value, key: &str) -> Option<String> {
    arguments.get(key)?.as_str().map(str::to_string)
}

/// the socket request a tool call asks for, none for subscribe and unknown tools
fn tool_request(name: &str, arguments: &Value) -> Result<Request, String> {
    let pane = string(arguments, "pane");
    Ok(match name {
        "query" => Request::Query {
            pane,
            block: arguments.get("block").and_then(Value::as_i64),
            all: arguments
                .get("all")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "get_text" => Request::GetText {
            pane,
            scrollback: arguments
                .get("scrollback")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "run" => Request::Run {
            pane,
            command: string(arguments, "command").ok_or("run needs a command")?,
        },
        other => return Err(format!("unknown tool {other:?}")),
    })
}

/// text an agent gets for a reply
fn reply_text(reply: &Reply) -> String {
    if let Some(text) = &reply.text {
        return text.clone();
    }
    if !reply.blocks.is_empty() {
        return serde_json_lenient::to_string_pretty(&reply.blocks).unwrap_or_default();
    }
    "done".into()
}

/// the next event named in `names` within `seconds`, as json
fn next_event(names: Vec<String>, seconds: u64) -> Result<String, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let request = Request::Subscribe { events: names };
        #[cfg(unix)]
        let result = remote::subscribe(&request, |line| {
            tx.send(Ok(line.to_string())).ok();
            false
        });
        #[cfg(not(unix))]
        let result: Result<(), String> = Err("remote control needs unix sockets".into());
        if let Err(error) = result {
            tx.send(Err(error)).ok();
        }
    });
    rx.recv_timeout(Duration::from_secs(seconds))
        .unwrap_or_else(|_| Ok(format!("no event in {seconds} seconds")))
}

/// the answer to one json-rpc message, none for notifications. `send` reaches kuterm
pub fn handle(message: &Value, send: &dyn Fn(&Request) -> Result<Reply, String>) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "kuterm", "version": env!("CARGO_PKG_VERSION")}
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let outcome = if name == "subscribe" {
                let names = arguments
                    .get("events")
                    .and_then(Value::as_array)
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(|n| n.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let seconds = arguments
                    .get("timeout_seconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(SUBSCRIBE_SECONDS);
                next_event(names, seconds)
            } else {
                tool_request(name, &arguments).and_then(|request| {
                    let reply = send(&request)?;
                    if reply.ok {
                        Ok(reply_text(&reply))
                    } else {
                        Err(reply.error.unwrap_or_else(|| "failed".into()))
                    }
                })
            };
            // tool failures are results the agent reads, not protocol errors
            Ok(match outcome {
                Ok(text) => json!({"content": [{"type": "text", "text": text}], "isError": false}),
                Err(error) => {
                    json!({"content": [{"type": "text", "text": error}], "isError": true})
                }
            })
        }
        other => Err(json!({"code": -32601, "message": format!("unknown method {other}")})),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    })
}

/// serve mcp on stdin and stdout until stdin ends, the exit code says if it worked
pub fn run() -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            return 1;
        };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json_lenient::from_str::<Value>(&line) {
            Ok(message) => handle(&message, &remote::send),
            Err(error) => Some(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32700, "message": format!("parse error: {error}")}
            })),
        };
        if let Some(answer) = answer
            && writeln!(stdout, "{answer}")
                .and_then(|()| stdout.flush())
                .is_err()
        {
            return 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use serde_json_lenient::{Value, json};

    use super::handle;
    use crate::remote::{Reply, Request};

    fn call(message: Value, replies: Reply) -> (Option<Value>, Vec<Request>) {
        let sent = RefCell::new(Vec::new());
        let send = |request: &Request| {
            sent.borrow_mut().push(request.clone());
            Ok(replies.clone())
        };
        (handle(&message, &send), sent.into_inner())
    }

    #[test]
    fn initialize_and_list_tools() {
        let (answer, _) = call(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}),
            Reply::done(),
        );
        let answer = answer.unwrap();
        assert_eq!(answer["id"], 1);
        assert_eq!(answer["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(answer["result"]["serverInfo"]["name"], "kuterm");
        let (answer, _) = call(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            Reply::done(),
        );
        let names: Vec<String> = answer.unwrap()["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["query", "get_text", "run", "subscribe"]);
        // notifications get no answer
        let (answer, _) = call(
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            Reply::done(),
        );
        assert_eq!(answer, None);
        let (answer, _) = call(
            json!({"jsonrpc": "2.0", "id": 3, "method": "fly"}),
            Reply::done(),
        );
        assert_eq!(answer.unwrap()["error"]["code"], -32601);
    }

    #[test]
    fn tool_calls_go_to_the_socket() {
        let reply = Reply {
            ok: true,
            text: Some("screen text".into()),
            ..Reply::default()
        };
        let (answer, sent) = call(
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "get_text", "arguments": {"pane": "build"}}}),
            reply,
        );
        assert_eq!(
            sent,
            [Request::GetText {
                pane: Some("build".into()),
                scrollback: false
            }]
        );
        let answer = answer.unwrap();
        assert_eq!(answer["result"]["content"][0]["text"], "screen text");
        assert_eq!(answer["result"]["isError"], false);

        let (_, sent) = call(
            json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "run", "arguments": {"command": "make"}}}),
            Reply::done(),
        );
        assert_eq!(
            sent,
            [Request::Run {
                pane: None,
                command: "make".into()
            }]
        );
        // a denied run is an error the agent reads
        let (answer, _) = call(
            json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "run", "arguments": {"command": "rm"}}}),
            Reply::failed("the user denied the run"),
        );
        let answer = answer.unwrap();
        assert_eq!(answer["result"]["isError"], true);
        assert_eq!(
            answer["result"]["content"][0]["text"],
            "the user denied the run"
        );
        let (answer, sent) = call(
            json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "run", "arguments": {}}}),
            Reply::done(),
        );
        assert!(sent.is_empty());
        assert_eq!(answer.unwrap()["result"]["isError"], true);
    }
}
