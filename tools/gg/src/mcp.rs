//! `gg mcp`: the tool API served to agents that speak MCP, the Model Context Protocol, over stdio:
//! JSON-RPC 2.0 messages, a line each, read on stdin and answered on stdout, as MCP's stdio
//! transport has them. Each tool is a method of the API, or `gg`'s `sessions`, and answers what
//! they answer, as JSON text, a capture its PNG too if it's asked to inline it. Each call connects
//! afresh to the game the options choose, so that one `gg run` restarted is the one it reaches, as
//! the MCP client its `initialize` names, the author of the edits it applies; `mcp` if it names
//! none.

use std::fs;
use std::io::{self, BufRead, Write as _};
use std::time::Duration;

use gg_api::{Failure, LogEvents};
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::game::{self, Pick};
use crate::{probe, runs};

/// The versions of MCP served, the latest first: a client's own, if it's one of them, or else the
/// latest. Those that `initialize`, `tools/list` and `tools/call` are the same in, which is all
/// that's served; not 2025-03-26, whose servers take batches of messages, which this doesn't.
const VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2024-11-05"];

/// How many of the log's events `logs` gives without `from`, the last.
const MOST_LOG_LINES: usize = 200;

/// What the tools' `show` is, for their schemas: the names, and how a PNG's bytes read, as gg-api's
/// `Shown` says.
const SHOWN: &str = "\
A part of lit shading by name: `base_color` (sRGB, as a texture stores it), `normal` (which way \
the point faces in the world, with its normal map: each axis -1 to 1 as byte / 255 * 2 - 1, X red, \
Y green, Z blue), `surface_normal` (the same without the map), `metallic`, `roughness` (0.045 to \
1) and `occlusion` (each 0 to 1 as byte / 255, gray), `emission` (as `base_color`, past 1 shown as \
1), `specular_color` (what it reflects head-on, as `base_color`), `direct_light` (the frame's \
lights', shadows and all), `sky_light`, `reflection`, `diffuse_light` and `specular_light` (the \
light it diffuses and reflects, of all three), each light exposed and tonemapped as the view's. Or \
an image the frame makes, by debug name: any other string, as `view depth`, or `{\"image\": name}` \
(at most 64 bytes), read by its kind: colors as they are, HDR tonemapped, depth as 1 + log2(depth) \
/ 24 (white at the near plane, a 24th darker per doubling of distance, black where nothing drew), \
IDs and visibility a color per value (black for 0), motion as normals, a float as its value. An \
image the frame doesn't make shows black, warning `renderer.no_image`. A part of shading is drawn \
without antialiasing, fog, depth of field, motion blur, bloom, grade, lens, exposure or sky (black); \
an image, from a frame drawn as the game's but without antialiasing, so every other pass's images \
are there, as `bloom 1` or `depth of field near`.";

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
names, whose `unsaved` says whether it differs from the file, and `edited` whether you or another \
author edited it since, as play changes it too, writing its entities, never those \
the game's code made; or opens another scene file in its place, or empties it, neither of which \
can be undone. `look` sets parts of the view's look, its viewpoint, an outline round entities, a \
grid, whether colliders' outlines show, and `show`, which part of the lit shading or which image \
the frame makes the view shows in place of its light, which the editor and any other client set \
too, the last to set a part winning; give back, as null, what you set when you're done. `capture` \
shows the view with the look, or with its own `show`, for that capture alone, which an agent \
reads for what shading made of a pixel; several, taken together from one world, are an array; and \
`pick` names the entity at a pixel of it, as a capture's PNG has it; `probe` reads the numbers of \
what shading took and made at one pixel, which is the way to find what's wrong there. \
Textures, models, fonts, prefabs and UIs have IDs, 16 hex digits, which `assets` lists with their \
paths: the game, but a lockstep session's joiner, whose host does, writes each one's in an ID file \
beside it, `<file>.meta`, such as `stone.png.meta`, as it starts and as it finds new files; commit it \
with its file. Rename or move a file with its ID file or alone: the game moves an ID file left behind \
beside the file it was renamed to, by what the file holds, as it runs or as it next starts, so \
that it keeps its ID, unless you edit it in the same step, or rename it in a clone the game hasn't \
run in yet; a warning then names the ID left behind. A texture's, a model's or a font's value is \
its ID where its file has one, as scenes save it, a lockstep joiner's its host's, and else its \
path; a value you give may name it either way, by its path from the asset root, and the game loads \
one it hasn't loaded yet. A model is placed as an entity with a `ModelRoot` of it, under which the \
game spawns the model's nodes, which you change through the model's file or the root, never \
themselves. An entity's `Text` draws its text where the entity is, in its font, or the engine's \
own for none. Edit the file of a shader, a texture, a model, a font, a UI, a sound or music the \
game has loaded, and it reloads within a second or so, warning in its logs if it doesn't compile, \
decode, bake or read, music playing on from where it was; in a recorded run or a lockstep session, \
a model's nodes and materials change only as the game next starts. A game's UI from a `.ui` file, \
its menus and HUDs, is laid out by the engine: `ui` lists each node's rectangle, text and state. \
You can't hear the game, but its \
logs say each sound and music it starts, `Sound started` and `Music started`, at the debug level. \
`debug` debugs its Luau scripts: set a script's breakpoints by its path and lines, then wait for \
a stop with `wait_ms`, and read the stack's locals; while a script has stopped, the game waits \
inside its tick, and so do the other tools' requests, but `logs` and `sessions`, until `debug` \
continues or steps. Under `gg run`, an edit to the game's code is patched in, or rebuilt and the \
game started again from a snapshot: `wait` waits until the game runs it, or says why it doesn't, \
with rustc's errors; while it rebuilds, the other tools fail with `gg.rebuilding`, and once a build \
has failed and the game has ended, with `gg.build_failed`.";

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

