//! `gg mcp`: the tool API served to agents that speak MCP, the Model Context Protocol, over stdio:
//! JSON-RPC 2.0 messages, a line each, read on stdin and answered on stdout, as MCP's stdio
//! transport has them. Each tool is a method of the API, or `gg`'s `sessions`, and answers what
//! they answer, as JSON text, a capture its PNG too. Each call connects afresh to the game the
//! options pick, so that one `gg run` restarted is the one it reaches, as the MCP client its
//! `initialize` names, the author of the edits it applies; `mcp` if it names none.

use std::fs;
use std::io::{self, BufRead, Write as _};

use gg_api::{Failure, LogEvents};
use serde_json::{Map, Value, json};

use crate::game::{self, Pick};

/// The versions of MCP served, the latest first: a client's own, if it's one of them, or else the
/// latest. Those that `initialize`, `tools/list` and `tools/call` are the same in, which is all
/// that's served; not 2025-03-26, whose servers take batches of messages, which this doesn't.
const VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2024-11-05"];

/// How many of the log's events `logs` gives without `from`, the last.
const MOST_LOG_LINES: usize = 200;

/// What `initialize` tells the client of the tools, for its model.
const INSTRUCTIONS: &str = "\
These tools reach a GGEngine game's development build as it runs, usually under `gg run`, through \
its tool API: read its world, step and capture it, and edit it. Entities are named by their IDs, \
16 hex digits, which `entities` lists with their names and components; `entity` shows one's \
components' values. An edit, `apply` or `set`, applies between ticks, paused or not, and `undo` \
takes back your last; a scene file saved while the game runs is an edit too, its author the file's \
path, which `undo` with that author takes back. `edits` lists each author's edits.";

/// Serves MCP on stdin and stdout until stdin ends, reaching the game `pick` picks.
///
/// Fails with `gg.io` if stdin or stdout breaks.
pub(crate) fn serve(mut pick: Pick) -> Result<(), Failure> {
    let io = |error: io::Error| Failure::new("gg.io", format!("stdin or stdout broke: {error}"));
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line.map_err(io)?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(answer) = answer(&mut pick, &line) {
            let mut out = io::stdout().lock();
            writeln!(out, "{answer}")
                .and_then(|()| out.flush())
                .map_err(io)?;
        }
    }
    Ok(())
}

/// The answer to the message `line`, as a line of JSON; none for a notification, or a response.
fn answer(pick: &mut Pick, line: &str) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(error) => {
            return Some(rpc_error(
                Value::Null,
                -32700,
                &format!("not JSON: {error}"),
            ));
        }
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // A response to a request of ours, which there are none of, has a result or an error;
        // anything else isn't a message, answered with its ID if it has one.
        let response = message.get("result").is_some() || message.get("error").is_some();
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        return (!response).then(|| rpc_error(id, -32600, "not a JSON-RPC request"));
    };
    // A notification, such as `notifications/initialized`, needs no answer.
    let id = message.get("id")?.clone();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => initialize(pick, &params),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => match call_tool(pick, &params) {
            Ok(result) => result,
            Err(message) => return Some(rpc_error(id, -32602, &message)),
        },
        other => return Some(rpc_error(id, -32601, &format!("no method `{other}`"))),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// What `initialize` answers: the version served, the tools, and what this is; and the client's
/// name, from its `clientInfo`, which the edits it applies are authored as.
fn initialize(pick: &mut Pick, params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|asked| VERSIONS.contains(asked))
        .unwrap_or(VERSIONS[0]);
    let name = params
        .pointer("/clientInfo/name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty());
    if let Some(name) = name {
        pick.client = name.to_owned();
    }
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "gg", "version": env!("CARGO_PKG_VERSION")},
        "instructions": INSTRUCTIONS,
    })
}

