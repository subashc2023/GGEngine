//! `gg mcp`: the tool API served to agents that speak MCP, the Model Context Protocol, over stdio:
//! JSON-RPC 2.0 messages, a line each, read on stdin and answered on stdout, as MCP's stdio
//! transport has them. Each tool is a method of the API, or `gg`'s `sessions`, and answers what
//! they answer, as JSON text, a capture its PNG too. Each call connects afresh to the game the
//! options choose, so that one `gg run` restarted is the one it reaches, as the MCP client its
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
path, which `undo` with that author takes back. While the game plays a copy of its world as \
edited, as after the editor's Play, an edit goes with the copy at `stop`; a scene file's save \
lasts. `edits` lists each author's edits. `scene` saves the open scene, the scene file `status` \
names, whose `unsaved` says whether it differs from the file, writing its entities, never those \
the game's code made; or opens another scene file in its place, or empties it, neither of which \
can be undone. `look` sets parts of the view's look, its viewpoint, an outline round an entity, a \
grid, and whether colliders' outlines show, which the editor and any other client set too, the \
last to set a part winning; give back, as null, what you set when you're done. `capture` shows \
the view with the look, and `pick` names the entity at a pixel of it, as a capture's PNG has it. \
Textures, models and fonts have IDs, 16 hex digits, which `assets` lists with their paths: the \
game, but a lockstep session's joiner, whose host does, writes each one's in an ID file beside \
it, `<file>.meta`, such as `stone.png.meta`, as it starts and as it finds new files; commit it \
with its file. Rename or move a file with its ID file or alone: the game moves an ID file left behind \
beside the file it was renamed to, by what the file holds, as it runs or as it next starts, so \
that it keeps its ID, unless you edit it in the same step, or rename it in a clone the game hasn't \
run in yet; a warning then names the ID left behind. A texture's, a model's or a font's value is \
its ID where its file has one, as scenes save it, a lockstep joiner's its host's, and else its \
path; a value you give may name it either way, by its path from the asset root, and the game loads \
one it hasn't loaded yet. A model is placed as an entity with a `ModelRoot` of it, under which the \
game spawns the model's nodes, which you change through the model's file or the root, never \
themselves. An entity's `Text` draws its text where the entity is, in its font, or the engine's \
own for none. Edit the file of a shader, a texture, a model, a font, a sound or music the game \
has loaded, and it reloads within a second or so, warning in its logs if it doesn't compile, \
decode or bake, music playing on from where it was; in a recorded run or a lockstep session, a \
model's nodes and materials change only as the game next starts. You can't hear the game, but its \
logs say each sound and music it starts, `Sound started` and `Music started`, at the debug level.";

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
    let numbers = |count: usize, description: &str| {
        json!({
            "type": "array",
            "items": {"type": "number"},
            "minItems": count,
            "maxItems": count,
            "description": description,
        })
    };
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": {"type": "object", "properties": properties, "required": required},
        })
    };
    // An object of one key, `name`, whose value is `value`, as gg-api writes an enum's variant
    // that holds something.
    let variant = |name: &str, value: Value| {
        json!({
            "type": "object",
            "properties": {name: value},
            "required": [name],
            "additionalProperties": false,
        })
    };
    let input = json!({
        "type": "array",
        "description": "The events, each an object of one key, its kind, or a string for one that \
                        holds nothing",
        "items": {"anyOf": [
            variant("key", json!({
                "type": "object",
                "description": "A key went down or up, by its position, as W3C UI Events' `code` \
                                names it, such as `KeyW`, `Space`, `ArrowLeft` or `ShiftLeft`",
                "properties": {
                    "key": {"type": "string"},
                    "pressed": {"type": "boolean"},
                    "repeat": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether it's a press a held key repeats",
                    },
                },
                "required": ["key", "pressed"],
                "additionalProperties": false,
            })),
            variant("text", json!({
                "type": "string",
                "description": "Text a key's press typed, after the press",
            })),
            variant("cursor", numbers(2, "The cursor moved over the view, to [x, y], pixels from \
                                          its top left, as a capture's PNG has it")),
            {
                "type": "string",
                "enum": ["cursor_entered", "cursor_left"],
                "description": "The cursor came over the view, or left it",
            },
            variant("button", json!({
                "type": "object",
                "description": "A mouse button went down or up",
                "properties": {
                    "button": {"anyOf": [
                        {"type": "string", "enum": ["left", "right", "middle", "back", "forward"]},
                        variant("other", json!({
                            "type": "integer",
                            "minimum": 0,
                            "maximum": 65535,
                            "description": "Another button, by its number, which differs from \
                                            one platform to another",
                        })),
                    ]},
                    "pressed": {"type": "boolean"},
                },
                "required": ["button", "pressed"],
                "additionalProperties": false,
            })),
            variant("scroll", json!({
                "description": "The wheel or a touchpad scrolled: positive y up, positive x left",
                "anyOf": [
                    variant("lines", numbers(2, "[x, y], as a wheel's notches go")),
                    variant("pixels", numbers(2, "[x, y], as a touchpad's go")),
                ],
            })),
            variant("focused", json!({
                "type": "boolean",
                "description": "The view gained or lost the keys",
            })),
        ]},
    });
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
            "The game's ticks, whether it's paused, whether it's stopped, its frames, its \
             entities, its view's size, its open scene's file, as `scene` names it, and whether \
             the open scene is unsaved.",
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
            "play",
            "Plays a copy of the game's world as edited in its place, until `stop`, if it's \
             stopped; a game already playing, as a plain `gg run` starts, gets nothing.",
            json!({}),
            &[],
        ),
        tool(
            "stop",
            "Stops the game: no tick runs, and its world is the world as edited, which only edits, \
             scene files' saves and `scene`'s `open` and `new` change; while a copy plays, brings \
             the world as edited back, what the copy did, its edits among them, going. IDs learned \
             while the copy played don't outlive it: they're drawn again. Recorded runs and \
             lockstep peers refuse it.",
            json!({}),
            &[],
        ),
        tool(
            "step",
            "Runs ticks of the simulation, 64 a second, pausing it, and answers once they've run; \
             refused while the game is stopped: `play` first.",
            json!({"ticks": {"type": "integer", "minimum": 1, "maximum": 10000, "default": 1}}),
            &[],
        ),
        tool(
            "scene",
            "Saves, opens or empties the open scene, the project's scene file the game started in \
             or one opened since, and answers the status, whose `scene` names it and whose \
             `unsaved` says whether it differs from its file. The open scene owns the entities its \
             file held and those clients' edits have made since it opened, their creates and \
             duplicates, never those the game's code made. `save` writes them to its file, or to \
             `path`, its file from then on; `open` puts the file at `path`'s entities in place of \
             them, keeping the game's own; `new` deletes them, leaving the scene untitled. `open` \
             and `new` can't be undone, every author's edits starting afresh, stop a copy that \
             plays, and are refused while the scene has unsaved edits (`scene.unsaved`), unless \
             `discard`.",
            json!({
                "action": {"type": "string", "enum": ["save", "open", "new"]},
                "path": {
                    "type": "string",
                    "description": "The scene file to open, or to save as: from the game's asset \
                                    root, as `status` names the open scene, such as \
                                    `scenes/level2.ron`, or absolute; in one of the project's \
                                    scenes folders, ending in `.ron`",
                },
                "discard": {
                    "type": "boolean",
                    "default": false,
                    "description": "For `open` and `new`: whether to discard the open scene's \
                                    unsaved edits",
                },
            }),
            &["action"],
        ),
        tool(
            "capture",
            "The game's view as a PNG, once a frame renders it, and where it's written: as it's \
             shown, the look included, but without the game's UI; or with `own`, as the game shows \
             it on its own, through its own viewpoint, without the look.",
            json!({"own": {"type": "boolean", "default": false}}),
            &[],
        ),
        tool(
            "look",
            "Sets parts of the game's look, the view's settings, which any client sets, such as \
             the editor, the last to set a part winning, and which last until a client changes \
             them or the game restarts; and answers the look: each part, null where it's the \
             game's own; `by`, the client that set each; `revision`, how many times it has \
             changed in the run; `run`; and `seen`, the viewpoint the last frame drew from, null \
             before the first or while it isn't finite. Leave a part out to keep it; null gives \
             it back to the game. Give back what you set when you're done. Without parts, it \
             reads the look.",
            json!({
                "viewpoint": {
                    "type": ["object", "null"],
                    "description": "Where the view is seen from, in place of the game's camera. \
                                    World space is right-handed with +Y up, and a viewpoint \
                                    without rotation looks down -Z",
                    "properties": {
                        "position": numbers(3, "[x, y, z]"),
                        "rotation": numbers(4, "A unit quaternion, [x, y, z, w]"),
                        "projection": {
                            "type": "object",
                            "description": "{\"Orthographic\": {\"height\", \"near\", \"far\"}} \
                                            or {\"Perspective\": {\"fov\", \"near\"}}, fov \
                                            vertical, in radians, as `entity` shows a camera's",
                        },
                    },
                    "required": ["position", "rotation", "projection"],
                },
                "outline": {
                    "type": ["object", "null"],
                    "description": "An outline round what the view shows of an entity, in place \
                                    of any the game draws",
                    "properties": {
                        "id": id,
                        "color": numbers(4, "Linear RGBA, alpha not multiplied in"),
                        "width": {
                            "type": "number",
                            "exclusiveMinimum": 0,
                            "description": "In the view's pixels, past 8 drawing 8",
                        },
                    },
                    "required": ["id", "color", "width"],
                },
                "grid": {
                    "type": ["string", "null"],
                    "enum": ["xy", "xz", "yz", null],
                    "description": "The plane a grid is drawn in, fitted to the view: `xy`, \
                                    where 2D games are, or `xz`, the ground",
                },
                "colliders": {
                    "type": ["boolean", "null"],
                    "description": "Whether colliders' outlines show, in place of whether the \
                                    game shows them",
                },
            }),
            &[],
        ),
        tool(
            "pick",
            "The entity the game's view shows at a pixel, as it's shown, the look included: its \
             ID and name, both null outside the view or where no entity draws. It answers once a \
             frame reads it, about four frames on, and fails while the game runs no frames, as \
             while its window is minimized.",
            json!({"at": numbers(2, "The pixel, [x, y] from the view's top left, as a capture's \
                                     PNG has it")}),
            &["at"],
        ),
        tool(
            "input",
            "Hands the game's layers input events, in order, as its window would, as a frame \
             starts: keys going down and up, by their positions, the text their presses typed, the \
             cursor moving over the view, coming over it and leaving it, mouse buttons, scrolls, \
             and the view's gaining or losing the keys. Release each key and button you press, as \
             the game holds it down until then.",
            json!({"events": input}),
            &["events"],
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
            "types",
            "The world's component types, in the order they registered, each with the value an \
             `add` without one gives it, or why there's none; and the descriptions of the types \
             they hold, which values are read and written by: each struct's fields and each \
             enum's variants, with their types, bounds, defaults and docs.",
            json!({}),
            &[],
        ),
        tool(
            "hash",
            "The world's hash, its content's and its layout's.",
            json!({}),
            &[]
        ),
        tool(
            "assets",
            "The game's textures, models and fonts, a page of them by their paths from its asset \
             root, as `scene` names files: each one's ID, 16 hex digits, or null while it has \
             none; its kind; whether a layer has added it; and `problem`, what's wrong with its ID \
             file, if anything, as `asset.duplicate_id`; and `orphans`, ID files whose assets \
             aren't there, as after a rename made with an edit. It answers once the game has looked at the asset \
             root since, so that a file added or renamed since is listed, and the game, but a \
             lockstep session's joiner, writes each missing ID file, `<file>.meta` beside its \
             file, which you commit with it, and moves one a rename of its file alone left behind \
             beside it.",
            json!({
                "kind": {
                    "type": "string",
                    "enum": gg_api::asset::KINDS,
                    "description": "Only those of this kind",
                },
                "skip": {"type": "integer", "minimum": 0, "default": 0},
                "take": {"type": "integer", "minimum": 0, "maximum": 10000, "default": 100},
            }),
            &[],
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
        "status" | "pause" | "resume" | "play" | "stop" | "types" | "hash" | "snapshot" => {
            call(name, Value::Null)
        }
        // `scene`'s path as given, from the game's asset root, as its status names scene files.
        "step" | "scene" | "look" | "pick" | "input" | "entities" | "entity" | "get" | "apply"
        | "undo" | "redo" | "edits" | "assets" => call(name, Value::Object(arguments)),
        "set" => {
            arguments.entry("path").or_insert_with(|| json!(""));
            call("apply", json!({"operations": [{"set": arguments}]}))
        }
        "capture" => {
            let params = match arguments.remove("own") {
                None | Some(Value::Null) => json!({}),
                Some(own) => json!({"own": own}),
            };
            let captured = game::call(pick, "capture", params)?;
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

    /// `initialize` serves the client's version if it's one of those served, names the edits after
    /// the client, and tells it of assets' ID files; a notification goes unanswered; and the tools
    /// are listed, each with a schema, `assets` taking the kinds gg-api names.
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
        let instructions = answer["result"]["instructions"].as_str();
        assert!(instructions.is_some_and(|text| text.contains("<file>.meta")));
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
        let assets = tools.iter().find(|tool| tool["name"] == "assets");
        let kinds = &assets.expect("an `assets` tool")["inputSchema"]["properties"]["kind"]["enum"];
        assert_eq!(*kinds, json!(gg_api::asset::KINDS));
        let unknown = super::answer(&mut pick, r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#);
        assert_eq!(unknown.expect("answered")["error"]["code"], -32601);
        let broken = super::answer(&mut pick, "{").expect("answered");
        assert_eq!(broken["error"]["code"], -32700);
    }

    /// `types` and `input` are listed, `types` taking nothing and `input` its events, each choice
    /// of whose schema is a kind of gg-api's input events, and each kind one of its choices.
    #[test]
    fn input_and_types_are_listed_with_the_apis_events() {
        use gg_api::{Button, InputEvent, Scroll};

        let listed = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let listed = answer(&mut pick(), listed).expect("answered");
        let tools = listed["result"]["tools"].as_array().expect("tools");
        let schema = |name: &str| {
            let tool = tools.iter().find(|tool| tool["name"] == name);
            tool.unwrap_or_else(|| panic!("a `{name}` tool"))["inputSchema"].clone()
        };
        let nothing = json!({"type": "object", "properties": {}, "required": []});
        assert_eq!(schema("types"), nothing);
        let input = schema("input");
        assert_eq!(input["required"], json!(["events"]));
        let events = &input["properties"]["events"];
        assert_eq!(events["type"], "array");
        // The kind each choice names: an object's one key, or a string's values.
        let choices = events["items"]["anyOf"].as_array().expect("choices");
        let mut named: Vec<&str> = (choices.iter())
            .flat_map(|choice| match choice["enum"].as_array() {
                Some(values) => values.iter().collect::<Vec<_>>(),
                None => (choice["required"].as_array().expect("its key").iter()).collect(),
            })
            .map(|name| name.as_str().expect("a kind"))
            .collect();
        // An event of each kind, which the match keeps whole as kinds are added.
        let each = [
            InputEvent::Key {
                key: "KeyW".to_owned(),
                pressed: true,
                repeat: false,
            },
            InputEvent::Text("w".to_owned()),
            InputEvent::Cursor([1.0, 2.0]),
            InputEvent::CursorEntered,
            InputEvent::CursorLeft,
            InputEvent::Button {
                button: Button::Left,
                pressed: true,
            },
            InputEvent::Scroll(Scroll::Lines([0.0, 1.0])),
            InputEvent::Focused(true),
        ];
        let mut kinds: Vec<String> = (each.iter())
            .map(|event| {
                match event {
                    InputEvent::Key { .. }
                    | InputEvent::Text(_)
                    | InputEvent::Cursor(_)
                    | InputEvent::CursorEntered
                    | InputEvent::CursorLeft
                    | InputEvent::Button { .. }
                    | InputEvent::Scroll(_)
                    | InputEvent::Focused(_) => {}
                }
                match serde_json::to_value(event).expect("an event") {
                    Value::String(kind) => kind,
                    Value::Object(event) => event.keys().next().expect("its kind").clone(),
                    other => panic!("an event as {other}"),
                }
            })
            .collect();
        named.sort_unstable();
        kinds.sort_unstable();
        assert_eq!(named, kinds);
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