/// A tool, as `tools/list` lists it: its name, what it does, and its input's JSON Schema, an object
/// of `properties`, those `required`.
struct Tool {
    name: &'static str,
    description: &'static str,
    properties: fn() -> Value,
    required: &'static [&'static str],
}

/// The tools, in `tools/list`'s order.
const TOOLS: &[Tool] = &[
    Tool {
        name: "sessions",
        description: "The running games: each one's app, process, address, engine, log, frames \
                      run, and what it waits for, if anything.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "status",
        description: "The game's ticks, pause and stop state, frames, entities, view size and open \
                      scene's file (`scene`); `unsaved`, whether the scene differs from its file, \
                      as a playing game's ticks make it; `edited`, whether authors edited it since \
                      it was saved or opened, which ticks don't; and in a world of cells, `cells`: \
                      how many are in, and how many out keep what they held.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "wait",
        description: "Call after editing the game's code, before checking what the edit does: \
                      waits until the game under `gg run` runs the code of every edit saved before \
                      the call, patched, or rebuilt and restarted, and answers what the run is \
                      doing. Fails with `gg.build_failed` and rustc's errors if the edits don't \
                      build (the game runs its old code), `gg.waiting` if the game ended and waits \
                      for a change, or `gg.timeout`.",
        properties: || {
            json!({"timeout_s": {"type": "integer", "minimum": 1, "default": 300,
                                 "description": "How long to wait, in seconds"}})
        },
        required: &[],
    },
    Tool {
        name: "pause",
        description: "Pauses the game's simulation.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "resume",
        description: "Runs the game's simulation in real time again.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "play",
        description: "Plays a copy of the game's world as edited in its place, until `stop`; a \
                      game already playing, as a plain `gg run` starts, is left as it is.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "stop",
        description: "Stops the game: no tick runs, and its world is the world as edited, which \
                      only edits, scene saves and `scene`'s `open` and `new` change. Stopping a \
                      playing copy brings the world as edited back: what the copy did goes, its \
                      edits and the IDs learned meanwhile among it. Recorded runs and lockstep \
                      peers refuse it.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "step",
        description: "Runs ticks of the simulation, 64 a second, pausing it, and answers once \
                      they've run; refused while the game is stopped: `play` first.",
        properties: || {
            json!({
                "ticks": {"type": "integer", "minimum": 1, "maximum": 10000, "default": 1},
            })
        },
        required: &[],
    },
    Tool {
        name: "scene",
        description: "Saves, opens or empties the open scene (the scene file the game started in, \
                      or one opened since) and answers the status. The open scene owns its file's \
                      entities and those clients' edits made since, their creates and duplicates, \
                      never those the game's code made.",
        properties: || {
            json!({
                "action": {
                    "type": "string",
                    "enum": ["save", "open", "new"],
                    "description": "`save` writes the scene's entities to its file, or to `path`, \
                                    its file from then on; `open` puts the entities of the file \
                                    at `path` in their place, keeping the game's own; `new` \
                                    deletes them, leaving the scene untitled. `open` and `new` \
                                    can't be undone, start every author's edits afresh, stop a \
                                    playing copy, and are refused while the scene has unsaved \
                                    edits (`scene.unsaved`), unless `discard`",
                },
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
            })
        },
        required: &["action"],
    },
    Tool {
        name: "capture",
        description: "Writes the game's view as a PNG once a frame renders it, as it's shown, the \
                      look included, without the game's UI; and answers its `path`, which a tool \
                      that reads images opens. The game keeps its newest 32 captures.",
        properties: || {
            json!({
                "own": {"type": "boolean", "default": false,
                        "description": "As the game shows it on its own: through its own \
                                        viewpoint, without the look"},
                "show": {
                    "anyOf": [shown(), {"type": "array", "minItems": 1, "items": shown()}],
                    "description": format!("What this capture alone shows in place of the \
                                            view's light, the look unchanged, in a frame that \
                                            isn't published. An array is taken in order from \
                                            one world, the game paused for it if it plays, and \
                                            answers an array of captures. {SHOWN}"),
                },
                "inline": {"type": "boolean", "default": false,
                           "description": "Whether the answer holds each PNG too, which a large \
                                           view makes large"},
            })
        },
        required: &[],
    },
    Tool {
        name: "frames",
        description: "Writes the game's next frames in a row, none skipped, as PNGs in `dir`, to \
                      see how the view settles, shimmers, flickers or trails frame by frame \
                      through a scripted move and a hold. Pause the game first to see the view \
                      alone change. Answers `frames` once all are written: each one's `path`, \
                      size, `ticks`, `streaming` and `viewpoint`.",
        properties: || {
            json!({
                "dir": {"type": "string",
                        "description": "An absolute path to a directory that exists, which gets \
                                        `frame-0001.png` on"},
                "count": {"type": "integer", "minimum": 1, "maximum": gg_api::MOST_FRAMES,
                          "description": "How many frames, from the first with nothing on its \
                                          way to the GPU"},
                "viewpoints": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": viewpoint(),
                        "required": ["position", "rotation", "projection"],
                    },
                    "description": "Each frame's viewpoint in turn, as `look`'s, the last held \
                                    for the frames after it; without them, the view as it's \
                                    shown, the look included",
                },
                "rect": {
                    "type": "array",
                    "items": {"type": "integer", "minimum": 0},
                    "minItems": 4,
                    "maxItems": 4,
                    "description": "The part of each frame to keep, [x, y, width, height] in \
                                    pixels from the view's top left: the frames hold at most \
                                    1 GiB of pixels",
                },
                "show": {
                    "anyOf": [shown(), {"type": "null"}],
                    "description": "What to show in place of the view's light, a name as \
                                    `capture`'s `show` takes it",
                },
            })
        },
        required: &["dir", "count"],
    },
    Tool {
        name: "look",
        description: "Sets parts of the view's look, which any client (such as the editor) sets, \
                      the last to set a part winning, until a client changes it or the game \
                      restarts. Leave a part out to keep it; null gives it back to the game, as \
                      you should what you set when done. Answers each part (null where it's the \
                      game's own), `by` (the client that set each), `revision` (changes in the \
                      run), `run`, and `seen` (the viewpoint the last frame drew from, null before \
                      the first or while it isn't finite).",
        properties: || {
            json!({
                "viewpoint": {
                    "type": ["object", "null"],
                    "description": "Where the view is seen from, in place of the game's camera. \
                                    World space is right-handed with +Y up, and a viewpoint \
                                    without rotation looks down -Z",
                    "properties": viewpoint(),
                    "required": ["position", "rotation", "projection"],
                },
                "outline": {
                    "type": ["object", "null"],
                    "description": "An outline round what the view shows of entities and of \
                                    every entity under each, one shape where they touch, with \
                                    a glow past its band, in place of any the game draws; no \
                                    IDs outline nothing",
                    "properties": {
                        "ids": {
                            "type": "array",
                            "items": id(),
                            "description": "The entities to outline, by ID",
                        },
                        "color": numbers(4, "Linear RGBA, alpha not multiplied in"),
                        "width": {
                            "type": "number",
                            "exclusiveMinimum": 0,
                            "description": "The band's width in the view's pixels, past 8 \
                                            drawing 8; the glow reaches as far again",
                        },
                    },
                    "required": ["ids", "color", "width"],
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
                "show": {
                    "anyOf": [shown(), {"type": "null"}],
                    "description": format!("What the view shows in place of its light, to see \
                                            how it's made, and which any capture without its own \
                                            `show` shows: {SHOWN}"),
                },
                "skip": {
                    "type": ["array", "null"],
                    "items": {"enum": passes()},
                    "description": "The passes the view leaves out, to narrow down which one makes \
                                    what it shows, each by the name of the environment's field \
                                    that turns it on, or `shadows`: each left out as the game \
                                    would draw without it, the game itself unchanged, and a \
                                    capture with `own` showing it as it is. `[]` or null gives \
                                    them back",
                },
            })
        },
        required: &[],
    },
    Tool {
        name: "pick",
        description: "The entity the view shows at a pixel, as it's shown, the look included: its \
                      ID and name, both null outside the view or where none draws; a model's node \
                      gives the model's root, which tools change in its place. Answers about four \
                      frames on, and fails while the game runs no frames, as while its window is \
                      minimized.",
        properties: || {
            json!({
                "at": numbers(2, "The pixel, [x, y] from the view's top left, as a capture's PNG \
                                  has it"),
                "within": {"type": "string",
                           "description": "The ID of the model's root, or of one of its nodes: \
                                           answers the node drawn at the pixel, not the root"},
            })
        },
        required: &["at"],
    },
    Tool {
        name: "images",
        description: "The images the next frame makes that a `show` (of `capture` or `look`) \
                      names, in the order made, each once: its debug name (`view depth`, `ambient \
                      occlusion`, `bloom 1`), what it holds (`color`, `hdr`, `ids`, `motion`, \
                      `visibility`, `depth` or `scalar`, which says how a show reads it) and its \
                      [width, height]. A frame showing a part of shading makes none after the \
                      view's passes, such as bloom's. Answers once a frame renders; fails while \
                      the game runs no frames, as while its window is minimized.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "probe",
        description: "What shading took and made at one pixel of the view, as numbers: the way to \
                      find what's wrong there (its material, its normal, which light reaches it), \
                      rather than reading a PNG by eye. Captures the pixel as each part of \
                      shading, writing no PNG, with a `pick` and a `look`, from one world, the \
                      game paused for them if it plays. Answers `at`, `size` (the view's [width, \
                      height]), `ticks`; `base_color`, `emission`, `specular_color` (reflected \
                      head-on): linear RGB from 0; `normal` (with its normal map), \
                      `surface_normal` (without): [x, y, z] in the world, -1 to 1; `metallic`, \
                      `roughness` (from 0.045), `occlusion` (the share of the light from all \
                      around that reaches it): 0 to 1; `direct_light` (the frame's lights', \
                      shadows and all), `sky_light` (from all around, darkened by occlusion), \
                      `reflection`, `diffuse_light`, `specular_light` (diffused and reflected, of \
                      all three), `shown` (the view's pixel): sRGB bytes [r, g, b], exposed and \
                      tonemapped as shown, not linear; `depth`: reversed, 1 at the near plane and \
                      smaller farther, 0 where nothing drew; `distance`: `near / depth` in world \
                      units in front of the viewpoint, not from it, null where nothing drew or \
                      without a perspective; `entity`, `{id, name}` as `pick` answers; \
                      `quantized`: values are 8-bit, good to about 1/255, `distance` to about 3%. \
                      Every value is null for a pixel outside the view.",
        properties: || {
            json!({
                "at": numbers(2, "The pixel, [x, y] from the view's top left, not negative, as a \
                                  capture's PNG has it"),
                "own": {"type": "boolean", "default": false,
                        "description": "Whether to probe the view as the game shows it on its \
                                        own, through its own viewpoint, without the look"},
            })
        },
        required: &["at"],
    },
    Tool {
        name: "input",
        description: "Hands the game's layers input events, in order, as its window would, as a \
                      frame starts. Release each key and button you press: the game holds it down \
                      until then.",
        properties: || json!({"events": input()}),
        required: &["events"],
    },
    Tool {
        name: "entities",
        description: "A page of the world's entities: each one's ID, name, parent and components' \
                      types.",
        properties: || {
            json!({
                "with": {"type": "string", "description": "Only those with this component"},
                "skip": {"type": "integer", "minimum": 0, "default": 0},
                "take": {"type": "integer", "minimum": 0, "maximum": 10000, "default": 100},
            })
        },
        required: &[],
    },
    Tool {
        name: "entity",
        description: "An entity: its ID, name, parent, children and components' values. For an \
                      instance's entity (its root, or one its prefab gives), `instance` too: the \
                      outermost instance's `root` and `prefab`; `link`, its own prefab, if it's an \
                      instance's root; `overrides`, each component differing from its prefab with \
                      the fields that differ (`\"\"` for the whole), as `revert` takes them; and \
                      `added`, the components its prefab doesn't give.",
        properties: || json!({"id": id()}),
        required: &["id"],
    },
    Tool {
        name: "get",
        description: "A component's value, or its field at a path, such as `translation.x` or \
                      `rect[1]`.",
        properties: || {
            json!({
                "id": id(),
                "component": {"type": "string"},
                "path": {"type": "string", "default": ""},
            })
        },
        required: &["id", "component"],
    },
    Tool {
        name: "types",
        description: "The world's component types in registration order, each with the value an \
                      `add` without one gives it, or why there's none; and the descriptions of the \
                      types they hold, by which values are read and written: fields and variants, \
                      with their types, bounds, defaults and docs.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "hash",
        description: "The world's hash, its content's and its layout's.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "cells",
        description: "The world's cells, where `Cells` cuts it into scene files that come in \
                      around its viewers: its `Cells` entity, folder and cell size; its viewers, \
                      each where it is and its radius (the look's viewpoint among them while \
                      stopped, as the editor's camera is); and each cell in, coming, keeping what \
                      it held, refused, holding a streamed root or filed, with its streamed roots' \
                      count and whether a scene save would write it. `world` is null without \
                      cells.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "assets",
        description: "A page of the game's assets by path from its asset root, as `scene` names \
                      files: each one's ID (16 hex digits, null while it has none), kind, whether \
                      a layer added it, and `problem` with its ID file, as `asset.duplicate_id`; \
                      and `orphans`, ID files whose assets are gone, as after a rename made with \
                      an edit. Answers once the game has looked at the asset root since the call, \
                      so files added or renamed before it are listed. The game, but a lockstep \
                      joiner, writes each missing ID file, `<file>.meta` (commit it with its \
                      file), and moves one a lone rename left behind.",
        properties: || {
            json!({
                "kind": {
                    "type": "string",
                    "enum": gg_api::asset::KINDS,
                    "description": "Only those of this kind",
                },
                "skip": {"type": "integer", "minimum": 0, "default": 0},
                "take": {"type": "integer", "minimum": 0, "maximum": 10000, "default": 100},
            })
        },
        required: &[],
    },
    Tool {
        name: "ui",
        description: "Each UI the game's layers added from a `.ui` file, laid out for its view, to \
                      check a layout as data: `name`, its file's `asset` ID and `path`, whether \
                      the last frame `drawn` it, `input`, `focus`, and `nodes`, each after its \
                      parent: `name`, `kind`, `parent`, `rect` ([x, y, width, height] from the \
                      view's top left in canvas units, 1,080 to its height), `pixels` (the same in \
                      the view's pixels, as a capture has them), `visible`, `text`, a widget's \
                      `value` ({\"number\": 0.5}, {\"on\": true} or {\"choice\": 1}), `disabled`, \
                      `focused` and `pressed`. An edited file reloads, each node keeping what the \
                      game's code set. Press a button with `input`: the cursor at its pixels, then \
                      the left button down and up; type into a text field clicked so with `text`; \
                      scroll a list under the cursor with `scroll`.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "snapshot",
        description: "Saves the world whole as a snapshot file, which a run given `--restore` \
                      starts from.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "apply",
        description: "Applies operations in order as one edit, all or none, which `undo` takes \
                      back, and answers the IDs of the entities it made.",
        properties: || {
            json!({"operations": {
                "type": "array",
                "items": {"type": "object"},
                "description": "Each an object of one key, its kind: {\"set\": {\"id\", \
                                \"component\", \"path\", \"value\"}}, {\"create\": {\"id\"?, \
                                \"parent\"?, \"components\": {name: value}}}, {\"duplicate\": \
                                {\"id\", \"copies\"?}}, {\"delete\": {\"id\"}}, {\"add\": {\"id\", \
                                \"component\", \"value\"?}}, {\"remove\": {\"id\", \
                                \"component\"}}, {\"reparent\": {\"id\", \"parent\", \
                                \"in_place\"?}}, keeping the entity's own transform, or with \
                                \"in_place\" true where it is in the world; {\"unlink\": \
                                {\"id\"}}, unlinking a prefab's instance into entities of its \
                                own; {\"revert\": {\"id\", \"component\", \"path\"?}}, setting a \
                                field of an instance's entity back to its prefab's. Values are \
                                JSON as `entity` shows them; a struct missing fields takes \
                                their defaults",
            }})
        },
        required: &["operations"],
    },
    Tool {
        name: "save_prefab",
        description: "Saves an entity and its descendants as a new prefab with its ID file, \
                      writing over nothing, its root at its origin, and links the entity to it as \
                      one edit, which `undo` unlinks, leaving the file; its entities keep their \
                      IDs. Answers the prefab's path from the asset root and its name, its asset \
                      ID.",
        properties: || {
            json!({
                "id": id(),
                "path": {
                    "type": "string",
                    "description": "The prefab's file, from the game's asset root, in a folder \
                                    there that its scan reaches (no hidden or `target` folder), \
                                    ending in `.prefab`; by default in `prefabs/`, named after \
                                    the entity",
                },
            })
        },
        required: &["id"],
    },
    Tool {
        name: "set",
        description: "Sets a component's field at a path, such as `translation.x`, or the whole \
                      component for an empty path, as an edit `undo` takes back.",
        properties: || {
            json!({
                "id": id(),
                "component": {"type": "string"},
                "path": {"type": "string", "default": ""},
                "value": {"description": "As `entity` shows values"},
            })
        },
        required: &["id", "component", "value"],
    },
    Tool {
        name: "undo",
        description: "Undoes an author's last edit, yours by default, and answers it; null if \
                      there's none.",
        properties: || json!({"author": author()}),
        required: &[],
    },
    Tool {
        name: "redo",
        description: "Redoes the last edit an author undid, yours by default, and answers it; null \
                      if there's none.",
        properties: || json!({"author": author()}),
        required: &[],
    },
    Tool {
        name: "edits",
        description: "Each author's edits, or one author's: those undo steps back through, and \
                      those redo does.",
        properties: || json!({"author": author()}),
        required: &[],
    },
    Tool {
        name: "debug",
        description: "Debugs the game's Luau scripts: sets `breakpoints`, does `action`, waits up \
                      to `wait_ms` for a stop; then answers where a script `stopped` (its stack, \
                      each function's script, line, locals and upvalues), the pause or step \
                      `asked`, the `breakpoints` and the `scripts`. A stopped script waits inside \
                      its tick, and the game with it, the tools but `debug`, `logs` and `sessions` \
                      waiting too, until `debug` has it go on.",
        properties: || {
            json!({
                "breakpoints": {
                    "type": "object",
                    "description": "A script's breakpoints, in place of those it had, each at its \
                                    line or the next with code; no lines clears them",
                    "properties": {
                        "script": {
                            "type": "string",
                            "description": "Its path from the project's root, such as \
                                            `scripts/spin.luau`",
                        },
                        "lines": {"type": "array", "items": {"type": "integer", "minimum": 1}},
                    },
                    "required": ["script", "lines"],
                    "additionalProperties": false,
                },
                "action": {
                    "type": "string",
                    "enum": ["pause", "continue", "step_into", "step_over", "step_out"],
                    "description": "`pause` stops the next line a script runs; from a stop, \
                                    `continue` goes on to the next, and the steps step",
                },
                "wait_ms": {"type": "integer", "minimum": 0, "maximum": 10000, "default": 0,
                            "description": "How long to wait for a script to stop, if none has"},
            })
        },
        required: &[],
    },
    Tool {
        name: "logs",
        description: "The game's log's events at `level` or above, each its JSON log's line: the \
                      last 200 it keeps, or those since `from`. Answers them with `next` and \
                      `run`, for the next call, and how many it `missed`, dropped before they were \
                      read.",
        properties: || {
            json!({
                "level": {
                    "type": "string",
                    "enum": gg_api::LEVELS,
                    "default": "warn",
                },
                "from": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "An earlier answer's `next`, with that answer's `run`: the \
                                    events since it",
                },
                "run": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "The earlier answer's `run`, so that after the game restarts, \
                                    its events are read from the oldest kept",
                },
            })
        },
        required: &[],
    },
];