/// The JSON-RPC error answering request `id`.
fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The tools, each with its input's JSON Schema.
fn tools() -> Value {
    let id = json!({"type": "string", "description": "An entity's ID, 16 hex digits"});
    let author = json!({
        "type": "string",
        "description": "Whose edits: an MCP client's name, `gg`, or a scene file's path, such as \
                        `scenes/grid.ron`; yours without one",
    });
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": {"type": "object", "properties": properties, "required": required},
        })
    };
    json!([
        tool(
            "sessions",
            "The running games: each one's app, process, address, engine, log, the frames it has \
             run, and what it waits for, if anything.",
            json!({}),
            &[],
        ),
        tool(
            "status",
            "The game's ticks, whether it's paused, its frames, its entities, and its view's size.",
            json!({}),
            &[],
        ),
        tool("pause", "Pauses the game's simulation.", json!({}), &[]),
        tool(
            "resume",
            "Runs the game's simulation in real time again.",
            json!({}),
            &[],
        ),
        tool(
            "step",
            "Runs ticks of the simulation, 64 a second, pausing it, and answers once they've run.",
            json!({"ticks": {"type": "integer", "minimum": 1, "maximum": 10000, "default": 1}}),
            &[],
        ),
        tool(
            "capture",
            "The game's view, without its UI, as a PNG, once a frame renders it; and where it's \
             written.",
            json!({}),
            &[],
        ),
        tool(
            "entities",
            "A page of the world's entities: each one's ID, name, parent and components' types.",
            json!({
                "with": {"type": "string", "description": "Only those with this component"},
                "skip": {"type": "integer", "minimum": 0, "default": 0},
                "take": {"type": "integer", "minimum": 0, "maximum": 10000, "default": 100},
            }),
            &[],
        ),
        tool(
            "entity",
            "An entity: its ID, name, parent, children, and its components' values.",
            json!({"id": id}),
            &["id"],
        ),
        tool(
            "get",
            "A component's value, or its field at a path, such as `translation.x` or `rect[1]`.",
            json!({
                "id": id,
                "component": {"type": "string"},
                "path": {"type": "string", "default": ""},
            }),
            &["id", "component"],
        ),
        tool(
            "hash",
            "The world's hash, its content's and its layout's.",
            json!({}),
            &[]
        ),
        tool(
            "snapshot",
            "Saves the world whole as a snapshot file, which a run given `--restore` starts from.",
            json!({}),
            &[],
        ),
        tool(
            "apply",
            "Applies operations, in order, as one edit, all or none, which `undo` takes back. Each \
             is an object of one key, its kind: {\"set\": {\"id\", \"component\", \"path\", \
             \"value\"}}, {\"create\": {\"id\"?, \"parent\"?, \"components\": {name: value}}}, \
             {\"duplicate\": {\"id\", \"copies\"?}}, {\"delete\": {\"id\"}}, {\"add\": {\"id\", \
             \"component\", \"value\"?}}, {\"remove\": {\"id\", \"component\"}}, {\"reparent\": \
             {\"id\", \"parent\"}}. Values are JSON as `entity` shows them; a struct missing \
             fields takes their defaults. Answers the IDs of the entities it made.",
            json!({"operations": {"type": "array", "items": {"type": "object"}}}),
            &["operations"],
        ),
        tool(
            "set",
            "Sets a component's field at a path, such as `translation.x`, or the whole component \
             for an empty path, as an edit `undo` takes back.",
            json!({
                "id": id,
                "component": {"type": "string"},
                "path": {"type": "string", "default": ""},
                "value": {"description": "As `entity` shows values"},
            }),
            &["id", "component", "value"],
        ),
        tool(
            "undo",
            "Undoes an author's last edit, yours by default, and answers it; null if there's none.",
            json!({"author": author}),
            &[],
        ),
        tool(
            "redo",
            "Redoes the last edit an author undid, yours by default, and answers it; null if \
             there's none.",
            json!({"author": author}),
            &[],
        ),
        tool(
            "edits",
            "Each author's edits, or one author's: those undo steps back through, and those redo \
             does.",
            json!({"author": author}),
            &[],
        ),
        tool(
            "logs",
            "The game's log's events at a level or above, each its JSON log's line: the last 200 \
             the game keeps, or those since an earlier answer's `next`, given as `from` with that \
             answer's `run`. Answers them with `next` and `run`, for the next call, and how many it \
             `missed`, dropped before they were read.",
            json!({
                "level": {
                    "type": "string",
                    "enum": gg_api::LEVELS,
                    "default": "warn",
                },
                "from": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "An earlier answer's `next`: the events since it",
                },
                "run": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "The earlier answer's `run`, so that after the game restarts, \
                                    its events are read from the oldest kept",
                },
            }),
            &[],
        ),
    ])
}

