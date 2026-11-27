//! `gg mcp`: the tool API served to MCP agents over stdio, as JSON-RPC 2.0 lines. Each tool is an
//! API method, or `gg`'s `sessions`, and answers JSON text, a capture its PNG too if inlined. Each
//! call connects afresh, so a restarted `gg run` is reached. Edits are authored as the client
//! `initialize` names, else `mcp`.

use std::fs;
use std::io::{self, BufRead, Write as _};
use std::time::Duration;

use gg_api::{Failure, LogEvents};
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::game::{self, Pick};
use crate::{probe, runs};

/// The MCP versions served, latest first: the client's if listed, else the latest. All agree on
/// the three methods served; not 2025-03-26, whose servers take batches.
const VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2024-11-05"];

/// The last events `logs` gives without `from`.
const MOST_LOG_LINES: usize = 200;

/// The tools' `show`, as gg-api's `Shown` says.
const SHOWN: &str = "\
A part of lit shading by name: `base_color` (sRGB, as a texture stores it), `normal` (which way \
the point faces in the world, with its normal map: each axis -1 to 1 as byte / 255 * 2 - 1, X red, \
Y green, Z blue), `surface_normal` (the same without the map), `metallic`, `roughness` (0.045 to \
1) and `occlusion` (each 0 to 1 as byte / 255, gray), `emission` (as `base_color`, past 1 shown as \
1), `specular_color` (what it reflects head-on, as `base_color`), `direct_light` (the frame's \
lights', shadows and all), `sky_light`, `reflection`, `traced_reflection` (before its blur, at any \
roughness, black where its ray found nothing), `reflection_rays` (why each ray found what it did: \
green trusted, red passed behind what's there, orange out of the last frame's view, magenta from \
behind a surface no back face bounds, gray nothing opaque, yellow its own surface, cyan turned \
back, navy past its reach, blue off the view, white out of steps), `diffuse_light` and \
`specular_light` (the light it diffuses and reflects, of all three), each light exposed and \
tonemapped as the view's. An \
unlit material shows its base color, and black for the rest. Or `path_traced`: the light as \
1,024 paths a pixel traced through its models find it, a reference for the rest, black without \
ray queries; a capture waits up to 25 s for them. Or \
an image the frame makes, by debug name: any other string, as `view depth`, or `{\"image\": name}` \
(at most 64 bytes), read by its kind: colors as they are, HDR tonemapped, depth as 1 + log2(depth) \
/ 24 (white at the near plane, a 24th darker per doubling of distance, black where nothing drew), \
IDs and visibility a color per value (black for 0), motion and normals as `normal`, a float as its \
value. An \
image the frame doesn't make shows black, warning `renderer.no_image`. A part of shading is drawn \
without antialiasing, fog, depth of field, motion blur, bloom, grade, lens, exposure or sky (black); \
an image, from a frame drawn as the game's but without antialiasing, so every other pass's images \
are there, as `bloom 1` or `depth of field near`.";

/// What `initialize` tells the client's model.
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

/// Serves until stdin ends. Fails with `gg.io` if stdin or stdout breaks.
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

/// None for a notification or a response.
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
        // We make no requests, so a result or error is a stray response; else not a message.
        let response = message.get("result").is_some() || message.get("error").is_some();
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        return (!response).then(|| rpc_error(id, -32600, "not a JSON-RPC request"));
    };
    // No ID, as `notifications/initialized`: a notification, unanswered.
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