/// The tools, each with its input's JSON Schema.
fn tools() -> Value {
    let tool = |tool: &Tool| {
        json!({
            "name": tool.name,
            "description": tool.description,
            "inputSchema": {
                "type": "object",
                "properties": (tool.properties)(),
                "required": tool.required,
            },
        })
    };
    TOOLS.iter().map(tool).collect()
}

/// The schema of an entity's ID.
fn id() -> Value {
    json!({"type": "string", "description": "An entity's ID, 16 hex digits"})
}

/// The schema of an author of edits.
fn author() -> Value {
    json!({
        "type": "string",
        "description": "Whose edits: an MCP client's name, `gg`, or a scene file's path, such as \
                        `scenes/grid.ron`; yours without one",
    })
}

/// The schema of an array of `count` numbers.
fn numbers(count: usize, description: &str) -> Value {
    json!({
        "type": "array",
        "items": {"type": "number"},
        "minItems": count,
        "maxItems": count,
        "description": description,
    })
}

/// The schema of an object of one key, `name`, whose value is `value`, as gg-api writes an enum's
/// variant that holds something.
fn variant(name: &str, value: Value) -> Value {
    json!({
        "type": "object",
        "properties": {name: value},
        "required": [name],
        "additionalProperties": false,
    })
}

/// The schema of a `show`: a name, or an image's.
fn shown() -> Value {
    json!({"anyOf": [
        {"type": "string", "minLength": 1, "maxLength": 64},
        variant("image", json!({"type": "string", "minLength": 1, "maxLength": 64})),
    ]})
}

