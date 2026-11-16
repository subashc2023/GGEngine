//! A request read into the call the main loop answers, its parameters checked.

use std::fmt::Display;
use std::path::PathBuf;

use gg_api::asset::AssetKind;
use gg_api::{
    Apply, Capture, Failure, Field, Input, InputEvent, Request, SavePrefab, Scene, SceneAction,
    Step, View,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use subsecond::JumpTable;

use super::failure;
use super::view::{checked_viewpoint, look, shown};
use crate::{EntityId, Key};

/// The methods, as `api.unknown_method` lists them.
pub(super) const METHODS: [&str; 35] = [
    "hello",
    "ping",
    "status",
    "pause",
    "resume",
    "play",
    "stop",
    "step",
    "scene",
    "capture",
    "frames",
    "view",
    "input",
    "look",
    "pick",
    "images",
    "entities",
    "entity",
    "get",
    "types",
    "hash",
    "cells",
    "assets",
    "ui",
    "snapshot",
    "patch",
    "apply",
    "save_prefab",
    "undo",
    "redo",
    "edits",
    "logs",
    "note",
    "debug",
    "quit",
];

/// The most ticks a `step` runs.
pub(super) const MOST_STEPS: u64 = 10_000;

/// The most entities or assets a page of `entities` or `assets` lists.
const MOST_LISTED: usize = 10_000;

/// The requests the main loop answers, as read.
#[derive(Debug, PartialEq)]
pub(super) enum Call {
    Status,
    Pause,
    Resume,
    Play,
    Stop,
    Step(u64),
    /// What to do to the open scene, with the path to open or save it as (checked to be a `.ron`
    /// file's) and whether to discard its unsaved edits.
    Scene(Scene),
    Capture {
        path: Option<PathBuf>,
        /// A pixel of the view to answer the bytes of; finite and not negative.
        at: Option<[f64; 2]>,
        own: bool,
        show: Option<gg_api::Shown>,
    },
    /// Frames in a row: the directory for their PNGs, how many, their viewpoints (each checked,
    /// its rotation normalized), the part of each kept (of some area), and what they show.
    Frames {
        dir: PathBuf,
        count: u32,
        viewpoints: Vec<gg_api::Viewpoint>,
        rect: Option<[u32; 4]>,
        show: Option<gg_api::Shown>,
    },
    /// The name of the memory to publish the view to, or `None` to stop.
    View(Option<String>),
    /// Events to hand the layers; each key event names a real key.
    Input(Vec<InputEvent>),
    /// The parts of the look to set, each checked, the viewpoint's rotation normalized.
    Look(gg_api::Look),
    /// The pixel of the view to pick at, and the entity whose model to pick within, if any.
    Pick([f64; 2], Option<EntityId>),
    Images,
    Entities {
        with: Option<String>,
        skip: usize,
        take: usize,
    },
    Entity(EntityId),
    Get {
        id: EntityId,
        component: String,
        path: String,
    },
    Types,
    Hash,
    Cells,
    Assets {
        kind: Option<AssetKind>,
        skip: usize,
        take: usize,
    },
    Ui,
    /// A snapshot at the path, if given, and whether the run ends once it's written.
    Snapshot {
        path: Option<PathBuf>,
        quit: bool,
    },
    Patch(JumpTable),
    /// An edit's operations, and the gesture it's part of, if it's sets of one field.
    Apply(Vec<gg_api::Operation>, Option<u64>),
    /// The entity to save as a prefab and link to it, and the prefab file's path (checked to be a
    /// `.prefab` file's).
    SavePrefab {
        id: EntityId,
        path: Option<PathBuf>,
    },
    /// An undo or redo of the named author's edits, else the client's own.
    Undo {
        author: Option<String>,
        redo: bool,
    },
    Edits(Option<String>),
    Quit,
}

/// The call `request` makes of the main loop.
///
/// Fails with:
///
/// - `api.unknown_method`: an unknown method.
/// - `api.bad_params`: parameters the method doesn't take; an invalid `look` ([`look`]); a `scene`
///   whose path is missing, given where none is taken, or not a `.ron` file's, or whose `discard`
///   is given to `save`; a `save_prefab` whose path isn't a `.prefab` file's; a `frames` whose
///   `dir` isn't an absolute path to a directory, whose `count` is 0 or past
///   [`gg_api::MOST_FRAMES`], or whose viewpoints or `rect` (of no area) are invalid.
/// - `api.bad_request`: a `hello` after the first.
/// - `ecs.bad_id`: an invalid ID.
pub(super) fn parse(request: &Request) -> Result<Call, Failure> {
    let method = request.method.as_str();
    let params = &request.params;
    let call = match method {
        "status" => none(method, params).map(|()| Call::Status)?,
        "pause" => none(method, params).map(|()| Call::Pause)?,
        "resume" => none(method, params).map(|()| Call::Resume)?,
        "play" => none(method, params).map(|()| Call::Play)?,
        "stop" => none(method, params).map(|()| Call::Stop)?,
        "types" => none(method, params).map(|()| Call::Types)?,
        "hash" => none(method, params).map(|()| Call::Hash)?,
        "cells" => none(method, params).map(|()| Call::Cells)?,
        "ui" => none(method, params).map(|()| Call::Ui)?,
        "snapshot" => {
            let gg_api::Snapshot { path, quit } = take(method, params)?;
            if path.as_deref().is_some_and(|path| !path.is_absolute()) {
                let what = "takes an absolute path, since the game's working directory may not be \
                            the tool's";
                return Err(bad_params(method, what));
            }
            Call::Snapshot { path, quit }
        }
        "patch" => {
            let gg_api::Patch { table } = take(method, params)?;
            let table: JumpTable = serde_json::from_value(table).map_err(|error| {
                let what = "takes as its `table` the jump table that `dx tools hotpatch` printed";
                bad_params(method, format!("{what}: {error}"))
            })?;
            if !table.lib.is_absolute() {
                let what = "takes a table whose `lib` is an absolute path, since the OS would look \
                            for a library named so where it finds any";
                return Err(bad_params(method, what));
            }
            Call::Patch(table)
        }
        "apply" => {
            let Apply {
                operations,
                gesture,
            } = take(method, params)?;
            let mut seen = Vec::new();
            let mut field = None;
            let one_field = operations.iter().all(|operation| match operation {
                gg_api::Operation::Set {
                    id,
                    component,
                    path,
                    ..
                } => {
                    let of = *field.get_or_insert((component, path));
                    let new = !seen.contains(&id);
                    seen.push(id);
                    of == (component, path) && new
                }
                _ => false,
            });
            if gesture.is_some() && !(one_field && !operations.is_empty()) {
                let what = "takes sets of one component's one field, no two of one entity, as a \
                            gesture's";
                return Err(bad_params(method, what));
            }
            Call::Apply(operations, gesture)
        }
        "save_prefab" => {
            let SavePrefab { id, path } = take(method, params)?;
            let prefab = |ext: &std::ffi::OsStr| ext.eq_ignore_ascii_case("prefab");
            if path
                .as_deref()
                .is_some_and(|path| !path.extension().is_some_and(prefab))
            {
                let what = "takes the path of a prefab's `.prefab` file";
                return Err(bad_params(method, what));
            }
            Call::SavePrefab {
                id: entity_id(&id)?,
                path,
            }
        }
        "undo" | "redo" => Call::Undo {
            author: take::<gg_api::Undo>(method, params)?.author,
            redo: method == "redo",
        },
        "edits" => Call::Edits(take::<gg_api::Edits>(method, params)?.author),
        "scene" => {
            let scene: Scene = take(method, params)?;
            let what = match (scene.action, &scene.path) {
                (SceneAction::Open, None) => Some("takes a `path` to open"),
                (SceneAction::New, Some(_)) => Some("takes no `path` to empty the open scene"),
                (SceneAction::Save, _) if scene.discard => {
                    Some("takes `discard` to open a file or empty the open scene alone")
                }
                _ => None,
            };
            if let Some(what) = what {
                return Err(bad_params(method, what));
            }
            let ron = |ext: &std::ffi::OsStr| ext.eq_ignore_ascii_case("ron");
            if scene
                .path
                .as_deref()
                .is_some_and(|path| !path.extension().is_some_and(ron))
            {
                let what = "takes the path of a scene's `.ron` file";
                return Err(bad_params(method, what));
            }
            Call::Scene(scene)
        }
        "quit" => none(method, params).map(|()| Call::Quit)?,
        "step" => {
            let Step { ticks } = take(method, params)?;
            if !(1..=MOST_STEPS).contains(&ticks) {
                let what = format!("runs from 1 to {MOST_STEPS} ticks, not {ticks}");
                return Err(bad_params(method, what));
            }
            Call::Step(ticks)
        }
        "capture" => {
            let Capture {
                path,
                at,
                own,
                show,
            } = take(method, params)?;
            if path.as_deref().is_some_and(|path| !path.is_absolute()) {
                let what = "takes an absolute path, since the game's working directory may not be \
                            the tool's";
                return Err(bad_params(method, what));
            }
            if at.is_some_and(|at| at.iter().any(|axis| !axis.is_finite() || *axis < 0.0)) {
                let what = "takes an `at` of finite numbers that aren't negative";
                return Err(bad_params(method, what));
            }
            if let Some(show) = &show {
                shown(method, show)?;
            }
            Call::Capture {
                path,
                at,
                own,
                show,
            }
        }
        "frames" => {
            let gg_api::Frames {
                dir,
                count,
                mut viewpoints,
                rect,
                show,
            } = take(method, params)?;
            if !dir.is_absolute() {
                let what = "takes an absolute `dir`, since the game's working directory may not be \
                            the tool's";
                return Err(bad_params(method, what));
            }
            // Checked before capture, not as the PNGs fail to write.
            if !dir.is_dir() {
                let what = format!(
                    "takes a `dir` that's a directory, as {} isn't",
                    dir.display()
                );
                return Err(bad_params(method, what));
            }
            if !(1..=gg_api::MOST_FRAMES).contains(&count) {
                let what = format!(
                    "captures from 1 to {} frames, not {count}",
                    gg_api::MOST_FRAMES
                );
                return Err(bad_params(method, what));
            }
            for viewpoint in &mut viewpoints {
                checked_viewpoint(method, viewpoint)?;
            }
            if let Some([.., width, height]) = rect
                && (width == 0 || height == 0)
            {
                return Err(bad_params(method, "takes a `rect` with an area"));
            }
            if let Some(show) = &show {
                shown(method, show)?;
            }
            Call::Frames {
                dir,
                count,
                viewpoints,
                rect,
                show,
            }
        }
        "look" => Call::Look(look(method, take(method, params)?)?),
        "pick" => {
            let gg_api::Pick { at, within } = take(method, params)?;
            let within = within.map(|id| id.parse::<EntityId>()).transpose();
            let within = within.map_err(|error| {
                bad_params(method, format!("takes `within`, an entity's ID: {error}"))
            })?;
            Call::Pick(at, within)
        }
        "images" => none(method, params).map(|()| Call::Images)?,
        "view" => {
            let View { memory } = take(method, params)?;
            Call::View(memory)
        }
        "input" => {
            let Input { events } = take(method, params)?;
            let unknown = events.iter().find_map(|given| match given {
                InputEvent::Key { key, .. } if Key::from_name(key).is_none() => Some(key),
                _ => None,
            });
            if let Some(key) = unknown {
                let what =
                    format!("names keys by their W3C `code`, as `KeyW`, and `{key}` is none");
                return Err(bad_params(method, what));
            }
            Call::Input(events)
        }
        "entities" => {
            let gg_api::Entities { with, skip, take } = self::take(method, params)?;
            if take > MOST_LISTED {
                let what = format!("lists at most {MOST_LISTED} entities a page, not {take}");
                return Err(bad_params(method, what));
            }
            Call::Entities { with, skip, take }
        }
        "entity" => {
            let gg_api::Entity { id } = take(method, params)?;
            Call::Entity(entity_id(&id)?)
        }
        "assets" => {
            let gg_api::Assets { kind, skip, take } = self::take(method, params)?;
            if take > MOST_LISTED {
                let what = format!("lists at most {MOST_LISTED} assets a page, not {take}");
                return Err(bad_params(method, what));
            }
            Call::Assets { kind, skip, take }
        }
        "get" => {
            let Field {
                id,
                component,
                path,
            } = take(method, params)?;
            let id = entity_id(&id)?;
            Call::Get {
                id,
                component,
                path,
            }
        }
        "hello" => {
            let message = "`hello` comes once, as a connection's first request";
            return Err(Failure::new("api.bad_request", message));
        }
        _ => {
            let message = format!(
                "there's no method `{method}`; the methods are {}",
                METHODS.join(", ")
            );
            return Err(Failure::new("api.unknown_method", message));
        }
    };
    Ok(call)
}

/// Method `method`'s parameters, `params`, as a `T`; its defaults stand for what isn't given.
pub(super) fn take<T: DeserializeOwned>(method: &str, params: &Value) -> Result<T, Failure> {
    let params = match params {
        Value::Null => Value::Object(serde_json::Map::new()),
        given => given.clone(),
    };
    serde_json::from_value(params).map_err(|error| bad_params(method, format!("takes {error}")))
}

/// Checks that `method`, which takes nothing, is given nothing: no parameters, `null` or `{}`.
fn none(method: &str, params: &Value) -> Result<(), Failure> {
    match params {
        Value::Null => Ok(()),
        Value::Object(map) if map.is_empty() => Ok(()),
        _ => Err(bad_params(method, "takes nothing")),
    }
}

pub(super) fn bad_params(method: &str, what: impl Display) -> Failure {
    Failure::new("api.bad_params", format!("`{method}` {what}"))
}

/// The entity ID `text` gives.
///
/// Fails with `ecs.bad_id` if it gives none.
fn entity_id(text: &str) -> Result<EntityId, Failure> {
    text.parse().map_err(|error| failure(&error))
}