/// What `tools/call` answers: the tool's result, or the error it failed with, as a result whose
/// `isError` is set, as MCP has a tool's errors.
///
/// Fails, saying why, for a call that doesn't name one of the tools, or gives arguments that aren't
/// an object, which MCP has as a protocol's error.
fn call_tool(pick: &Pick, params: &Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("`tools/call` names a tool in `name`")?;
    let tools = tools();
    let known = tools.as_array().expect("a list of tools").iter();
    if !known.map(|tool| &tool["name"]).any(|known| known == name) {
        return Err(format!("there's no tool `{name}`"));
    }
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(arguments)) => arguments.clone(),
        Some(_) => return Err("a tool's `arguments` are an object".to_owned()),
    };
    let text = |text: String| json!({"type": "text", "text": text});
    let done = match run(pick, name, arguments) {
        Ok(Ran::Json(result)) => json!({"content": [text(result)], "isError": false}),
        Ok(Ran::Image(result, png)) => {
            let image = json!({"type": "image", "data": base64(&png), "mimeType": "image/png"});
            json!({"content": [text(result), image], "isError": false})
        }
        Err(failure) => {
            let error = json!({"error": {"code": failure.code, "message": failure.message}});
            json!({"content": [text(error.to_string())], "isError": true})
        }
    };
    Ok(done)
}

/// What a tool gave: JSON, or JSON and a PNG.
enum Ran {
    Json(String),
    Image(String, Vec<u8>),
}

/// Runs tool `name` with `arguments`.
///
/// Fails with `gg.args` for a tool there's none of, and as the call it makes fails.
fn run(pick: &Pick, name: &str, mut arguments: Map<String, Value>) -> Result<Ran, Failure> {
    let call = |method: &str, params: Value| {
        game::call(pick, method, params).map(|result| Ran::Json(result.get().to_owned()))
    };
    match name {
        "sessions" => {
            game::sessions(pick.project.as_deref()).map(|sessions| Ran::Json(sessions.to_string()))
        }
        "status" | "pause" | "resume" | "hash" | "snapshot" => call(name, Value::Null),
        "step" | "entities" | "entity" | "get" | "apply" | "undo" | "redo" | "edits" => {
            call(name, Value::Object(arguments))
        }
        "set" => {
            arguments.entry("path").or_insert_with(|| json!(""));
            call("apply", json!({"operations": [{"set": arguments}]}))
        }
        "capture" => {
            let captured = game::call(pick, "capture", Value::Null)?;
            let result: Value = serde_json::from_str(captured.get()).unwrap_or(Value::Null);
            let path = result
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let png = fs::read(path).map_err(|error| {
                Failure::new("gg.io", format!("can't read the capture {path}: {error}"))
            })?;
            Ok(Ran::Image(captured.get().to_owned(), png))
        }
        "logs" => {
            let level = arguments.get("level").and_then(Value::as_str);
            let level = level.unwrap_or("warn").to_ascii_lowercase();
            if !gg_api::LEVELS.contains(&level.as_str()) {
                let message = format!(
                    "`level` is `error`, `warn`, `info`, `debug` or `trace`, not `{level}`"
                );
                return Err(Failure::new("gg.args", message));
            }
            let number = |name: &str| match arguments.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(given) => given.as_u64().map(Some).ok_or_else(|| {
                    let message = format!("`{name}` is a whole number, not {given}");
                    Failure::new("gg.args", message)
                }),
            };
            let params = match (number("from")?, number("run")?) {
                // A cursor numbers one run's events, which a restart starts again.
                (Some(_), None) => {
                    let message = "takes `from` with the `run` of the answer it came from";
                    return Err(Failure::new("gg.args", message));
                }
                (Some(from), run) => json!({"level": level, "from": from, "run": run}),
                (None, _) => json!({"level": level, "last": MOST_LOG_LINES}),
            };
            let read: LogEvents = game::connect(pick)?.call_as("logs", params)?;
            let events: Vec<&str> = read.events.iter().map(|event| event.get()).collect();
            Ok(Ran::Json(format!(
                r#"{{"run":{},"next":{},"missed":{},"events":[{}]}}"#,
                read.run,
                read.next,
                read.missed,
                events.join(",")
            )))
        }
        other => Err(Failure::new(
            "gg.args",
            format!("there's no tool `{other}`"),
        )),
    }
}