/// The schemas of a viewpoint's parts, as `look` and `frames` take them.
fn viewpoint() -> Value {
    json!({
        "position": numbers(3, "[x, y, z]"),
        "rotation": numbers(4, "A unit quaternion, [x, y, z, w]"),
        "projection": {
            "type": "object",
            "description": "{\"Orthographic\": {\"height\", \"near\", \"far\"}} or \
                            {\"Perspective\": {\"fov\", \"near\"}}, fov vertical, in radians, as \
                            `entity` shows a camera's",
        },
    })
}

/// The passes, by the API's own names, so that the list can't fall behind it.
fn passes() -> Vec<Value> {
    (gg_api::Pass::ALL.iter())
        .map(|pass| serde_json::to_value(pass).expect("a pass's name"))
        .collect()
}

/// The schema of `input`'s events.
fn input() -> Value {
    json!({
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
            variant("motion", numbers(2, "The mouse moved [x, y] in its own units, as its raw \
                                          motion goes, positive right and down, moving no \
                                          cursor: for looking about with the cursor grabbed")),
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
    })
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
    if !TOOLS.iter().any(|tool| tool.name == name) {
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
        Ok(Ran::Image(result, pngs)) => {
            let mut content = vec![text(result)];
            for png in pngs {
                let data = base64(&png);
                content.push(json!({"type": "image", "data": data, "mimeType": "image/png"}));
            }
            json!({"content": content, "isError": false})
        }
        Err(failure) => {
            let error = json!({"error": {"code": failure.code, "message": failure.message}});
            json!({"content": [text(error.to_string())], "isError": true})
        }
    };
    Ok(done)
}

