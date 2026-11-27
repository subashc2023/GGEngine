//! Reads a request into the main loop's call, checking its parameters.

use std::fmt::Display;
use std::path::PathBuf;

use gg_api::asset::AssetKind;
use gg_api::{
    Apply, Capture, Failure, Field, Input, InputEvent, Request, SavePrefab, Scene, SceneAction,
    Step, View,
};
use gg_core::math::Vec2;
use serde::de::DeserializeOwned;
use serde_json::Value;
use subsecond::JumpTable;

use super::view::{checked_viewpoint, look, shown};
use super::world::Subject;
use crate::Key;

/// Listed by `api.unknown_method`.
pub(super) const METHODS: [&str; 40] = [
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
    "press",
    "actions",
    "look",
    "overlays",
    "pick",
    "cast",
    "project",
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

/// The most items a page of `entities` or `assets` lists.
const MOST_LISTED: usize = 10_000;

/// Requests the main loop answers.
#[derive(Debug, PartialEq)]
pub(super) enum Call {
    Status,
    Pause,
    Resume,
    Play,
    Stop,
    Step(u64),
    /// An action on the open scene, its path (a checked `.ron`) and whether to discard unsaved
    /// edits.
    Scene(Scene),
    Capture {
        path: Option<PathBuf>,
        /// A view pixel to answer the bytes of; finite, not negative.
        at: Option<[f64; 2]>,
        own: bool,
        show: Option<gg_api::Shown>,
    },
    /// Consecutive frames: their PNGs' directory, count, viewpoints (checked, rotations
    /// normalized), the part kept (nonzero area) and what they show.
    Frames {
        dir: PathBuf,
        count: u32,
        viewpoints: Vec<gg_api::Viewpoint>,
        rect: Option<[u32; 4]>,
        show: Option<gg_api::Shown>,
    },
    /// The shared memory to publish the view to; `None` stops.
    View(Option<String>),
    /// Events to hand the layers at once, each key event naming a real key, and what to hand after
    /// waits or let go of after a hold, if either follows.
    Input(Vec<InputEvent>, Option<Hold>),
    /// The layers' actions to list.
    Actions,
    /// A UI's node to click, by name, the UI's name if given, and the ticks to run after, from 1
    /// to [`MOST_STEPS`].
    Press {
        node: String,
        ui: Option<String>,
        ticks: Option<u64>,
    },
    /// The look's parts to set, checked, the viewpoint's rotation normalized.
    Look(gg_api::Look),
    /// The debug overlays to turn on and off, and the Render menu's choices, checked.
    Overlays(gg_api::Overlays),
    /// The view pixel to pick at, and optionally the entity whose model to pick within.
    Pick([f64; 2], Option<Subject>),
    /// The view pixel to cast through, else its middle, and the entity whose colliders it skips.
    Cast(Option<[f64; 2]>, Option<Subject>),
    /// An entity, or a finite world point, to find in the view.
    Project(Projecting),
    Images,
    Entities {
        with: Option<String>,
        skip: usize,
        take: usize,
        /// Each field's component type and path, split at the first `.`.
        fields: Vec<(String, String)>,
    },
    Entity(Subject),
    Get {
        id: Subject,
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
    /// A snapshot at the optional path; whether the run ends once it's written.
    Snapshot {
        path: Option<PathBuf>,
        quit: bool,
    },
    Patch(JumpTable),
    /// An edit's operations, its gesture if it sets one field, and whether it changes the playing
    /// copy alone (`play`).
    Apply(Vec<gg_api::Operation>, Option<u64>, bool),
    /// The entity to save as a prefab and link, and the prefab's path (a checked `.prefab`).
    SavePrefab {
        id: Subject,
        path: Option<PathBuf>,
    },
    /// Undoes or redoes the named author's edits, else the client's.
    Undo {
        author: Option<String>,
        redo: bool,
    },
    Edits(Option<String>),
    Quit,
}

/// What `project` finds in the view.
#[derive(Debug, PartialEq)]
pub(super) enum Projecting {
    Entity(Subject),
    Point([f64; 3]),
}

/// What an `input` hands after its first events, and when, as the ticks run.
#[derive(Debug, PartialEq)]
pub(super) struct Hold {
    /// Each wait's ticks with the events after it, in order; then a `hold`'s ticks, with none.
    /// The ticks together are from 1 to [`MOST_STEPS`].
    pub(super) runs: Vec<(u64, Vec<InputEvent>)>,
    /// Whether to let go of what the events leave down once the runs are done: a `hold`'s.
    pub(super) lets_go: bool,
}

/// A release of each key and button `events` press and leave down, and a return to rest of each
/// stick and trigger they leave away from it, in the order pressed: none for a click's.
pub(super) fn releases(events: &[InputEvent]) -> Vec<InputEvent> {
    let mut down = Vec::new();
    hold(&mut down, events);
    down
}

/// Notes in `down`, as [`releases`] gives them, what `events` press or move and leave so, and
/// takes out what they let go of. Returns the presses of a key or button that `down` held
/// already, which change nothing: a second press of an action, say, with no release between.
pub(super) fn hold(down: &mut Vec<InputEvent>, events: &[InputEvent]) -> Vec<InputEvent> {
    let mut already = Vec::new();
    for given in events {
        let (release, pressed) = match *given {
            // A repeat is a held key's own.
            InputEvent::Key { repeat: true, .. } => continue,
            InputEvent::Key {
                ref key, pressed, ..
            } => {
                let key = key.clone();
                let release = InputEvent::Key {
                    key,
                    pressed: false,
                    repeat: false,
                };
                (release, pressed)
            }
            InputEvent::Button { button, pressed } => {
                let release = InputEvent::Button {
                    button,
                    pressed: false,
                };
                (release, pressed)
            }
            InputEvent::PadButton {
                pad,
                button,
                pressed,
            } => {
                let release = InputEvent::PadButton {
                    pad,
                    button,
                    pressed: false,
                };
                (release, pressed)
            }
            InputEvent::PadStick { pad, stick, value } => {
                let rest = InputEvent::PadStick {
                    pad,
                    stick,
                    value: [0.0; 2],
                };
                (rest, value != [0.0; 2])
            }
            InputEvent::PadTrigger {
                pad,
                trigger,
                value,
            } => {
                let rest = InputEvent::PadTrigger {
                    pad,
                    trigger,
                    value: 0.0,
                };
                (rest, value != 0.0)
            }
            _ => continue,
        };
        let moved = matches!(
            given,
            InputEvent::PadStick { .. } | InputEvent::PadTrigger { .. }
        );
        if pressed && down.contains(&release) {
            if !moved {
                already.push(release);
            }
            continue;
        }
        down.retain(|held| *held != release);
        if pressed {
            down.push(release);
        }
    }
    already
}

/// What [`releases`] gives `release` the release of, as `held` and `already_down` name it: a key
/// by its code, as `KeyR`; `mouse left`; `pad 0 south`; `pad 0 left stick`; `pad 0 right trigger`.
pub(super) fn held_name(release: &InputEvent) -> String {
    // As the API spells it: `south`, or `{"other":9}` for a button known by its number alone.
    fn named(value: &impl serde::Serialize) -> String {
        let value = serde_json::to_value(value).unwrap_or_default();
        value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned)
    }
    match release {
        InputEvent::Key { key, .. } => key.clone(),
        InputEvent::Button { button, .. } => format!("mouse {}", named(button)),
        InputEvent::PadButton { pad, button, .. } => format!("pad {pad} {}", named(button)),
        InputEvent::PadStick { pad, stick, .. } => format!("pad {pad} {} stick", named(stick)),
        InputEvent::PadTrigger { pad, trigger, .. } => {
            format!("pad {pad} {} trigger", named(trigger))
        }
        other => format!("{other:?}"),
    }
}

/// The main loop's call for `request`.
///
/// Fails with:
///
/// - `api.unknown_method`.
/// - `api.bad_params`: parameters the method doesn't take; an invalid `look` ([`look`]); an
///   `input` with an unknown key, a stick past 1 or a trigger outside 0 to 1, a `hold` or wait of
///   0, or the two past [`MOST_STEPS`] ticks together; a `press` of 0
///   or past [`MOST_STEPS`] ticks; a `project` of neither or both of an entity and a finite
///   point; an `entities` field without a type; a `scene` path
///   missing, unwanted or not `.ron`, or `discard` given to `save`; a `save_prefab` path not
///   `.prefab`; a `frames` `dir` not an absolute directory path, a `count` of 0 or past
///   [`gg_api::MOST_FRAMES`], or invalid viewpoints or `rect` (no area).
/// - `api.bad_request`: a second `hello`.
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
        "actions" => none(method, params).map(|()| Call::Actions)?,
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
                mut operations,
                gesture,
                play,
            } = take(method, params)?;
            if play && gesture.is_some() {
                let what = "takes no gesture with `play`: a gesture's sets join the client's \
                            last edit, which may be one of the world as edited";
                return Err(bad_params(method, what));
            }
            for operation in &mut operations {
                if let gg_api::Operation::Set {
                    component, path, ..
                }
                | gg_api::Operation::Revert {
                    component, path, ..
                } = operation
                {
                    split_field(component, path);
                }
            }
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
            Call::Apply(operations, gesture, play)
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
                id: Subject::of(&id),
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
        "overlays" => {
            let asked: gg_api::Overlays = take(method, params)?;
            if let Some(problem) = crate::debug_overlays::state::check(&asked) {
                return Err(bad_params(method, problem));
            }
            Call::Overlays(asked)
        }
        "pick" => {
            let gg_api::Pick { at, within } = take(method, params)?;
            Call::Pick(at, within.as_deref().map(Subject::of))
        }
        "cast" => {
            let gg_api::Cast { at, exclude } = take(method, params)?;
            Call::Cast(at, exclude.as_deref().map(Subject::of))
        }
        "project" => match take(method, params)? {
            gg_api::ProjectPoint {
                entity: Some(entity),
                point: None,
            } => Call::Project(Projecting::Entity(Subject::of(&entity))),
            gg_api::ProjectPoint {
                entity: None,
                point: Some(point),
            } if point.iter().all(|x| x.is_finite()) => Call::Project(Projecting::Point(point)),
            _ => {
                let what = "takes an `entity`, or a finite `point`, `[x, y, z]`, one of them";
                return Err(bad_params(method, what));
            }
        },
        "images" => none(method, params).map(|()| Call::Images)?,
        "view" => {
            let View { memory } = take(method, params)?;
            Call::View(memory)
        }
        "input" => {
            let Input { events, hold } = take(method, params)?;
            let refused = events.iter().find_map(|given| match *given {
                InputEvent::Key { ref key, .. } if Key::from_name(key).is_none() => Some(format!(
                    "names keys by their W3C `code`, as `KeyW`, and `{key}` is none"
                )),
                // Past 1 by a rounding's width, as [0.70711, 0.70711], it's taken as 1.
                InputEvent::PadStick { value, .. }
                    if Vec2::from(value).is_nan() || Vec2::from(value).length() > 1.001 =>
                {
                    let [x, y] = value;
                    Some(format!(
                        "moves a stick to `[x, y]` of length at most 1, not [{x}, {y}]"
                    ))
                }
                InputEvent::PadTrigger { value, .. } if !(0.0..=1.0).contains(&value) => {
                    Some(format!("moves a trigger from 0 to 1, not {value}"))
                }
                _ => None,
            });
            if let Some(what) = refused {
                return Err(bad_params(method, what));
            }
            if hold == Some(0) {
                return Err(bad_params(method, "holds at least 1 tick, not 0"));
            }
            // The events up to the first wait go at once; each wait's, after its ticks.
            let mut first = Vec::new();
            let mut runs: Vec<(u64, Vec<InputEvent>)> = Vec::new();
            for given in events {
                match (given, runs.last_mut()) {
                    // A computed wait may come to 0, which waits for nothing.
                    (InputEvent::Wait(0), _) => {}
                    (InputEvent::Wait(ticks), _) => runs.push((ticks, Vec::new())),
                    (given, Some((_, run))) => run.push(given),
                    (given, None) => first.push(given),
                }
            }
            runs.extend(hold.map(|ticks| (ticks, Vec::new())));
            let ticks = (runs.iter()).try_fold(0, |sum: u64, &(ticks, _)| sum.checked_add(ticks));
            if ticks.is_none_or(|ticks| ticks > MOST_STEPS) {
                let what = format!("waits and holds {MOST_STEPS} ticks at most together");
                return Err(bad_params(method, what));
            }
            let hold = (!runs.is_empty()).then_some(Hold {
                runs,
                lets_go: hold.is_some(),
            });
            Call::Input(first, hold)
        }
        "press" => {
            let gg_api::Press { node, ui, ticks } = take(method, params)?;
            if let Some(ticks) = ticks.filter(|ticks| !(1..=MOST_STEPS).contains(ticks)) {
                let what = format!("runs from 1 to {MOST_STEPS} ticks, not {ticks}");
                return Err(bad_params(method, what));
            }
            Call::Press { node, ui, ticks }
        }
        "entities" => {
            let gg_api::Entities {
                with,
                skip,
                take,
                fields,
            } = self::take(method, params)?;
            if take > MOST_LISTED {
                let what = format!("lists at most {MOST_LISTED} entities a page, not {take}");
                return Err(bad_params(method, what));
            }
            let fields = fields.iter().map(|field| {
                let (component, path) = field.split_once('.').unwrap_or((field, ""));
                if component.is_empty() || (path.is_empty() && field.ends_with('.')) {
                    let what = format!(
                        "takes fields of a component's type, then `.` and a path if any, as \
                         `Transform.translation`, not `{field}`"
                    );
                    return Err(bad_params(method, what));
                }
                Ok((component.to_owned(), path.to_owned()))
            });
            let fields = fields.collect::<Result<_, _>>()?;
            Call::Entities {
                with,
                skip,
                take,
                fields,
            }
        }
        "entity" => {
            let gg_api::Entity { id } = take(method, params)?;
            Call::Entity(Subject::of(&id))
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
                mut component,
                mut path,
            } = take(method, params)?;
            split_field(&mut component, &mut path);
            Call::Get {
                id: Subject::of(&id),
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

/// Moves a path given in `component`, as `Transform.translation`, to the front of `path`.
///
/// `entities` takes its fields so: a type's name has no `.`.
fn split_field(component: &mut String, path: &mut String) {
    if let Some((of, rest)) = component.split_once('.') {
        *path = if path.is_empty() {
            rest.to_owned()
        } else {
            format!("{rest}.{path}")
        };
        component.truncate(of.len());
    }
}

/// `params` as a `T`; defaults fill what isn't given.
pub(super) fn take<T: DeserializeOwned>(method: &str, params: &Value) -> Result<T, Failure> {
    let params = match params {
        Value::Null => Value::Object(serde_json::Map::new()),
        given => given.clone(),
    };
    serde_json::from_value(params).map_err(|error| bad_params(method, format!("takes {error}")))
}

/// Checks a method that takes nothing got no parameters, `null` or `{}`.
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