/// `bytes` in base64, as MCP's images are.
fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &byte)| n | u32::from(byte) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(DIGITS[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn pick() -> Pick {
        Pick {
            pid: None,
            app: None,
            timeout: Duration::from_secs(1),
            client: "mcp".to_owned(),
            project: None,
        }
    }

    #[test]
    fn base64_is_rfc_4648s() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ];
        for (bytes, encoded) in cases {
            assert_eq!(base64(bytes.as_bytes()), encoded, "{bytes}");
        }
    }

    /// `initialize` serves the client's version if it's one of those served, and names the edits
    /// after the client; a notification goes unanswered; and the tools are listed, each with a
    /// schema.
    #[test]
    fn a_client_initializes_and_lists_the_tools() {
        let mut pick = pick();
        let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"claude-code","version":"2.0"}}}"#
            .replace('\n', "");
        let answer = answer(&mut pick, &initialize).expect("answered");
        assert_eq!(answer["id"], 1);
        assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(pick.client, "claude-code");
        let newer = r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2099-01-01"}}"#;
        let answer = super::answer(&mut pick, newer).expect("answered");
        assert_eq!(answer["result"]["protocolVersion"], VERSIONS[0]);
        let notified = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert_eq!(super::answer(&mut pick, notified), None);
        let listed = super::answer(
            &mut pick,
            r#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#,
        )
        .expect("answered");
        let tools = listed["result"]["tools"].as_array().expect("tools");
        assert!(tools.len() > 10);
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        }
        let unknown = super::answer(&mut pick, r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#);
        assert_eq!(unknown.expect("answered")["error"]["code"], -32601);
        let broken = super::answer(&mut pick, "{").expect("answered");
        assert_eq!(broken["error"]["code"], -32700);
    }

    /// A tool that fails, as one given a level there's none of, or a cursor that isn't a number,
    /// does, before it looks for a game, answers a result whose `isError` is set; a tool there's
    /// none of is a protocol's error.
    #[test]
    fn a_tool_that_fails_answers_an_error_result() {
        for arguments in [
            r#"{"level":"loud"}"#,
            r#"{"from":"later"}"#,
            r#"{"from":3}"#,
        ] {
            let call = format!(
                r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"logs","arguments":{arguments}}}}}"#
            );
            let answer = answer(&mut pick(), &call).expect("answered");
            assert_eq!(answer["result"]["isError"], true, "{arguments}");
            let text = answer["result"]["content"][0]["text"]
                .as_str()
                .expect("text");
            assert!(text.contains("gg.args"), "{arguments}: {text}");
        }
        let call = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope"}}"#;
        let answer = super::answer(&mut pick(), call).expect("answered");
        assert_eq!(answer["error"]["code"], -32602);
        let odd = super::answer(&mut pick(), r#"{"jsonrpc":"2.0","id":6,"method":7}"#);
        assert_eq!(odd.expect("answered")["id"], 6);
    }
}