/// What a tool gave: JSON, or JSON and PNGs.
enum Ran {
    Json(String),
    Image(String, Vec<Vec<u8>>),
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
        _ if crate::NO_PARAMETERS.contains(&name) || name == "snapshot" => call(name, Value::Null),
        // `scene`'s and `save_prefab`'s paths as given, from the game's asset root, as its status
        // names scene files.
        "step" | "scene" | "pick" | "input" | "entities" | "entity" | "get" | "apply"
        | "save_prefab" | "undo" | "redo" | "edits" | "assets" | "debug" => {
            call(name, Value::Object(arguments))
        }
        "look" | "frames" => {
            // A name that isn't a variant's is an image's, as `capture`'s `show` has it.
            if let Some(show) = arguments.get_mut("show")
                && !show.is_null()
            {
                *show = game::shown(show)?;
            }
            call(name, Value::Object(arguments))
        }
        "set" => {
            arguments.entry("path").or_insert_with(|| json!(""));
            call("apply", json!({"operations": [{"set": arguments}]}))
        }
        "capture" => {
            let inline = match arguments.remove("inline") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(inline)) => inline,
                Some(given) => {
                    let message = format!("`inline` is true or false, not {given}");
                    return Err(Failure::new("gg.args", message));
                }
            };
            let params = match arguments.remove("own") {
                None | Some(Value::Null) => json!({}),
                Some(own) => json!({"own": own}),
            };
            // An array of shows answers an array, even of one.
            let (shows, listed) = match arguments.remove("show") {
                None | Some(Value::Null) => (Vec::new(), false),
                Some(Value::Array(given)) if given.is_empty() => {
                    let message = "`show` is a name, or an array of at least one";
                    return Err(Failure::new("gg.args", message));
                }
                Some(Value::Array(given)) => (shows_of(&given)?, true),
                Some(given) => (shows_of(&[given])?, false),
            };
            let taken = game::captures(pick, params, &shows)?;
            captured_as(&taken, listed, inline)
        }
        "probe" => {
            let bad = |message: String| Failure::new("gg.args", message);
            let at = arguments.get("at").and_then(Value::as_array);
            let at = match at.map(|at| at.iter().map(Value::as_f64).collect::<Vec<_>>()) {
                Some(at) => match at[..] {
                    [Some(x), Some(y)] => probe::pixel([x, y])?,
                    _ => return Err(bad("`at` is [x, y], two numbers".to_owned())),
                },
                None => return Err(bad("`at` is [x, y], two numbers".to_owned())),
            };
            let own = match arguments.get("own") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(own)) => *own,
                Some(given) => return Err(bad(format!("`own` is true or false, not {given}"))),
            };
            probe::run(pick, at, own).map(|probe| Ran::Json(probe.to_string()))
        }
        "wait" => {
            let timeout = match arguments.get("timeout_s") {
                None | Some(Value::Null) => 300,
                Some(given) => given
                    .as_u64()
                    .filter(|&seconds| seconds > 0)
                    .ok_or_else(|| {
                        let message =
                            format!("`timeout_s` is a whole number of seconds, not {given}");
                        Failure::new("gg.args", message)
                    })?,
            };
            let told = runs::wait(pick, Duration::from_secs(timeout))?;
            Ok(Ran::Json(told.to_string()))
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

/// `given`, each a `show` of `capture` or `look`, as the API's `Shown`.
///
/// Fails with `gg.args` for one that isn't a name or an image's.
fn shows_of(given: &[Value]) -> Result<Vec<Value>, Failure> {
    given.iter().map(game::shown).collect()
}

/// What captures that the game answered, `taken`, give: their answers, which name each PNG's path,
/// for a tool that reads images to open, so that an agent's context doesn't hold every capture it
/// takes, as an array if `listed`, else the one's; and with `inline`, each PNG too, in order.
///
/// Fails with `gg.io` if a PNG it inlines can't be read.
fn captured_as(taken: &[Box<RawValue>], listed: bool, inline: bool) -> Result<Ran, Failure> {
    let answer = match taken {
        [one] if !listed => one.get().to_owned(),
        _ => game::joined(taken),
    };
    if !inline {
        return Ok(Ran::Json(answer));
    }
    let mut pngs = Vec::with_capacity(taken.len());
    for captured in taken {
        let result: Value = serde_json::from_str(captured.get()).unwrap_or(Value::Null);
        let path = result
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        pngs.push(fs::read(path).map_err(|error| {
            Failure::new("gg.io", format!("can't read the capture {path}: {error}"))
        })?);
    }
    Ok(Ran::Image(answer, pngs))
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
    use super::*;

    fn pick() -> Pick {
        Pick {
            pid: None,
            app: None,
            timeout: Duration::from_secs(1),
            client: "mcp".to_owned(),
            grant: None,
            project: None,
        }
    }

    /// A capture answers where its PNG is, and inlines the PNG only if it's asked to.
    #[test]
    fn a_capture_inlines_its_png_only_if_asked() {
        let path = std::env::temp_dir().join(format!("gg-mcp-capture-{}.png", std::process::id()));
        fs::write(&path, b"a png").expect("written");
        let captured = json!({"path": path, "ticks": 3}).to_string();
        let taken = [RawValue::from_string(captured.clone()).expect("JSON")];
        let named = captured_as(&taken, false, false);
        let inlined = captured_as(&taken, false, true);
        let listed = captured_as(&taken, true, true);
        let _ = fs::remove_file(&path);
        assert!(matches!(named, Ok(Ran::Json(answer)) if answer == captured));
        assert!(matches!(inlined, Ok(Ran::Image(answer, pngs))
            if answer == captured && pngs == [b"a png"]));
        // An array of shows answers an array, even of one, and a PNG for each.
        assert!(matches!(listed, Ok(Ran::Image(answer, pngs))
            if answer == format!("[{captured}]") && pngs.len() == 1));
        let gone = captured_as(&taken, true, true)
            .err()
            .map(|failure| failure.code);
        assert_eq!(gone.as_deref(), Some("gg.io"));
    }

    /// Several captures are an array of their answers, each with a PNG of its own if inlined.
    #[test]
    fn several_captures_answer_an_array_with_each_png() {
        let dir = std::env::temp_dir();
        let paths = ["a", "b"]
            .map(|name| dir.join(format!("gg-mcp-capture-{name}-{}.png", std::process::id())));
        let taken = paths.clone().map(|path| {
            fs::write(&path, path.to_string_lossy().as_bytes()).expect("written");
            RawValue::from_string(json!({"path": path}).to_string()).expect("JSON")
        });
        let named = captured_as(&taken, true, false);
        let inlined = captured_as(&taken, true, true);
        for path in &paths {
            let _ = fs::remove_file(path);
        }
        let Ok(Ran::Json(named)) = named else {
            panic!("JSON");
        };
        let named: Value = serde_json::from_str(&named).expect("JSON");
        assert_eq!(named, json!([{"path": paths[0]}, {"path": paths[1]}]));
        let Ok(Ran::Image(_, pngs)) = inlined else {
            panic!("images");
        };
        let want = paths.map(|path| path.to_string_lossy().into_owned().into_bytes());
        assert_eq!(pngs, want);
    }

    /// A `show` is a name, an image's name, which may hold spaces, or an object of one, in a
    /// capture's and a look's schema alike; and the capture's takes an array.
    #[test]
    fn show_is_listed_for_capture_and_look_and_read_as_the_apis_shown() {
        use gg_api::Shown;

        let listed = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let listed = answer(&mut pick(), listed).expect("answered");
        let tools = listed["result"]["tools"].as_array().expect("tools");
        let show = |name: &str| {
            let tool = tools.iter().find(|tool| tool["name"] == name);
            tool.unwrap_or_else(|| panic!("a `{name}` tool"))["inputSchema"]["properties"]["show"]
                .clone()
        };
        let (capture, look) = (show("capture"), show("look"));
        assert!(capture["anyOf"][1]["type"] == "array", "{capture}");
        assert!(look["anyOf"][1]["type"] == "null", "{look}");
        // Each of the API's variants is named in both descriptions.
        let variants = [
            Shown::BaseColor,
            Shown::Normal,
            Shown::SurfaceNormal,
            Shown::Metallic,
            Shown::Roughness,
            Shown::Occlusion,
            Shown::Emission,
            Shown::SpecularColor,
            Shown::DirectLight,
            Shown::SkyLight,
            Shown::Reflection,
            Shown::DiffuseLight,
            Shown::SpecularLight,
        ];
        for description in [&capture["description"], &look["description"]] {
            let description = description.as_str().expect("described");
            for variant in &variants {
                let name = serde_json::to_value(variant).expect("a name");
                let name = name.as_str().expect("a string");
                assert!(description.contains(&format!("`{name}`")), "{name}");
            }
        }
        let shows = shows_of(&[
            json!("roughness"),
            json!("view depth"),
            json!({"image": "id"}),
        ]);
        let want = [
            json!("roughness"),
            json!({"image": "view depth"}),
            json!({"image": "id"}),
        ];
        assert_eq!(shows.expect("shows"), want);
        assert!(shows_of(&[json!(3)]).is_err());
    }

    /// A look's `skip` takes a list of the passes, by the API's names, or null.
    #[test]
    fn skip_is_listed_for_look_by_the_apis_names() {
        let listed = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let listed = answer(&mut pick(), listed).expect("answered");
        let tools = listed["result"]["tools"].as_array().expect("tools");
        let look = tools.iter().find(|tool| tool["name"] == "look");
        let skip = &look.expect("a `look` tool")["inputSchema"]["properties"]["skip"];
        assert_eq!(skip["type"], json!(["array", "null"]));
        let names = gg_api::Pass::ALL.map(|pass| serde_json::to_value(pass).expect("a name"));
        assert_eq!(skip["items"]["enum"], json!(names));
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
        assert!(tools.iter().any(|tool| tool["name"] == "ui"), "a `ui` tool");
        let unknown = super::answer(&mut pick, r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#);
        assert_eq!(unknown.expect("answered")["error"]["code"], -32601);
        let broken = super::answer(&mut pick, "{").expect("answered");
        assert_eq!(broken["error"]["code"], -32700);
    }

    /// Each tool is a command's, which `gg help` names and gives a section; `save_prefab` is
    /// `save-prefab`'s.
    #[test]
    fn each_tool_is_a_command_with_help() {
        for tool in tools().as_array().expect("tools") {
            let name = tool["name"].as_str().expect("a name").replace('_', "-");
            assert!(crate::section(&name).is_ok(), "{name}");
        }
    }

    /// `probe` is listed, taking a pixel and whether to probe the game's own view, and describing
    /// each field it answers; one given a pixel that isn't two numbers, not negative, or an `own`
    /// that isn't a boolean, fails with `gg.args` before it looks for a game.
    #[test]
    fn probe_is_listed_with_its_fields_and_refuses_odd_arguments() {
        let listed = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let listed = answer(&mut pick(), listed).expect("answered");
        let tools = listed["result"]["tools"].as_array().expect("tools");
        let probe = tools.iter().find(|tool| tool["name"] == "probe");
        let probe = probe.expect("a `probe` tool");
        assert_eq!(probe["inputSchema"]["required"], json!(["at"]));
        assert_eq!(probe["inputSchema"]["properties"]["at"]["minItems"], 2);
        assert_eq!(probe["inputSchema"]["properties"]["own"]["type"], "boolean");
        let description = probe["description"].as_str().expect("described");
        for field in [
            "at",
            "size",
            "ticks",
            "base_color",
            "emission",
            "normal",
            "surface_normal",
            "metallic",
            "roughness",
            "occlusion",
            "direct_light",
            "sky_light",
            "reflection",
            "shown",
            "depth",
            "distance",
            "entity",
            "quantized",
        ] {
            assert!(description.contains(&format!("`{field}`")), "{field}");
        }
        for arguments in [
            r#"{}"#,
            r#"{"at":[1]}"#,
            r#"{"at":[1,"2"]}"#,
            r#"{"at":[-1,2]}"#,
            r#"{"at":[1,2],"own":"yes"}"#,
        ] {
            let call = format!(
                r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"probe","arguments":{arguments}}}}}"#
            );
            let answer = answer(&mut pick(), &call).expect("answered");
            assert_eq!(answer["result"]["isError"], true, "{arguments}");
            let text = answer["result"]["content"][0]["text"].as_str();
            assert!(
                text.is_some_and(|text| text.contains("gg.args")),
                "{arguments}"
            );
        }
    }

    /// `debug` is listed, its actions those gg-api's `debug` takes.
    #[test]
    fn debug_is_listed_with_the_apis_actions() {
        use gg_api::DebugAction::{Continue, Pause, StepInto, StepOut, StepOver};

        let listed = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let listed = answer(&mut pick(), listed).expect("answered");
        let tools = listed["result"]["tools"].as_array().expect("tools");
        let debug = tools.iter().find(|tool| tool["name"] == "debug");
        let actions =
            &debug.expect("a `debug` tool")["inputSchema"]["properties"]["action"]["enum"];
        assert_eq!(
            *actions,
            json!([Pause, Continue, StepInto, StepOver, StepOut])
        );
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
        assert_eq!(schema("cells"), nothing);
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
            InputEvent::Motion([3.0, -4.0]),
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
                    | InputEvent::Motion(_)
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
            r#"{"inline":"yes"}"#,
            r#"{"show":""}"#,
            r#"{"show":[]}"#,
            r#"{"show":["normal",3]}"#,
            r#"{"show":{"other":"x"}}"#,
        ] {
            let name = if arguments.contains("inline") || arguments.contains("show") {
                "capture"
            } else {
                "logs"
            };
            let call = format!(
                r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
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