/// Takes the client's `clientInfo` name as the edits' author.
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

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// `properties` and `required` make the input's JSON Schema.
struct Tool {
    name: &'static str,
    description: &'static str,
    properties: fn() -> Value,
    required: &'static [&'static str],
}

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
                      it was saved or opened, which ticks don't; in a world of cells, `cells`: how \
                      many are in, and how many out keep what they held; and `cursor_grabbed`, \
                      whether the game wants the cursor grabbed, so it looks about by `motion` \
                      events, not `cursor` moves.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "wait",
        description: "Call after editing the game's code, scenes or scripts, before checking \
                      what the edit does: waits until the game under `gg run` runs the code of \
                      every edit saved before the call, patched, or rebuilt and restarted, and has \
                      looked at its scene and script files since, and answers what the run is \
                      doing. Its `reload` is the last patch (`patched`, with its `files`, \
                      `build_ms` and `apply_ms`) or rebuild (`rebuilt`, with its `why` and \
                      `build_ms`), with `at`, ms since the epoch, as `updated`. `files_looked` is \
                      when the game last looked at its scene and script files, and `files` the \
                      last event it logged of each, the latest 20 files', with its `timestamp`: \
                      `scene.applied`, `Script reloaded`, a warning why one didn't apply, a \
                      script's error. Fails with \
                      `gg.build_failed` and rustc's errors if the edits don't build (the game runs \
                      its old code), `gg.waiting` if the game ended and waits for a change, or \
                      `gg.timeout`.",
        properties: || {
            json!({"timeout_s": {"type": "integer", "minimum": 1, "default": 300,
                                 "description": "How long to wait, in seconds"}})
        },
        required: &[],
    },
    Tool {
        name: "rebuild",
        description: "Call after an edit `wait` says was patched in, but that the game won't run \
                      again, as in a function reached only through the game's own trait objects \
                      or function pointers (`gg run` rebuilds by itself for code only `main` or a \
                      layer's `load` or `restored` reaches). Has \
                      `gg run` rebuild the game with every edit saved, once no patch or build is \
                      under way, and start it again from a snapshot, as an edit no patch carries \
                      does. Then waits until the game runs the new build, and answers as `wait` \
                      does, its `reload` the rebuild. Fails with `gg.build_failed` and rustc's \
                      errors if it doesn't build (the game runs its old code), `gg.no_rebuild` if \
                      the run doesn't build the game itself (`--no-patch`, or restarts off), \
                      `gg.waiting` if the game ended as it started again, or `gg.timeout`. With \
                      `fresh`, the game starts afresh instead, its layers' `attach` building the \
                      world again: for an edit in `attach` (`gg.attach_changed`). The world's \
                      state is lost.",
        properties: || {
            json!({"timeout_s": {"type": "integer", "minimum": 1, "default": 300,
                                 "description": "How long to wait, in seconds"},
                   "fresh": {"type": "boolean", "default": false,
                             "description": "Start the game afresh, not from a snapshot"}})
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
        name: "overlays",
        description: "Turns the game's debug overlays on and off, those its key below Esc's bar \
                      offers in a development build with `App::debug_overlays`, and answers each \
                      overlay's `name`, `group` (stats, inspect, world, game), whether it's \
                      `on` and, for a World overlay on, the lines and labels it `drew` in the last \
                      frame (0: nothing to show), the names on that no overlay has yet \
                      (`waiting`), the `channel`, the \
                      passes left out (`skip`) and whether the `bar` shows. World overlays \
                      (colliders, contacts, velocities, navigation, cells, bounds, axes, names, \
                      skeletons, cameras, lights, sounds) draw with the world, so `capture` shows \
                      them; stats and panels are UI, which it doesn't. Kept for the game's next \
                      runs. Leave every part out to read them.",
        properties: || {
            json!({
                "on": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Names to turn on, in any case, `_` or `-` for a space: the \
                                    engine's, such as `fps`, `memory`, `log`, `inspector`, \
                                    `colliders`, `velocities`, `bounds` or `names`, or the \
                                    game's own, which waits until the game asks for it",
                },
                "off": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Names to turn off; a name in both `on` and `off` is refused",
                },
                "only": {
                    "type": "boolean",
                    "description": "Turn every overlay off before `on` and `off` apply",
                },
                "channel": {
                    "type": ["string", "null"],
                    "enum": ["base_color", "normal", "surface_normal", "metallic", "roughness",
                             "occlusion", "emission", "specular_color", "direct_light",
                             "sky_light", "reflection", "diffuse_light", "specular_light",
                             "traced_reflection", "reflection_rays", null],
                    "description": "The part of shading the view shows in place of its light, as \
                                    the bar's Render menu picks it; null for the light. The look's \
                                    `show` wins over it",
                },
                "skip": {
                    "type": "array",
                    "items": {"enum": passes()},
                    "description": "The passes the view leaves out, in place of those before, \
                                    with the look's own; `[]` for none",
                },
                "bar": {
                    "type": "boolean",
                    "description": "Whether the overlays' bar shows over the top of the view",
                },
            })
        },
        required: &[],
    },
    Tool {
        name: "pick",
        description: "The entity the view shows at a pixel, as it's shown, the look included: its \
                      ID and name, then its `parent` and `parent_name`, as a unit's for its \
                      shape; all null outside the view or where none draws. A model's node gives \
                      the model's root, which tools change in its place. Answers about four \
                      frames on, and fails while the game runs no frames, as while its window is \
                      minimized.",
        properties: || {
            json!({
                "at": numbers(2, "The pixel, [x, y] from the view's top left, as a capture's PNG \
                                  has it"),
                "within": {"type": "string",
                           "description": "The ID or name of the model's root, or of one of its nodes: \
                                           answers the node drawn at the pixel, not the root"},
            })
        },
        required: &["at"],
    },
    Tool {
        name: "cast",
        description: "What a ray from the view's camera through a pixel, or the view's middle where \
                      a crosshair is, hits first of the colliders, sensors skipped, a 2D one \
                      where it holds the point the ray crosses z = 0: the entity's ID and name, \
                      the `point` hit, the surface's `normal` there and the `distance` from the \
                      camera; ID and name null for none. Reads the camera the last frame drew \
                      and the colliders as the last tick left them, as a game's shot does: what \
                      a shot would hit, where `pick` says what shows, a HUD included. Fails with \
                      api.no_view before a frame has drawn the view.",
        properties: || {
            json!({
                "at": numbers(2, "The pixel, [x, y] from the view's top left, as `pick` takes \
                                  it; the view's middle without it"),
                "exclude": {"type": "string",
                            "description": "The ID or name of an entity whose colliders the ray \
                                            passes through, as a player's in front of the camera. \
                                            Colliders the camera is inside are passed through \
                                            without it"},
            })
        },
        required: &[],
    },
    Tool {
        name: "project",
        description: "Where the view shows an entity, by the middle of what it draws, else its \
                      origin, or a world point, from the camera the last frame drew: `at`, the \
                      pixel as `pick` takes it, null outside the view or behind the camera; `turn`, \
                      the radians [right, up] the camera turns to face it, about its own axes, \
                      null without perspective; `distance` from the camera. Hidden or not. To aim \
                      a camera turned by mouse motion, move by `turn` over the game's sensitivity; \
                      a camera orbiting its player moves as it turns, so project again.",
        properties: || {
            json!({
                "entity": {"type": "string", "description": "An entity's ID or name"},
                "point": numbers(3, "A world point, [x, y, z]"),
            })
        },
        required: &[],
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
                      Every value is null for a pixel outside the view. On an unlit material, \
                      each part of shading but `base_color` is 0, and normals [-1, -1, -1].",
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
                      frame starts, a `wait` running ticks between them. Release each key and \
                      button you press, and bring sticks and triggers back to rest: the game \
                      holds them until then, or give `hold`. Set the game's actions by name, as \
                      `actions` lists them, with `action` events. \
                      Without `hold` or a `wait`, it answers `{}` before a tick sees the events; \
                      paused, none does until a `step`.",
        properties: || {
            json!({
                "events": input(),
                "hold": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10000,
                    "description": "Ticks (64 a second) to hold what the events press and leave \
                                    down or away from rest, after the last event. It's let go \
                                    after exactly that many, and the answer is the status then, \
                                    as `step`'s; the next tick sees the release. With none left \
                                    down, such as a click's, it just runs the ticks. Paused, the \
                                    hold steps its ticks; refused while stopped.",
                },
            })
        },
        required: &["events"],
    },
    Tool {
        name: "actions",
        description: "The game's controls, to learn them without reading its code. `actions`: \
                      each `name`, its `layer`, `kind` (`button`; `axis`, -1 to 1; `axis2`, \
                      [x, y]), `does` (what its value moves, in what units, and when) and \
                      `bindings`, each a list of sources named as `input` hands them: a button's \
                      one; an axis's negative then positive; a pair of axes' left, right, down \
                      and up. Set one by name with `input`'s `action` event. `controls`: what the \
                      layers read apart from actions, each with its `layer`, `reads` (a source, \
                      or \"motion\", \"scroll\" or \"cursor\", handed as `input`'s events of those \
                      names) and `does`. A layer lists them through `Layer::actions`; a game's \
                      layer that doesn't lists none.",
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "press",
        description: "Clicks a `.ui` button or widget by name, at the middle of what shows of it, \
                      as a player would: a button presses, a toggle flips, a drop-down opens or \
                      closes, a text field takes the focus, a slider takes its middle value. \
                      Answers the `ui` and `node`, the pixel clicked `at`, the `ticks`, and \
                      `events`: what the UIs gave the game's layers, each `{\"ui\", \"kind\", \
                      \"node\", \"value\"}` with `kind` `pressed`, `focused`, `changed` or `back`. \
                      Empty `events` means no UI took the click, as when another covers it. \
                      Refused, clicking nothing, for a node that's disabled (`ui.disabled`), \
                      doesn't show (`ui.hidden`), isn't a button or widget (`ui.wrong_kind`), or \
                      whose UI takes no input or didn't draw (`ui.no_input`).",
        properties: || {
            json!({
                "node": {"type": "string", "description": "The node's name, as `ui` lists it"},
                "ui": {
                    "type": "string",
                    "description": "The UI's name, as `ui` lists it: needed only where several \
                                    UIs have a node of the name",
                },
                "ticks": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10000,
                    "description": "Ticks (64 a second) to run after the click before answering, \
                                    as `step` does, so the game has acted on it; refused while \
                                    stopped",
                },
            })
        },
        required: &["node"],
    },
    Tool {
        name: "entities",
        description: "A page of the world's entities: each one's ID, name, parent, children and \
                      components' types, and with `fields`, each one's values of them, as `get` \
                      reads them: many entities' positions or health in one call.",
        properties: || {
            json!({
                "with": {"type": "string", "description": "Only those with this component"},
                "skip": {"type": "integer", "minimum": 0, "default": 0},
                "take": {"type": "integer", "minimum": 0, "maximum": 10000, "default": 100},
                "fields": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Fields to read of each entity, answered in its `fields` by \
                                    field: a component's type for the whole component, or its \
                                    type, `.` and a path, as `Transform.translation` or \
                                    `Sprite.rect[1]`. An entity without the component goes \
                                    without the field.",
                },
            })
        },
        required: &[],
    },
    Tool {
        name: "entity",
        description: "An entity: its ID, name, parent, children and components' values; \
                      `bounds`, the sphere round what it and those under it draw, and `box`, the \
                      box along the world's axes round the same, `{\"min\", \"max\"}`; and \
                      `touching`, the entities its colliders touched at the last step of physics \
                      and those a character hit as it moved (none standing still), each \
                      `{\"id\", \"name\", \"sensor\"}`. For an instance's entity (its root, or \
                      one its prefab gives), `instance` too: the \
                      outermost instance's `root` and `prefab`; `link`, its own prefab, if it's an \
                      instance's root; `overrides`, each component differing from its prefab with \
                      the fields that differ (`\"\"` for the whole), as `revert` takes them; and \
                      `added`, the components its prefab doesn't give.",
        properties: || json!({"id": entity()}),
        required: &["id"],
    },
    Tool {
        name: "get",
        description: "A component's value, or its field at a path, such as `translation.x` or \
                      `rect[1]`. The path may instead follow the component's type and `.`, as \
                      `Transform.translation.x`.",
        properties: || {
            json!({
                "id": entity(),
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
                      game's code set. Press a button or widget by name with `press`; type into a \
                      text field pressed so with `input`'s `text`; scroll a list under the cursor \
                      with `input`'s `scroll`.",
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
                "id": entity(),
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
                      component for an empty path, as an edit `undo` takes back. The path may \
                      instead follow the component's type and `.`, as `Transform.translation.x`.",
        properties: || {
            json!({
                "id": entity(),
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

/// An entity, as the tool API takes one.
fn entity() -> Value {
    json!({"type": "string",
           "description": "An entity's ID, 16 hex digits, or else the `Name` of the one entity \
                           with it"})
}

fn id() -> Value {
    json!({"type": "string", "description": "An entity's ID, 16 hex digits"})
}

fn author() -> Value {
    json!({
        "type": "string",
        "description": "Whose edits: an MCP client's name, `gg`, or a scene file's path, such as \
                        `scenes/grid.ron`; yours without one",
    })
}

fn numbers(count: usize, description: &str) -> Value {
    json!({
        "type": "array",
        "items": {"type": "number"},
        "minItems": count,
        "maxItems": count,
        "description": description,
    })
}

/// An object of one key, as gg-api writes an enum's variant with a value.
/// Each gamepad button as `input` names it.
const PAD_BUTTONS: [&str; 17] = [
    "south",
    "east",
    "north",
    "west",
    "left_shoulder",
    "right_shoulder",
    "left_trigger",
    "right_trigger",
    "select",
    "start",
    "mode",
    "left_thumb",
    "right_thumb",
    "dpad_up",
    "dpad_down",
    "dpad_left",
    "dpad_right",
];

/// The schema of an input event's gamepad.
fn pad() -> Value {
    json!({
        "type": "integer",
        "minimum": 0,
        "default": 0,
        "description": "The gamepad's number",
    })
}

fn variant(name: &str, value: Value) -> Value {
    json!({
        "type": "object",
        "properties": {name: value},
        "required": [name],
        "additionalProperties": false,
    })
}

fn shown() -> Value {
    json!({"anyOf": [
        {"type": "string", "minLength": 1, "maxLength": 64},
        variant("image", json!({"type": "string", "minLength": 1, "maxLength": 64})),
    ]})
}

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

/// Built from the API, so the list can't fall behind it.
fn passes() -> Vec<Value> {
    (gg_api::Pass::ALL.iter())
        .map(|pass| serde_json::to_value(pass).expect("a pass's name"))
        .collect()
}

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
            variant("wait", json!({
                "type": "integer",
                "minimum": 0,
                "maximum": 10000,
                "description": "Ticks (64 a second) to run before the events after it, stepped \
                                while paused: a key held across a jump, a click once a unit has \
                                walked. The answer is then the status once the last event is \
                                handed or the hold ends, as `step`'s. The waits and `hold` run \
                                10,000 ticks at most together",
            })),
            variant("aim", json!({
                "anyOf": [
                    {"type": "string"},
                    {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3},
                ],
                "description": "The ID or name of an entity, or a point of the world's [x, y, z], \
                                to move the cursor to, at the pixel `project` answers for it, read \
                                as the event is handed so a moving target is aimed at where it is \
                                then. Fails with api.unseen, handing nothing after it, where the \
                                view doesn't show it",
            })),
            variant("pad_button", json!({
                "type": "object",
                "description": "A gamepad's button went down or up, by position: `south` is \
                                Xbox's A and PlayStation's Cross. A trigger's press as a button \
                                is `left_trigger` or `right_trigger`, apart from its travel",
                "properties": {
                    "pad": pad(),
                    "button": {"type": "string", "enum": PAD_BUTTONS},
                    "pressed": {"type": "boolean"},
                },
                "required": ["button", "pressed"],
                "additionalProperties": false,
            })),
            variant("pad_stick", json!({
                "type": "object",
                "description": "A gamepad's stick moved",
                "properties": {
                    "pad": pad(),
                    "stick": {"type": "string", "enum": ["left", "right"]},
                    "value": numbers(2, "[x, y]: right and up positive, of length at most 1, \
                                         [0, 0] at rest"),
                },
                "required": ["stick", "value"],
                "additionalProperties": false,
            })),
            variant("pad_trigger", json!({
                "type": "object",
                "description": "A gamepad's trigger moved",
                "properties": {
                    "pad": pad(),
                    "trigger": {"type": "string", "enum": ["left", "right"]},
                    "value": {
                        "type": "number",
                        "minimum": 0,
                        "maximum": 1,
                        "description": "0 at rest, 1 all the way",
                    },
                },
                "required": ["trigger", "value"],
                "additionalProperties": false,
            })),
            variant("action", json!({
                "type": "object",
                "description": "One of the game's actions, by name as `actions` lists it, set to \
                                `value`. It's handed as the events of the first binding that \
                                gives the value, as a player's keys or stick would, on pad 0, \
                                the others let go of: keys and buttons give -1, 0 and 1; sticks \
                                and triggers, values between. Fails with api.no_action, naming the actions, for a name \
                                no layer binds, handing nothing",
                "properties": {
                    "name": {"type": "string"},
                    "value": {"anyOf": [
                        {
                            "type": "number",
                            "minimum": -1,
                            "maximum": 1,
                            "description": "A button's 1, pressed, the default, or 0, released; \
                                            an axis's -1 to 1",
                        },
                        numbers(2, "A pair of axes' [x, y], each -1 to 1"),
                    ]},
                },
                "required": ["name"],
                "additionalProperties": false,
            })),
        ]},
    })
}

/// A tool's error is a result with `isError` set, as MCP has it.
///
/// Fails, as a protocol error, for an unknown tool or arguments that aren't an object.
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

enum Ran {
    Json(String),
    Image(String, Vec<Vec<u8>>),
}

/// Fails with `gg.args` for an unknown tool, and as the call it makes fails.
fn run(pick: &Pick, name: &str, mut arguments: Map<String, Value>) -> Result<Ran, Failure> {
    let call = |method: &str, params: Value| {
        game::call(pick, method, params).map(|result| Ran::Json(result.get().to_owned()))
    };
    match name {
        "sessions" => {
            game::sessions(pick.project.as_deref()).map(|sessions| Ran::Json(sessions.to_string()))
        }
        _ if crate::NO_PARAMETERS.contains(&name) || name == "snapshot" => call(name, Value::Null),
        // `scene`'s and `save_prefab`'s paths stay as given: from the asset root.
        "step" | "scene" | "pick" | "cast" | "project" | "input" | "press" | "entities"
        | "entity" | "get" | "apply" | "save_prefab" | "undo" | "redo" | "edits" | "assets"
        | "debug" | "overlays" => call(name, Value::Object(arguments)),
        "look" | "frames" => {
            // A name that isn't a variant's is an image's, as in `capture`.
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
        "wait" | "rebuild" => {
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
            let timeout = Duration::from_secs(timeout);
            let told = match name {
                "wait" => runs::wait(pick, timeout)?,
                _ => {
                    let fresh = match arguments.get("fresh") {
                        None | Some(Value::Null) => false,
                        Some(Value::Bool(fresh)) => *fresh,
                        Some(given) => {
                            let message = format!("`fresh` is true or false, not {given}");
                            return Err(Failure::new("gg.args", message));
                        }
                    };
                    runs::rebuild(pick, timeout, fresh)?
                }
            };
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
                // A cursor numbers one run's events; a restart starts again.
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

/// Fails with `gg.args` for a show that isn't a name or an image's.
fn shows_of(given: &[Value]) -> Result<Vec<Value>, Failure> {
    given.iter().map(game::shown).collect()
}

/// The answers, naming each PNG's path so an agent's context needn't hold every capture: an array
/// if `listed`, else the one's; with `inline`, each PNG too, in order.
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
        // A listed show answers an array, even of one.
        assert!(matches!(listed, Ok(Ran::Image(answer, pngs))
            if answer == format!("[{captured}]") && pngs.len() == 1));
        let gone = captured_as(&taken, true, true)
            .err()
            .map(|failure| failure.code);
        assert_eq!(gone.as_deref(), Some("gg.io"));
    }

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

    /// `capture`'s and `look`'s `show` take a name or an image's (spaces allowed); `capture`'s
    /// also an array.
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
        // Both descriptions name each of the API's variants.
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
            Shown::TracedReflection,
            Shown::ReflectionRays,
            Shown::PathTraced,
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

    /// `save_prefab` is `save-prefab`.
    #[test]
    fn each_tool_is_a_command_with_help() {
        for tool in tools().as_array().expect("tools") {
            let name = tool["name"].as_str().expect("a name").replace('_', "-");
            assert!(crate::section(&name).is_ok(), "{name}");
        }
    }

    /// Bad `at` or `own` fail with `gg.args` before any game is reached.
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
        assert_eq!(input["properties"]["hold"]["minimum"], 1);
        let press = schema("press");
        assert_eq!(press["required"], json!(["node"]));
        assert_eq!(press["properties"]["ticks"]["maximum"], 10000);
        let fields = &schema("entities")["properties"]["fields"];
        assert_eq!(fields["items"]["type"], "string");
        let events = &input["properties"]["events"];
        assert_eq!(events["type"], "array");
        // A choice's kind: an object's one key, or a string's values.
        let choices = events["items"]["anyOf"].as_array().expect("choices");
        let mut named: Vec<&str> = (choices.iter())
            .flat_map(|choice| match choice["enum"].as_array() {
                Some(values) => values.iter().collect::<Vec<_>>(),
                None => (choice["required"].as_array().expect("its key").iter()).collect(),
            })
            .map(|name| name.as_str().expect("a kind"))
            .collect();
        // One event of each kind; the match stays exhaustive as kinds are added.
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
            InputEvent::Wait(3),
            InputEvent::Aim(gg_api::Aimed::Entity("Player".to_owned())),
            InputEvent::PadButton {
                pad: 0,
                button: gg_api::PadButton::South,
                pressed: true,
            },
            InputEvent::PadStick {
                pad: 0,
                stick: gg_api::Stick::Left,
                value: [0.0, 1.0],
            },
            InputEvent::PadTrigger {
                pad: 0,
                trigger: gg_api::Trigger::Left,
                value: 1.0,
            },
            InputEvent::Action {
                name: "Jump".to_owned(),
                value: None,
            },
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
                    | InputEvent::Focused(_)
                    | InputEvent::Wait(_)
                    | InputEvent::Aim(_)
                    | InputEvent::PadButton { .. }
                    | InputEvent::PadStick { .. }
                    | InputEvent::PadTrigger { .. }
                    | InputEvent::Action { .. } => {}
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
        // Each button the schema names is one the protocol reads.
        for button in PAD_BUTTONS {
            let read = serde_json::from_value::<gg_api::PadButton>(json!(button));
            assert!(read.is_ok(), "{button}");
        }
    }

    /// A tool failing before a game is reached answers `isError`; an unknown tool is a protocol
    /// error.
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
