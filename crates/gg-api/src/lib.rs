//! The tool API's protocol, which a game's development build serves and tools speak, as
//! ARCHITECTURE.md's Agents and tools describes: requests and their answers as lines of JSON over a
//! TCP connection on the machine's loopback; the session file a running game says where it serves
//! in; what each method takes and answers; the memory a game's view is shared through as it
//! renders, [`view`]; and a client, for tools. It also reads a game's project manifest,
//! [`Project`], which tools find a game by and its development build reads, and assets' ID files,
//! [`asset`], which its development build writes beside each texture, model and font. It depends on
//! no crate of the engine's, so that tools build without the GPU's toolchain.
//!
//! A request is `{"id":1,"method":"step","params":{"ticks":2}}`, without `params` for a method that
//! takes none, and its answer `{"id":1,"result":...}` or `{"id":1,"error":{"code":...,"message":...}}`,
//! each a line. A connection's first request is `hello`, with the session's token. The methods:
//!
//! - `hello`, [`Hello`]: answers [`Welcome`]; names the client, whose edits the connection's
//!   `apply`, `undo` and `redo` are.
//! - `ping`: answers [`Pulse`] at once, even while the game's main thread is busy.
//! - `status`, `pause`, `resume`, `play` and `stop`: answer [`Status`].
//!
//!   `stop` stops the game: frames run no ticks, and its world is the world as edited, which only
//!   edits, scene files' saves and `scene`'s `open` and `new` change, until `play`. While a copy
//!   plays, `stop` brings back the world as edited as it was when the copy started: at its ticks;
//!   with its scene files as last applied, to which a save made since applies; and with the
//!   authors' edits, which `undo` steps back through again. What the copy did goes with it, edits
//!   and entities. The IDs it drew are drawn again, so a client keeps no ID learned while it
//!   played across a `stop`.
//!
//!   A game playing from its start, which no `play` started, holds no world as edited: `stop` stops
//!   it where it is, and that world is the world as edited from then on.
//!
//!   `stop` fails the `step`s waiting with `api.stopped`. It fails itself with `api.no_stop` in a
//!   recorded run or a lockstep peer, which play from start to end.
//!
//!   `play` has a copy of the world as edited play in its place, from its ticks, until `stop`.
//!
//!   Neither does anything in a state the game is already in.
//! - `step`, [`Step`]: runs ticks, pausing the simulation, and answers [`Status`] once they've run;
//!   fails with `api.stopped` while the game is stopped: `play` first.
//! - `scene`, [`Scene`]: opens, saves or empties the open scene, the project's scene file the game
//!   started in or one opened since, and answers [`Status`], which names it.
//!
//!   The open scene owns the entities its file held as last applied and those clients' edits have
//!   made since it opened, their creates and duplicates, never those the game's code made. `open`
//!   puts a scene file's entities in place of the open scene's, the game watching the file from
//!   then on. `new` deletes the open scene's entities and leaves it untitled. `save` writes them to
//!   its file, or to `path`, which becomes its file, and the game's watch applies nothing for it.
//!
//!   Each acts on the world as edited: `save` writes it while a copy plays, and `open` and `new`
//!   stop the copy first. Neither `open` nor `new` can be undone: every author's edits start
//!   afresh, as after a restore.
//!
//!   A path is absolute, or relative to the asset root, as [`Status::scene`] names files; it is in
//!   one of the project's scene folders and ends in `.ron`.
//!
//!   It fails with:
//!   - `engine.no_project`, in a game that isn't a project's;
//!   - `scene.bad_path`, for a path outside the scene folders;
//!   - `scene.unsaved`, for `open` or `new` while the open scene has unsaved edits, unless
//!     `discard`;
//!   - `scene.untitled`, for `save` of an untitled scene without a path;
//!   - `scene.watched`, for a path another scene file the game loaded has;
//!   - `scene.changed`, for `save` while the file differs from what the game last applied, so that
//!     an edit to it isn't lost;
//!   - `api.no_scene`, in a lockstep peer, and for `open` and `new` in a recorded run;
//!   - and as loading or saving a scene fails, such as with `scene.file` or `scene.outside`.
//! - `capture`, [`Capture`]: writes the game's view as a PNG once a frame renders it, as it's
//!   shown, the look among it, or as the game shows it on its own, and answers [`Captured`].
//! - `view`, [`View`]: has the game publish each frame's view, from the next, into the memory a
//!   client made, as [`view::Reader`] makes it, in place of any it published to before, or stop
//!   publishing for `null`, and answers `{}`; fails with `api.view` if the memory can't be opened.
//!   A game run `--embedded` renders its view at the size the memory asks, and runs a frame once
//!   the client has taken the last; another publishes its view at its own size, while it fits.
//! - `input`, [`Input`]: hands the game's layers events, in order, as its window would, as the
//!   frame starts, and answers `{}`.
//! - `look`, [`Look`]: sets parts of the game's look, its view settings, which any client sets, the
//!   last to set a part winning, and which last until a client changes them or the run ends: the
//!   viewpoint the view is seen from, the outline round an entity, a grid, and whether colliders'
//!   outlines show; and answers [`Looked`], the look once set, or as it is, for `{}` or no params.
//!   It's set as a frame starts, which draws with it. It fails with `api.bad_params`, saying why,
//!   for a viewpoint that isn't finite, whose rotation doesn't normalize, or whose projection can't
//!   project, as [`Projection`] says; or for an outline whose ID isn't 16 hex digits, whose color
//!   isn't finite, or whose width isn't finite and positive. An outline of an ID no entity has
//!   outlines nothing, and isn't refused.
//! - `pick`, [`Pick`]: answers [`Picked`], the entity the view shows at a pixel, as it's shown, the
//!   look among it, in the frame that reads it, about four frames after it's asked.
//! - `entities`, [`Entities`]: answers [`EntityList`], a page of the world's entities, with the
//!   revision of its types, as `types` answers it.
//! - `entity`, [`Entity`]: answers the entity: `{"id":...,"name":...,"parent":...,"children":[...],
//!   "components":{"Transform":{...},...},"space":{"matrix":[...],"rotation":[...]}}`, its
//!   components in the order their types registered; and `space`, where the space its `Transform`
//!   is in sits in the world, as the last tick left it: its parent's pose, as a matrix of 16 floats,
//!   column by column, and a rotation, `[x, y, z, w]`, without the parent's scales; the identity
//!   for a root. Its floats are written as a value's are.
//! - `get`, [`Field`]: answers a component's field, as a value.
//! - `types`: answers [`Types`], the world's component types and the descriptions of the types
//!   they hold, which values are read and written by: a client draws and checks values by them, as
//!   the editor's Inspector does; and their revision. A type's description changes as a script
//!   that declares it reloads with other fields, and as a game rebuilt describes its types
//!   otherwise; the revision changes with each change to the types, so that a client reads them
//!   again as the revision `entities` answers moves on.
//! - `hash`: answers [`struct@Hash`], the world's.
//! - `assets`, [`Assets`]: answers [`AssetList`], a page of the textures, models and fonts under
//!   the game's asset root, in the order of their paths, each with its ID, which its ID file beside
//!   it, [`asset::Meta`], holds, as ARCHITECTURE.md's Project state describes, and the ID files
//!   whose assets aren't there. It answers once the game has looked at the root since it asked, or at
//!   once from its last look while its frames pause, as while its window is minimized, so that
//!   an asset added or renamed since is listed, a run that writes ID files, a game alone or a
//!   lockstep session's host that serves the API, having written each one missing and moved beside
//!   its file each that a rename of the file alone left behind, as it does as it starts and as it
//!   watches the root. A listed asset's `problem` says what's wrong with its ID file:
//!   - `asset.no_id`, none, in a run that writes none, a lockstep session's joiner: its host writes
//!     it, as a game alone does;
//!   - `asset.bad_meta`, one that doesn't read: fix it, or remove it for a fresh one;
//!   - `asset.meta_case`, one whose name differs from `<file>.meta` in case alone: rename it;
//!   - `asset.duplicate_id`, one holding the ID another asset has this run, as a copy's does, or
//!     none while another ID file holds its ID. From the next run, the
//!     first asset by path whose ID file holds an ID has it, as the message says, and another
//!     whose file holds it goes without until that file is removed, after which a run writes it a
//!     fresh one, as it does for one without an ID file: remove a copy's ID file, rather than the
//!     original's;
//!   - `asset.id_changed`, one changed since the run read it: the run keeps the ID it had until it
//!     ends;
//!   - `asset.unwritten`, one the run couldn't write, saying why.
//!
//!   An orphan, an ID file left behind as by a rename the game couldn't pair with its file, as one
//!   made with an edit, goes beside its asset, named for it, or is deleted. It fails with
//!   `engine.no_root` in a game that names no asset root, and with `asset.unlisted` if the root
//!   can't be listed.
//! - `snapshot`, [`Snapshot`]: saves the world whole as a file, which a run given `--restore`
//!   starts from, and answers [`Snapshotted`] once it's written: a stopped game's file records that
//!   it's stopped, and one taken while a copy plays saves the world as edited too, so that a run
//!   restored from it goes on as it was. Before the first tick of a game playing from its start,
//!   which applies what the layers queued as they started, it fails with `api.not_ticked`.
//! - `patch`, [`Patch`]: applies a patch of the game's code, which `dx tools hotpatch` built for the
//!   game's process, between ticks, and answers [`Patched`]; fails with `api.bad_params` if the
//!   table's library, `lib`, isn't an absolute path, and with `api.patch` if the build can't take
//!   patches, as [`Session::aslr_reference`] says, the table's reference is past where the game's
//!   `main` is, so that it isn't for this process, the library doesn't load, or it isn't a patch of
//!   the game's build, as one that exports no `main` isn't.
//! - `apply`, [`Apply`]: applies an edit, [`Operation`]s that undo and redo together, as the
//!   frame starts, between ticks, and answers [`Applied`]; it applies all or none, failing with
//!   the code of the first refused, such as `ecs.no_entity`, which names it. A set of a gesture,
//!   such as a drag, joins the client's last edit if that's a set of the same field in the same
//!   gesture, so that the gesture undoes whole.
//! - `undo` and `redo`, [`Undo`]: undo the last edit of an author, the client's own by default,
//!   such as a scene file's, which its path names, or redo the last it undid, and answer
//!   [`Edit`], or `null` if there's none; an edit the world refuses to undo or redo, as when an
//!   entity it changed is gone since, fails, and is dropped.
//! - `edits`, [`Edits`]: answers [`History`], an author's edits, or each author's.
//! - `logs`, [`Logs`]: answers [`LogEvents`], events of the game's JSON log, each its line as the
//!   log wrote it, of the last the game keeps, at most 10,000 and 8 MiB of lines: from an earlier
//!   answer's `next`, or else the last of them, or all, from the oldest; those at a level and more
//!   severe; waiting for one to come, if none is there. A `from` given with another run's `run`, as
//!   a client following the game's logs across a restart gives, reads from the oldest kept, so that
//!   the client misses nothing it can still have.
//! - `note`, [`Note`]: has the game log an event the client gives, naming the client, and answers
//!   `{}`, so that those following its logs learn what the client knows of it, as `gg run` tells of
//!   a patch or a rebuild of it that failed. It fails with `api.bad_params` for a code that isn't a
//!   stable code's shape, a dotted name such as `gg.patch_failed`, or is longer than
//!   [`MOST_NOTE_CODE`] bytes, for an empty message, and for a message or fields longer than
//!   [`MOST_NOTE`] bytes.
//! - `quit`: answers `{}`, and the run ends cleanly, as when its window closes, but without asking
//!   its layers, which may keep a window open.
//!
//! The methods but `hello`, `ping`, `logs` and `note` are answered by the game's main thread, as a
//! frame starts. While the window is minimized, and no frames run, it answers them between frames,
//! but `step`, `capture` and `pick`, which need frames, fail with `api.paused`; while a file dialog
//! or the window menu Windows runs is open, holding the main thread, they wait. The connection's
//! own thread answers `hello`, `ping`, `logs` and `note` as it reads them, so that a `logs` that
//! waits holds up the connection's later requests: follow the logs on a connection of their own.
//!
//! A value is JSON as its kind is: a number, a string, a `bool`; a vector or a quaternion an array
//! of numbers, `[x, y, z, w]` for a quaternion; a float that isn't finite the string `inf`, `-inf`
//! or `NaN`; an entity its ID, a string of 16 hex digits, or `null` if it isn't alive; a texture
//! its name, its asset ID where its file has one, such as `5f3c9a1e07b2d846`, a model's image the
//! model's and its place, such as `5f3c9a1e07b2d846#0`, or else its path, such as
//! `assets/logo.png`, or `null` for one without; a model or a font its name, as a texture's; an
//! option `null` or its value; a list an array; a struct an object of its fields, in their order, a
//! tuple struct's named `0`, `1` and so on; an enum's variant its name, as a string, or for a
//! variant with fields an object of one key, its name, whose value is an object of the fields; and
//! what reflection doesn't read, such as a `MeshDraw`, the string `(opaque)`. A value given, as an
//! operation's, is read the same way, by the type of the field it goes in, as a component's
//! description gives it: an integer where a float goes, a texture, a model or a font by its name or
//! a path to its file, which the game loads from the file under its asset root if it hasn't, its
//! model's image with its model, an entity by the ID of one that's alive, or `null` for none
//! alive, and a struct, or a variant's fields, missing a field, which takes its default, as in a
//! scene; what's opaque is kept as it is, or made as the type's own is, as an `(opaque)` that a
//! create or an add gives says.

pub mod asset;
mod project;
// It maps memory another process shares, which only unsafe code reaches.
#[allow(unsafe_code)]
pub mod view;

use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

pub use project::{Project, UNBUILT};

/// The most bytes a line may have, its line break aside, past which the side reading it drops the
/// connection: a request's or an answer's, such as a page of entities.
pub const MAX_LINE: usize = 16 << 20;

/// Where a running game serves the API, which its development build writes as a file of JSON in
/// the `sessions` directory beside its executable as the API starts, and removes as the run ends;
/// one that a crash or a kill leaves behind names an address that refuses connections. Only the
/// user the game runs as can read the file: on Unix its mode is 0600, and on Windows its DACL
/// allows its owner alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// The app's name, as `App::new` gives it, such as `Sandbox`.
    pub app: String,
    /// Its process's ID.
    pub pid: u32,
    /// Where it listens, on the loopback.
    pub address: SocketAddr,
    /// What a client's `hello` gives, so that only processes that can read the file, the user's
    /// own, talk to it.
    pub token: String,
    /// The run's JSON log, unless it couldn't be created.
    pub log: Option<PathBuf>,
    /// The engine's version and build, such as `0.1.0 debug`.
    pub engine: String,
    /// Where the game's `main` is in its process, which `dx tools hotpatch` builds a patch for the
    /// process against (`--aslr-reference`), as the OS loads the executable somewhere else in each;
    /// 0 in a build that can't take patches: one whose executable exports no `main`, as one that
    /// cargo built for Windows or Linux doesn't, or one without debug assertions, without which
    /// subsecond never looks a patch up. A debug build that cargo made for macOS exports its `main`
    /// all the same, though only patches of the build `dx build --fat-binary` made suit a process.
    pub aslr_reference: u64,
    /// The player it plays in a lockstep session, 0 for the host and 1 for the joiner; `None` for a
    /// game alone, or one whose engine is older than lockstep.
    #[serde(default)]
    pub player: Option<u8>,
    /// The path of the game's project manifest, as the game named it, which tools inside the
    /// project reach it by; `None` for an app that isn't a project, or one whose engine is older
    /// than projects.
    #[serde(default)]
    pub project: Option<PathBuf>,
}

impl Session {
    /// The directory of the session file of a game whose executable is in `exe_dir`.
    pub fn dir(exe_dir: &Path) -> PathBuf {
        exe_dir.join("sessions")
    }

    /// The name of the session file of app `app` in process `pid`, such as `sandbox-1234.json`: the
    /// app's name in lower case, each character that isn't an ASCII letter or digit a `-`, as its
    /// log's is.
    pub fn file_name(app: &str, pid: u32) -> String {
        let app: String = app
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        format!("{app}-{pid}.json")
    }

    /// The sessions whose files are in `dir`, each with its file's path, in the order of their
    /// names; none if `dir` can't be read. A file that isn't a session's is skipped, as a session
    /// file being written is, beside its own.
    pub fn list(dir: &Path) -> Vec<(PathBuf, Self)> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut sessions: Vec<_> = entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path.extension()? != "json" {
                    return None;
                }
                let session = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
                Some((path, session))
            })
            .collect();
        sessions.sort_by(|(a, _), (b, _)| a.cmp(b));
        sessions
    }
}

/// A request, a line of JSON; its answer has its `id`.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    /// `null`, and left out, for a method that takes nothing.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

/// An answer to a request, a line of JSON: its result, or the error it failed with.
#[derive(Debug, Deserialize)]
pub struct Answer {
    pub id: u64,
    /// The result's JSON, as the game wrote it: `null` too, as an `undo` with nothing to undo
    /// answers, which is a result.
    #[serde(default, deserialize_with = "present")]
    pub result: Option<Box<RawValue>>,
    #[serde(default)]
    pub error: Option<Failure>,
}

/// A field that's there, whatever its JSON, `null` too, which an `Option` would take for none.
fn present<'de, D, T>(json: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(json).map(Some)
}

/// The line answering request `id`: with `result`, its JSON, or with the error it failed with.
pub fn answer_line(id: u64, answer: Result<&str, &Failure>) -> String {
    match answer {
        Ok(result) => format!("{{\"id\":{id},\"result\":{result}}}\n"),
        Err(failure) => {
            let failure = serde_json::to_string(failure).expect("strings always serialize");
            format!("{{\"id\":{id},\"error\":{failure}}}\n")
        }
    }
}

/// Reads a line, without its line break, `\n` or `\r\n`: `None` once the stream ends, or ends
/// partway through a line, as a connection cut short does.
///
/// Fails with `InvalidData` for a line of more than [`MAX_LINE`] bytes, or one that isn't UTF-8.
pub fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    let limit = MAX_LINE as u64 + 1;
    reader.by_ref().take(limit).read_until(b'\n', &mut line)?;
    if line.pop() != Some(b'\n') {
        if line.len() >= MAX_LINE {
            let message = format!("a line of more than {MAX_LINE} bytes");
            return Err(io::Error::new(io::ErrorKind::InvalidData, message));
        }
        return Ok(None);
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// An error as the API gives it: a stable code, a dotted name such as `ecs.no_entity`, and a
/// message that says what happened, naming what it's about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

impl Failure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

/// What `hello` takes: the session's token. Any other first request, or another token, is refused
/// with `api.unauthorized`, and the connection closes.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub token: String,
    /// The client, the author of the edits the connection applies, which the connection's `undo`
    /// and `redo` step through: `api` without one. Connections with the same name share them, so
    /// that a client undoes its own edits across connections, as `gg`'s commands do, each one's
    /// own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
}

/// What `hello` answers: the game the connection reaches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub app: String,
    pub pid: u32,
    /// The engine's version and build, such as `0.1.0 debug`.
    pub engine: String,
    /// The run's JSON log, unless it couldn't be created.
    pub log: Option<PathBuf>,
}

/// What `ping` answers: the frames the game has run, which a hung game's main thread stops
/// counting, and what frames have paused for, if they've paused for the user: `minimized`, while
/// the main window has no area, `dialog`, while a file dialog is open, or `menu`, while the window
/// menu Windows runs is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pulse {
    pub frames: u64,
    pub waiting: Option<String>,
}

/// What `status`, `pause`, `resume`, `play`, `stop` and `step` answer: the game as the frame that
/// answers starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// The ticks the simulation has run, which number the next: after a `stop` that brings back the
    /// world as edited, that world's, so that they may go back.
    pub ticks: u64,
    /// Whether the simulation is paused, as `pause` and `step` leave it, which lasts across `stop`
    /// and `play`.
    pub paused: bool,
    /// Whether the game is stopped, as `stop` and `--stopped` leave it: its world is the world as
    /// edited, which no tick changes, until `play`. False from an engine older than `stop`.
    #[serde(default)]
    pub stopped: bool,
    /// The frames the game has run.
    pub frames: u64,
    /// The world's living entities.
    pub entities: usize,
    /// The game's view's width and height in pixels, as the last frame handed it to the renderer;
    /// `None` while it had no area.
    pub view: Option<[u32; 2]>,
    /// The open scene's file, as the asset root names it, such as `scenes/arena.ron`: the scene
    /// that `scene` acts on, of the world as edited. `None` while it's untitled, as after `new`; in
    /// a game that isn't a project's; and from an engine older than `scene`.
    #[serde(default)]
    pub scene: Option<String>,
    /// Whether the open scene, in the world as edited, differs from its file, as `save` would
    /// write it, or, untitled, holds entities. False from an engine older than `scene`.
    #[serde(default)]
    pub unsaved: bool,
}

/// What `scene` takes: what to do to the open scene; the file to open, or to save it as, absolute
/// or relative to the asset root; and, for `open` and `new`, whether to discard its unsaved edits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub action: SceneAction,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub discard: bool,
}

/// What `scene` does to the open scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SceneAction {
    Open,
    Save,
    New,
}

/// What `step` takes: how many ticks to run, 1 by default and at most 10,000, a frame's worth
/// each frame. The simulation stays paused after them; `resume` runs it again, and drops the
/// steps not yet run, answering those waiting; `stop` drops the steps not yet run too, failing
/// those waiting with `api.stopped`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default = "one")]
    pub ticks: u64,
}

fn one() -> u64 {
    1
}

/// What `capture` takes: where to write the PNG, an absolute path, whose directory exists; without
/// one, a file in the `captures` directory beside the game's executable.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Whether to capture the view as the game shows it on its own, through its own viewpoint,
    /// without the look's outline, grid or colliders, in a frame that isn't published to a view's
    /// memory, and that a windowed game shows in its window; without it, the capture shows the
    /// view as it's shown, the look among it.
    #[serde(default)]
    pub own: bool,
}

/// What `capture` answers: where the PNG is, its size, the view's in pixels, and the ticks the
/// simulation had run in the frame it shows, as that frame drew them: while paused, as the last
/// tick left the world.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Captured {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub ticks: u64,
}

/// What `view` takes: the name of the memory to publish the view into, as [`view::Reader::name`]
/// gives it, or `None` to stop publishing.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    #[serde(default)]
    pub memory: Option<String>,
}

/// What `input` takes: the events to hand the game's layers, in order.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub events: Vec<InputEvent>,
}

/// An event `input` hands the game's layers, as its window would: `{"key":{"key":"KeyW",
/// "pressed":true}}`, a key naming it, whose value is what it takes; `"cursor_left"` for one that
/// takes nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum InputEvent {
    /// A key went down or up, by its position, as its W3C UI Events `code` names it, such as
    /// `KeyW`, or winit, which names `MetaLeft` and `MetaRight` `SuperLeft` and `SuperRight`;
    /// `repeat` marks the presses a held key repeats.
    Key {
        key: String,
        pressed: bool,
        #[serde(default)]
        repeat: bool,
    },
    /// Text a key's press typed, after it.
    Text(String),
    /// The cursor moved over the view, to `[x, y]` pixels from its top left.
    Cursor([f64; 2]),
    /// The cursor came over the view.
    CursorEntered,
    /// The cursor left the view.
    CursorLeft,
    /// A mouse button went down or up.
    Button { button: Button, pressed: bool },
    /// The wheel or a touchpad scrolled: positive `y` up, and positive `x` left.
    Scroll(Scroll),
    /// The view gained or lost the keys.
    Focused(bool),
}

/// A mouse button: `"left"`, `"right"`, `"middle"`, the side buttons `"back"` and `"forward"`,
/// or another by its number, `{"other":9}`, which differs from one platform to another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// How far a scroll went: `{"lines":[x, y]}`, as a wheel's notches go, or `{"pixels":[x, y]}`, as
/// a touchpad's do.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scroll {
    Lines([f32; 2]),
    Pixels([f64; 2]),
}

/// Where the game's view is seen from, and how: from `position`, facing as `rotation`, a unit
/// quaternion, turns it, through `projection`. World space is right-handed with +Y up, and a
/// viewpoint without rotation looks down -Z, with +X to its right and +Y up.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Viewpoint {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub projection: Projection,
}

impl Viewpoint {
    /// Whether each of its numbers is finite.
    pub fn is_finite(&self) -> bool {
        let projection = match self.projection {
            Projection::Orthographic { height, near, far } => [height, near, far],
            Projection::Perspective { fov, near } => [fov, near, 0.0],
        };
        (self.position.iter())
            .chain(&self.rotation)
            .chain(&projection)
            .all(|number| number.is_finite())
    }
}

/// How a viewpoint projects what's in front of it onto the view, fitted to the view's aspect
/// ratio: `{"Orthographic":{"height":2.0,"near":-1.0,"far":1.0}}` or
/// `{"Perspective":{"fov":1.0472,"near":0.01}}`, as `entity` gives a camera's `projection`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Projection {
    /// Without perspective, as 2D games draw: the view spans `height` world units from bottom to
    /// top, and shows what's from `near` to `far` in front of the viewpoint. It projects if
    /// `height` is a positive normal number, and `near` and `far` are finite and differ.
    Orthographic { height: f32, near: f32, far: f32 },
    /// With perspective, as 3D games draw: the view spans `fov` radians from bottom to top, and
    /// shows what's at least `near` in front of the viewpoint, however far. It projects if `fov` is
    /// more than 0 and less than π, and `near` is a positive normal number.
    Perspective { fov: f32, near: f32 },
}

/// An outline round what the view shows of an entity, by its ID, 16 hex digits: its color, in
/// linear RGBA, alpha not multiplied in; and its width, how far out it reaches from the entity's
/// pixels, in the view's pixels, a width past 8 drawing 8.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outline {
    pub id: String,
    pub color: [f32; 4],
    pub width: f32,
}

/// One of the world's axis planes, by the two axes it spans: `"xy"`, the plane 2D games are in;
/// `"xz"`, the ground; or `"yz"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Plane {
    Xy,
    Xz,
    Yz,
}

/// What `look` takes: the parts of the game's look to set, each left out to keep it as it is,
/// `null` to give it back to the game, or its value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Look {
    /// The viewpoint the view is seen from, in place of the game's own; its rotation is
    /// normalized.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub viewpoint: Option<Option<Viewpoint>>,
    /// The outline round an entity, in place of any the game's layers draw.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub outline: Option<Option<Outline>>,
    /// The plane a grid is drawn in, fitted to the view, as the editor draws one.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub grid: Option<Option<Plane>>,
    /// Whether the colliders' outlines show, in place of whether the game shows them.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub colliders: Option<Option<bool>>,
}

/// What `look` answers: the look once set, each part `None` where it's the game's own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Looked {
    pub viewpoint: Option<Viewpoint>,
    pub outline: Option<Outline>,
    pub grid: Option<Plane>,
    pub colliders: Option<bool>,
    pub by: By,
    /// How many times the look has changed in the run, which a frame shared gives as the revision
    /// it drew with ([`view::Frame::look`]): a `look` that changes no part's value or setter
    /// doesn't count.
    pub revision: u64,
    /// The run's number, as `logs` gives it, so that a client tells a restarted game, whose look
    /// starts afresh, from a look unchanged.
    pub run: u64,
    /// The viewpoint the last frame handed to the renderer drew from, the look's or the game's own;
    /// `None` before the first, and while the viewpoint it drew from has a number that isn't
    /// finite, which JSON can't carry. A `look` that sets a viewpoint answers before a frame draws
    /// with it.
    pub seen: Option<Viewpoint>,
}

/// Who set each part of the look: the client, by the name it said hello with, or `None` where the
/// part is the game's own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct By {
    pub viewpoint: Option<String>,
    pub outline: Option<String>,
    pub grid: Option<String>,
    pub colliders: Option<String>,
}

/// What `pick` takes: the pixel of the view to pick at, `[x, y]` from its top left, as `input`'s
/// cursor is, and as a capture's PNG, the view, has it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pick {
    pub at: [f64; 2],
}

/// What `pick` answers: the entity the view shows at the pixel, by its ID, and its name, as it's
/// answered, or for a model's node the model's root, which tools change in its place; both `None`
/// outside the view or where no entity draws, and the name for an entity gone by the answer, whose
/// ID is kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picked {
    pub id: Option<String>,
    pub name: Option<String>,
}

/// What `entities` takes: those with component `with`, or all, in the order queries find them,
/// `skip` of them skipped and at most `take`, 100 by default.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entities {
    #[serde(default)]
    pub with: Option<String>,
    #[serde(default)]
    pub skip: usize,
    #[serde(default = "page")]
    pub take: usize,
}

fn page() -> usize {
    100
}

/// What `entities` answers: how many there are in all, the page asked for, and the revision of the
/// world's types, as `types` answers it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityList {
    pub total: usize,
    pub entities: Vec<Listed>,
    /// 0 from an engine older than revisions of the types.
    #[serde(default)]
    pub types: u64,
}

/// An entity as `entities` lists it: its ID, its name, its parent's ID, its children's, in their
/// order, and its components' types, in the order they registered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    pub id: String,
    pub name: Option<String>,
    pub parent: Option<String>,
    /// Empty from an engine older than lists of them.
    #[serde(default)]
    pub children: Vec<String>,
    pub components: Vec<String>,
}

/// What `entity` takes: an entity's ID.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entity {
    pub id: String,
}

/// What `get` takes: an entity's ID, one of its components' types, and the path of a field of it,
/// such as `translation.x` or `rect[1]`, empty for the whole component.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub id: String,
    pub component: String,
    #[serde(default)]
    pub path: String,
}

/// What `types` answers: the world's component types, in the order they registered, and the named
/// types, structs and enums, that they are and hold, in the order they were described, each under
/// a name no other has; and their revision, a number that names them as they are, which each
/// change to them changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Types {
    pub components: Vec<ComponentInfo>,
    pub types: Vec<TypeInfo>,
    /// 0 from an engine older than revisions of the types.
    #[serde(default)]
    pub revision: u64,
}

/// A component type, as `types` gives it: its name, which its description has too, and the value an
/// `add` without one gives it, or why there's none, as when it holds an entity, which has no
/// default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComponentInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// A named type as reflection describes it: `{"name":"Spin","doc":...,"shape":{"struct":
/// {"fields":[...],"tuple":false}}}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeInfo {
    pub name: String,
    /// Its doc comment, its lines joined into paragraphs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub doc: String,
    pub shape: Shape,
}

/// What a named type holds: a struct's fields, a tuple struct's named `0`, `1` and so on; an enum's
/// variants; or, for `"opaque"`, nothing values give or take but `(opaque)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Struct { fields: Vec<FieldInfo>, tuple: bool },
    Enum(Vec<VariantInfo>),
    Opaque,
}

/// A field of a struct or of a variant: its name, which paths name it by; its type; the least and
/// the most its numbers, its vectors' components and its elements may be; how the Inspector shows
/// it; its doc comment; and its own default, if its description gives it one, which may be `null`,
/// as an option's: without one, its type's is, as gg-engine's `Types::field_default` makes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: Type,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<Bound>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<Bound>,
    #[serde(default, skip_serializing_if = "Show::is_plain")]
    pub show: Show,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub doc: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub default: Option<Value>,
}

/// An enum's variant: its name and its fields, a tuple's if `tuple`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VariantInfo {
    pub name: String,
    pub fields: Vec<FieldInfo>,
    pub tuple: bool,
}

/// A field's type: `"bool"`, `{"int":{"bits":32,"signed":true}}`, `"f32"`, `"f64"`, `"string"`,
/// `"vec2"`, `"vec3"`, `"vec4"`, `"quat"`, `"entity"`, `"texture"`, `"model"`, `"font"`,
/// `{"option":...}`, `{"list":...}`, `{"array":[..., 4]}`, `{"named":"Transform"}`, a type `types`
/// describes, or `"opaque"`, a field reflection leaves as it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Type {
    Bool,
    Int { bits: u8, signed: bool },
    F32,
    F64,
    String,
    Vec2,
    Vec3,
    Vec4,
    Quat,
    Entity,
    Texture,
    Model,
    Font,
    Option(Box<Type>),
    List(Box<Type>),
    Array(Box<Type>, u32),
    Named(String),
    Opaque,
}

/// A field's `min` or `max`, as written: `{"int":-3}` or `{"float":0.5}`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bound {
    Int(i128),
    Float(f64),
}

/// How the Inspector shows a field: `"plain"`; `"degrees"`, an `f32` of radians in degrees; or
/// `"color"`, a `vec3` or `vec4` of linear color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Show {
    #[default]
    Plain,
    Degrees,
    Color,
}

impl Show {
    fn is_plain(&self) -> bool {
        *self == Self::Plain
    }
}

/// What `hash` answers: the world's hash, its content's and its layout's, each 16 hex digits, as
/// recordings write them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hash {
    pub content: String,
    pub layout: String,
}

/// What `assets` takes: the assets of kind `kind`, or all, in the order of their paths, `skip` of
/// them skipped and at most `take`, 100 by default.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assets {
    #[serde(default)]
    pub kind: Option<asset::AssetKind>,
    #[serde(default)]
    pub skip: usize,
    #[serde(default = "page")]
    pub take: usize,
}

/// What `assets` answers: how many there are in all, of the kind asked for; the page asked for; and
/// every ID file of that kind whose asset isn't there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetList {
    pub total: usize,
    pub assets: Vec<ListedAsset>,
    pub orphans: Vec<ListedOrphan>,
}

/// An asset as `assets` lists it: its ID, 16 hex digits, or `None` while it has none; its path from
/// the asset root, with `/` between its parts, as [`Status::scene`] names files, spelled as on disk;
/// its kind; whether a layer has added it, by whatever path; and what's wrong with its ID file, if
/// anything, as the `assets` method's docs list: why it has no ID, or why its ID isn't the one its
/// file holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedAsset {
    pub id: Option<String>,
    pub path: String,
    pub kind: asset::AssetKind,
    pub loaded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<Failure>,
}

/// An ID file whose asset isn't there, as after a rename that left it behind: its own path from the
/// asset root, the kind of the asset it names, and the ID it holds, or `None` if it doesn't read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedOrphan {
    pub path: String,
    pub kind: asset::AssetKind,
    pub id: Option<String>,
}

/// What `snapshot` takes: where to write the snapshot, an absolute path, whose directory exists;
/// without one, a file in the `snapshots` directory beside the game's executable.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    #[serde(default)]
    pub path: Option<PathBuf>,
}

/// What `snapshot` answers: where the snapshot is, the ticks the simulation had run as it was taken,
/// which a run restored from it goes on from, its entities, and its size in bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshotted {
    pub path: PathBuf,
    pub ticks: u64,
    pub entities: usize,
    pub bytes: u64,
}

/// What `patch` takes: the jump table that `dx tools hotpatch` printed, as it printed it, which
/// names the patch's library and maps each function the patch replaces to its new address.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub table: Value,
}

/// What `patch` answers: the ticks the simulation had run as the patch applied, the first that runs
/// the patched code being the next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patched {
    pub ticks: u64,
}

/// An authoring change to the world, as `apply` takes it: `{"set":{"id":...,"component":...,
/// "path":...,"value":...}}`, a key naming it, whose value is an object of what it takes. It names
/// entities by their IDs, and carries values as gg-api's docs say.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Spawns an entity with `components`, each by its type's name, its value its whole value: the
    /// last of `parent`'s children, or a root. Its ID is `id`, or one drawn without one, which
    /// [`Applied`] gives.
    Create {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        components: serde_json::Map<String, Value>,
    },
    /// Spawns a copy of entity `id` and of its descendants, the copy the last of its parent's
    /// children, each with its components as they are, each reference to one of them to its copy,
    /// and each exclusive field false, as a camera's `primary`: their IDs `copies`, `id`'s copy's
    /// first, then its descendants', each parent's before its children's, or ones drawn without
    /// them, which [`Applied`] gives.
    Duplicate {
        id: String,
        #[serde(default)]
        copies: Vec<String>,
    },
    /// Despawns entity `id` and its descendants.
    Delete { id: String },
    /// Gives entity `id` a `component`, which it hasn't: `value`, or the type's default without
    /// one.
    Add {
        id: String,
        component: String,
        #[serde(default)]
        value: Option<Value>,
    },
    /// Takes entity `id`'s `component` away.
    Remove { id: String, component: String },
    /// Sets the field at `path` of entity `id`'s `component`, such as `translation.x`, to `value`;
    /// the whole component for an empty path. A component's exclusive field set true is made false
    /// on every other entity with the component.
    Set {
        id: String,
        component: String,
        #[serde(default)]
        path: String,
        value: Value,
    },
    /// Makes entity `id` the last of `parent`'s children, or a root for `null`.
    Reparent {
        id: String,
        /// Given, `null` for a root, so that one left out isn't taken for a root.
        #[serde(deserialize_with = "Option::deserialize")]
        parent: Option<String>,
    },
}

/// What `apply` takes: an edit's operations, applied in order; and, for a set that's part of a
/// gesture, such as a drag or a number typed, the gesture, a number the client gives each of its
/// own, the same for each of its sets: a set of the field that the client's last edit set, with the
/// same gesture, joins that edit, taking its value and keeping what undoes it, so that the gesture
/// undoes whole, and any other edit of the client's that reaches the world ends it, refused there
/// or not; one refused as it's read, as for an ID that isn't one, a type there's none of or a value
/// of another kind, doesn't. Since a client's edits are those of every connection with its name, a
/// gesture's number is its own across them, as one drawn at random to start from is. With a
/// gesture, an apply is of one set, or refused with `api.bad_params`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Apply {
    pub operations: Vec<Operation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gesture: Option<u64>,
}

/// What `apply` answers: the ticks the simulation had run as the edit applied, the next being the
/// first after it, and the IDs of the entities its creates and duplicates spawned, in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    pub ticks: u64,
    pub entities: Vec<String>,
}

/// What `undo` and `redo` take: whose last edit to undo, or redo, by its author's name: the
/// client's own without one.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Undo {
    #[serde(default)]
    pub author: Option<String>,
}

/// An edit, as `undo`, `redo` and `edits` give it: its author and its operations in words, such as
/// ``set `Spin.speed` of 2b7e151628aed2a6``.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub author: String,
    pub operations: Vec<String>,
}

/// What `edits` takes: whose edits to list, by its author's name; every author's without one.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edits {
    #[serde(default)]
    pub author: Option<String>,
}

/// What `edits` answers: each author's log, in the order the authors first edited; each of the
/// world the game runs, since a world took its place, as one restored from a snapshot does, which
/// starts the logs afresh. `play` starts the logs afresh for the copy, and `stop` brings back those
/// of the world as edited.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    pub authors: Vec<Authored>,
}

/// An author's log, as `edits` gives it: the operations in words of each edit `undo` steps back
/// through, the first applied first, so that `undo` undoes the last; and of each `redo` steps
/// through, the last undone last.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authored {
    pub author: String,
    pub done: Vec<Vec<String>>,
    pub undone: Vec<Vec<String>>,
}

/// The levels `logs` reads from, by the names its `level` takes, the most severe first.
pub const LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

/// What `logs` takes: the number of the event to read from, `from`, as an earlier answer's `next`
/// gives it, with that answer's `run`; without `from`, the `last` events to read, or else all, from
/// the oldest kept; the least severe `level` to read, one of [`LEVELS`], every event the game's
/// filter let through without one; and how long to wait for an event to come, if none is there to
/// read, `wait_ms`, in milliseconds, 0 by default and at most 10,000.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Logs {
    #[serde(default)]
    pub from: Option<u64>,
    #[serde(default)]
    pub run: Option<u64>,
    #[serde(default)]
    pub last: Option<usize>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub wait_ms: u64,
}

/// What `logs` answers: the run read, `run`, drawn at random as the game's API started, for the next
/// `logs` to give with `next`, the number of the event to read from then; `end`, the number the
/// next event logged gets; how many events from where it read, before `next`, it `missed`, of any
/// level, dropped as newer ones came or too long to keep, as a line of more than 1 MiB is; and the
/// events read, in order, each the JSON log's line, as written, but for its line break.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEvents {
    pub run: u64,
    pub next: u64,
    pub end: u64,
    pub missed: u64,
    pub events: Vec<Box<RawValue>>,
}

/// The most bytes a [`Note`]'s code may have.
pub const MOST_NOTE_CODE: usize = 64;

/// The most bytes a [`Note`]'s message may have, and its fields as JSON: enough for a build's
/// errors, as rustc renders them, in a line of the log that the game keeps, which may have 1 MiB.
pub const MOST_NOTE: usize = 64 << 10;

/// What `note` takes: the event's code, a stable code, as gg-api's errors have, such as
/// `gg.patch_failed`; its level; its message, which says what happened, naming what it's about;
/// and its fields, if any, an object of JSON, which the game logs as one field, `fields`, its JSON
/// as a string. The game logs it with the client's name, as the connection's `hello` gave it, as
/// `client`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Note {
    pub code: String,
    pub level: NoteLevel,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Map<String, Value>>,
}

/// A note's level: `"info"` or `"warn"`, never an error's, as the run's errors are its own, which a
/// headless run's status counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteLevel {
    Info,
    Warn,
}

/// A connection to a running game's API, for tools: a request at a time, each waiting for its
/// answer.
pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    /// The ID of the next request.
    next: u64,
    welcome: Welcome,
    /// Set once an answer didn't come in time, or the connection broke, after which a line may be
    /// cut short: every call fails then.
    broken: bool,
}

impl Client {
    /// Connects to the game `session` names and says hello as client `name`, the author of the
    /// edits it applies, waiting at most `timeout` to connect and for each answer.
    ///
    /// Fails with `api.connect` if nothing listens there, as once the game has ended;
    /// `api.unauthorized` if the session's token isn't the game's; and as [`call`](Self::call)
    /// does.
    pub fn connect(session: &Session, name: &str, timeout: Duration) -> Result<Self, Failure> {
        let stream = TcpStream::connect_timeout(&session.address, timeout).map_err(|error| {
            let message = format!(
                "can't connect to {} (process {}) at {}: {error}; it has ended, unless it's still \
                 starting",
                session.app, session.pid, session.address
            );
            Failure::new("api.connect", message)
        })?;
        let io = |error: io::Error| Failure::new("api.connect", error.to_string());
        stream.set_nodelay(true).map_err(io)?;
        stream.set_read_timeout(Some(timeout)).map_err(io)?;
        let writer = stream.try_clone().map_err(io)?;
        let mut client = Self {
            reader: BufReader::new(stream),
            writer,
            next: 0,
            welcome: Welcome {
                app: session.app.clone(),
                pid: session.pid,
                engine: session.engine.clone(),
                log: session.log.clone(),
            },
            broken: false,
        };
        let hello = Hello {
            token: session.token.clone(),
            client: Some(name.to_owned()),
        };
        client.welcome = client.call_as("hello", hello)?;
        Ok(client)
    }

    /// What the game said as the connection opened.
    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    /// Waits at most `timeout` for each answer from then on; `None` waits as long as the game
    /// takes.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<(), Failure> {
        let stream = self.reader.get_ref();
        stream
            .set_read_timeout(timeout)
            .map_err(|error| Failure::new("api.connect", error.to_string()))
    }

    /// Sends request `method` with `params`, `null` for a method that takes nothing, and returns
    /// its result's JSON, as the game wrote it.
    ///
    /// Fails with the error the game answers; with `api.timeout` if its answer doesn't come in
    /// time; with `api.closed` if the connection closes first, as when the game ends; and with
    /// `api.protocol` for an answer that isn't one. After `api.timeout` or `api.closed`, every
    /// call fails with `api.closed`.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Box<RawValue>, Failure> {
        let closed = || Failure::new("api.closed", "the connection to the game closed");
        if self.broken {
            return Err(closed());
        }
        let id = self.next;
        self.next += 1;
        let request = Request {
            id,
            method: method.to_owned(),
            params,
        };
        let mut line = serde_json::to_string(&request).expect("a request always serializes");
        line.push('\n');
        if self.writer.write_all(line.as_bytes()).is_err() {
            self.broken = true;
            return Err(closed());
        }
        let line = match read_line(&mut self.reader) {
            Ok(Some(line)) => line,
            Ok(None) => {
                self.broken = true;
                return Err(closed());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                self.broken = true;
                let message =
                    format!("the game didn't answer `{method}` in time; it may be hung or busy");
                return Err(Failure::new("api.timeout", message));
            }
            Err(error) => {
                self.broken = true;
                let message = format!("the connection to the game broke: {error}");
                return Err(Failure::new("api.closed", message));
            }
        };
        let protocol = |what: String| Failure::new("api.protocol", what);
        let answer: Answer = serde_json::from_str(&line)
            .map_err(|error| protocol(format!("an answer that isn't one: {error}: {line}")))?;
        if answer.id != id {
            let message = format!("the answer to request {}, where {id}'s came", answer.id);
            return Err(protocol(message));
        }
        match (answer.result, answer.error) {
            (_, Some(failure)) => Err(failure),
            (Some(result), None) => Ok(result),
            (None, None) => Err(protocol(format!("an answer without a result: {line}"))),
        }
    }

    /// As [`call`](Self::call), with `params` as JSON and the result read as a `T`.
    ///
    /// Fails as `call` does, and with `api.protocol` for a result that isn't a `T`.
    pub fn call_as<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: impl Serialize,
    ) -> Result<T, Failure> {
        let params = serde_json::to_value(params).expect("parameters always serialize");
        let result = self.call(method, params)?;
        serde_json::from_str(result.get()).map_err(|error| {
            let message = format!("`{method}` answered what isn't its result: {error}");
            Failure::new("api.protocol", message)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    #[test]
    fn a_session_file_is_named_as_its_apps_log_is() {
        assert_eq!(Session::file_name("Sandbox", 42), "sandbox-42.json");
        assert_eq!(Session::file_name("My Game 2", 7), "my-game-2-7.json");
    }

    #[test]
    fn sessions_list_their_files_but_those_that_arent_ones() {
        let dir = std::env::temp_dir().join(format!("gg-api-sessions-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let session = |pid| Session {
            app: "Sandbox".to_owned(),
            pid,
            address: "127.0.0.1:4000".parse().unwrap(),
            token: "t".to_owned(),
            log: None,
            engine: "0.1.0 debug".to_owned(),
            aslr_reference: 0x7ff6_1234_5678,
            player: None,
            project: Some(PathBuf::from("games/shards/project.ron")),
        };
        for pid in [2, 1] {
            let text = serde_json::to_string(&session(pid)).unwrap();
            fs::write(dir.join(Session::file_name("Sandbox", pid)), text).unwrap();
        }
        // Being written, beside its own; and something else.
        let whole = serde_json::to_string(&session(3)).unwrap();
        fs::write(dir.join("sandbox-3.json.tmp"), whole).unwrap();
        fs::write(dir.join("sandbox-4.json"), "{\"app\":").unwrap();
        // Of an engine older than projects.
        let mut older = serde_json::to_value(session(5)).unwrap();
        older.as_object_mut().unwrap().remove("project");
        fs::write(dir.join("sandbox-5.json"), older.to_string()).unwrap();
        let listed = Session::list(&dir);
        let pids: Vec<_> = listed.iter().map(|(_, session)| session.pid).collect();
        assert_eq!(pids, [1, 2, 5]);
        assert_eq!(listed[0].1, session(1));
        assert_eq!(listed[2].1.project, None);
        assert!(Session::list(&dir.join("none")).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// An operation is an object of one key, its kind, of what it takes, its defaults left out; a
    /// reparent names its parent, `null` for a root.
    #[test]
    fn operations_read_as_their_docs_say() {
        let read = |json: &str| serde_json::from_str::<Operation>(json);
        let set = read(r#"{"set": {"id": "a", "component": "Spin", "value": 2}}"#).unwrap();
        let expected = Operation::Set {
            id: "a".to_owned(),
            component: "Spin".to_owned(),
            path: String::new(),
            value: serde_json::json!(2),
        };
        assert_eq!(set, expected);
        let root = read(r#"{"reparent": {"id": "a", "parent": null}}"#).unwrap();
        let expected = Operation::Reparent {
            id: "a".to_owned(),
            parent: None,
        };
        assert_eq!(root, expected);
        for refused in [
            r#"{"reparent": {"id": "a"}}"#,
            r#"{"set": {"id": "a", "component": "Spin", "value": 2, "extra": 1}}"#,
            r#"{"delete": {"id": "a"}, "remove": {"id": "a", "component": "Spin"}}"#,
        ] {
            assert!(read(refused).is_err(), "{refused}");
        }
    }

    /// A field's default given as `null` is a default, unlike one left out; bounds keep integers
    /// apart from floats, the widest of a `u64` too.
    #[test]
    fn types_read_as_their_docs_say() {
        let json = r#"{"name": "Spin", "shape": {"struct": {"tuple": false, "fields": [
            {"name": "speed", "type": "f32", "min": {"float": -0.5}, "show": "degrees"},
            {"name": "count", "type": {"int": {"bits": 64, "signed": false}},
             "max": {"int": 18446744073709551615}},
            {"name": "target", "type": {"option": "entity"}, "default": null},
            {"name": "grid", "type": {"array": [{"named": "Cell"}, 4]}}]}}}"#;
        let info: TypeInfo = serde_json::from_str(json).unwrap();
        let Shape::Struct { fields, tuple } = &info.shape else {
            panic!("a struct: {info:?}");
        };
        assert!(!tuple);
        assert_eq!(fields[0].min, Some(Bound::Float(-0.5)));
        assert_eq!(fields[0].show, Show::Degrees);
        assert_eq!(fields[0].default, None);
        assert_eq!(fields[1].max, Some(Bound::Int(u64::MAX.into())));
        assert_eq!(fields[2].ty, Type::Option(Box::new(Type::Entity)));
        assert_eq!(fields[2].default, Some(Value::Null));
        let cell = Box::new(Type::Named("Cell".to_owned()));
        assert_eq!(fields[3].ty, Type::Array(cell, 4));
        let back: TypeInfo = serde_json::from_str(&serde_json::to_string(&info).unwrap()).unwrap();
        assert_eq!(back, info);
    }

    /// `logs` takes nothing but what its docs say, defaulting the rest; and its answer keeps each
    /// event as the log wrote it, its fields in their order.
    #[test]
    fn logs_read_as_their_docs_say_and_answer_events_as_written() {
        let logs: Logs = serde_json::from_str(r#"{"from": 3, "run": 7}"#).unwrap();
        assert_eq!((logs.from, logs.run, logs.last), (Some(3), Some(7), None));
        assert_eq!((logs.level, logs.wait_ms), (None, 0));
        assert!(serde_json::from_str::<Logs>(r#"{"form": 3}"#).is_err());
        let event = r#"{"timestamp":"t","level":"WARN","code":"asset.missing","message":"m"}"#;
        let answer = format!(r#"{{"run":7,"next":4,"end":4,"missed":0,"events":[{event}]}}"#);
        let read: LogEvents = serde_json::from_str(&answer).unwrap();
        assert_eq!((read.run, read.next, read.end, read.missed), (7, 4, 4, 0));
        assert_eq!(read.events.len(), 1);
        assert_eq!(read.events[0].get(), event);
    }

    #[test]
    fn input_events_read_as_their_docs_say() {
        let json = r#"{"events": [{"key": {"key": "KeyW", "pressed": true}}, "cursor_left",
            {"button": {"button": {"other": 9}, "pressed": false}}, {"scroll": {"lines": [0, 1]}},
            {"cursor": [1.5, 2]}, {"text": "a"}, {"focused": false}]}"#;
        let input: Input = serde_json::from_str(json).unwrap();
        let expected = [
            InputEvent::Key {
                key: "KeyW".to_owned(),
                pressed: true,
                repeat: false,
            },
            InputEvent::CursorLeft,
            InputEvent::Button {
                button: Button::Other(9),
                pressed: false,
            },
            InputEvent::Scroll(Scroll::Lines([0.0, 1.0])),
            InputEvent::Cursor([1.5, 2.0]),
            InputEvent::Text("a".to_owned()),
            InputEvent::Focused(false),
        ];
        assert_eq!(input.events, expected);
        let refused = r#"{"events": [{"key": {"key": "KeyW", "pressed": true, "held": true}}]}"#;
        assert!(serde_json::from_str::<Input>(refused).is_err());
    }

    /// A part of the look left out is kept, `null` gives it back to the game, and a value sets it,
    /// written as it's read; a projection is written as reflection writes a camera's.
    #[test]
    fn looks_read_as_their_docs_say() {
        let json = r#"{"viewpoint": {"position": [0, 1, 2.5], "rotation": [0, 0, 0, 1],
            "projection": {"Perspective": {"fov": 1.0, "near": 0.01}}}, "outline": null,
            "grid": "xz"}"#;
        let look: Look = serde_json::from_str(json).unwrap();
        let viewpoint = Viewpoint {
            position: [0.0, 1.0, 2.5],
            rotation: [0.0, 0.0, 0.0, 1.0],
            projection: Projection::Perspective {
                fov: 1.0,
                near: 0.01,
            },
        };
        let expected = Look {
            viewpoint: Some(Some(viewpoint)),
            outline: Some(None),
            grid: Some(Some(Plane::Xz)),
            colliders: None,
        };
        assert_eq!(look, expected);
        let written = serde_json::to_value(&look).unwrap();
        assert_eq!(written.get("outline"), Some(&Value::Null), "given back");
        assert_eq!(written.get("colliders"), None, "kept");
        assert_eq!(serde_json::from_value::<Look>(written).unwrap(), look);
        assert_eq!(serde_json::from_str::<Look>("{}").unwrap(), Look::default());
        let orthographic = Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        };
        assert_eq!(
            serde_json::to_string(&orthographic).unwrap(),
            r#"{"Orthographic":{"height":2.0,"near":-1.0,"far":1.0}}"#
        );
        let set = r#"{"outline": {"id": "2b7e151628aed2a6", "color": [1, 0.5, 0, 1], "width": 2},
            "colliders": false}"#;
        let look: Look = serde_json::from_str(set).unwrap();
        let outline = Outline {
            id: "2b7e151628aed2a6".to_owned(),
            color: [1.0, 0.5, 0.0, 1.0],
            width: 2.0,
        };
        assert_eq!(look.outline, Some(Some(outline)));
        assert_eq!(look.colliders, Some(Some(false)));
        for refused in [
            r#"{"grid": "XZ"}"#,
            r#"{"colliders": true, "camera": null}"#,
            r#"{"outline": {"id": "2b7e151628aed2a6", "color": [1, 0, 0, 1]}}"#,
            r#"{"viewpoint": {"position": [0, 0, 0], "rotation": [0, 0, 0, 1],
                "projection": {"perspective": {"fov": 1.0, "near": 0.01}}}}"#,
            r#"{"viewpoint": {"position": [0, 0, 0], "rotation": [0, 0, 0, 1],
                "projection": {"Perspective": {"fov": 1.0, "near": 0.01, "far": 9}}}}"#,
        ] {
            assert!(serde_json::from_str::<Look>(refused).is_err(), "{refused}");
        }
    }

    /// Each part of the look `null` where it's the game's own, beside who set the others.
    #[test]
    fn looked_reads_as_its_docs_say() {
        let json = r#"{"viewpoint": null, "grid": "xy", "colliders": null,
            "outline": {"id": "2b7e151628aed2a6", "color": [1, 0.5, 0, 1], "width": 2},
            "by": {"viewpoint": null, "outline": "editor", "grid": "gg", "colliders": null},
            "revision": 3, "run": 7, "seen": {"position": [0, 0, 5], "rotation": [0, 0, 0, 1],
            "projection": {"Orthographic": {"height": 2, "near": -1, "far": 1}}}}"#;
        let looked: Looked = serde_json::from_str(json).unwrap();
        let seen = Viewpoint {
            position: [0.0, 0.0, 5.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            projection: Projection::Orthographic {
                height: 2.0,
                near: -1.0,
                far: 1.0,
            },
        };
        let expected = Looked {
            viewpoint: None,
            outline: Some(Outline {
                id: "2b7e151628aed2a6".to_owned(),
                color: [1.0, 0.5, 0.0, 1.0],
                width: 2.0,
            }),
            grid: Some(Plane::Xy),
            colliders: None,
            by: By {
                viewpoint: None,
                outline: Some("editor".to_owned()),
                grid: Some("gg".to_owned()),
                colliders: None,
            },
            revision: 3,
            run: 7,
            seen: Some(seen),
        };
        assert_eq!(looked, expected);
        let back: Looked = serde_json::from_str(&serde_json::to_string(&looked).unwrap()).unwrap();
        assert_eq!(back, looked);
    }

    /// A pick names a pixel of the view, and its answer an entity, or none; a capture is of the
    /// view as it's shown, unless it asks for the game's own.
    #[test]
    fn picks_and_captures_read_as_their_docs_say() {
        let pick: Pick = serde_json::from_str(r#"{"at": [3, 4.5]}"#).unwrap();
        assert_eq!(pick.at, [3.0, 4.5]);
        for refused in ["{}", r#"{"at": [3]}"#, r#"{"at": [3, 4], "frame": 1}"#] {
            assert!(serde_json::from_str::<Pick>(refused).is_err(), "{refused}");
        }
        let picked = r#"{"id": "2b7e151628aed2a6", "name": null}"#;
        let expected = Picked {
            id: Some("2b7e151628aed2a6".to_owned()),
            name: None,
        };
        assert_eq!(serde_json::from_str::<Picked>(picked).unwrap(), expected);
        let capture: Capture = serde_json::from_str("{}").unwrap();
        assert_eq!((capture.path, capture.own), (None, false));
        let capture: Capture = serde_json::from_str(r#"{"own": true}"#).unwrap();
        assert!(capture.own);
        assert!(serde_json::from_str::<Capture>(r#"{"bare": true}"#).is_err());
    }

    /// A scene request says what to do, with a path and whether to discard, when given; a status
    /// from an engine older than `scene` names none, and has nothing unsaved.
    #[test]
    fn scenes_read_as_their_docs_say() {
        let scene: Scene = serde_json::from_str(r#"{"action": "new"}"#).unwrap();
        assert_eq!(
            (scene.action, scene.path, scene.discard),
            (SceneAction::New, None, false)
        );
        let open = r#"{"action": "open", "path": "scenes/arena.ron", "discard": true}"#;
        let scene: Scene = serde_json::from_str(open).unwrap();
        let path = Some(PathBuf::from("scenes/arena.ron"));
        assert_eq!(
            (scene.action, scene.path, scene.discard),
            (SceneAction::Open, path, true)
        );
        for refused in [
            "{}",
            r#"{"action": "close"}"#,
            r#"{"action": "save", "as": "a.ron"}"#,
        ] {
            assert!(serde_json::from_str::<Scene>(refused).is_err(), "{refused}");
        }
        let older = r#"{"ticks": 1, "paused": false, "frames": 2, "entities": 3, "view": null}"#;
        let status: Status = serde_json::from_str(older).unwrap();
        assert_eq!((status.scene, status.unsaved), (None, false));
    }

    /// `assets` takes a kind there is, or none, and a page, defaulting each; and its answer gives
    /// each kind in lowercase, and a listed asset's problem only where it has one.
    #[test]
    fn asset_lists_read_as_their_docs_say() {
        let read =
            |json: &str| serde_json::from_str::<Assets>(json).map(|a| (a.kind, a.skip, a.take));
        assert_eq!(read("{}").unwrap(), (None, 0, 100));
        let all = r#"{"kind": null, "skip": 0, "take": 100}"#;
        assert_eq!(read(all).unwrap(), (None, 0, 100), "as gg asks");
        let models = read(r#"{"kind": "model", "skip": 2}"#).unwrap();
        assert_eq!(models, (Some(asset::AssetKind::Model), 2, 100));
        for refused in [
            r#"{"kind": "sound"}"#,
            r#"{"kind": "Model"}"#,
            r#"{"with": "Sprite"}"#,
        ] {
            assert!(read(refused).is_err(), "{refused}");
        }
        let json = concat!(
            r#"{"total":2,"assets":[{"id":"5f3c9a1e07b2d846","path":"assets/sheet.png","#,
            r#""kind":"texture","loaded":true},{"id":null,"path":"assets/copy.png","#,
            r#""kind":"texture","loaded":false,"problem":{"code":"asset.duplicate_id","#,
            r#""message":"..."}}],"orphans":[{"path":"assets/gone.png.meta","kind":"texture","#,
            r#""id":"0a1b2c3d4e5f6071"}]}"#,
        );
        let expected = AssetList {
            total: 2,
            assets: vec![
                ListedAsset {
                    id: Some("5f3c9a1e07b2d846".to_owned()),
                    path: "assets/sheet.png".to_owned(),
                    kind: asset::AssetKind::Texture,
                    loaded: true,
                    problem: None,
                },
                ListedAsset {
                    id: None,
                    path: "assets/copy.png".to_owned(),
                    kind: asset::AssetKind::Texture,
                    loaded: false,
                    problem: Some(Failure::new("asset.duplicate_id", "...")),
                },
            ],
            orphans: vec![ListedOrphan {
                path: "assets/gone.png.meta".to_owned(),
                kind: asset::AssetKind::Texture,
                id: Some("0a1b2c3d4e5f6071".to_owned()),
            }],
        };
        assert_eq!(serde_json::from_str::<AssetList>(json).unwrap(), expected);
        assert_eq!(serde_json::to_string(&expected).unwrap(), json);
        let model = ListedAsset {
            kind: asset::AssetKind::Model,
            ..expected.assets[0].clone()
        };
        let written = serde_json::to_value(&model).unwrap();
        assert_eq!(written["kind"], "model");
        assert_eq!(written.get("problem"), None, "none to write");
    }

    #[test]
    fn lines_end_at_their_breaks_and_are_kept_short() {
        let mut reader = Cursor::new(b"first\nsecond\r\nthird, cut short".to_vec());
        assert_eq!(read_line(&mut reader).unwrap().as_deref(), Some("first"));
        assert_eq!(read_line(&mut reader).unwrap().as_deref(), Some("second"));
        assert_eq!(read_line(&mut reader).unwrap(), None);
        let mut long = vec![b'x'; MAX_LINE + 1];
        long.push(b'\n');
        let error = read_line(&mut Cursor::new(long)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let mut most = vec![b'x'; MAX_LINE];
        most.push(b'\n');
        let line = read_line(&mut Cursor::new(most)).unwrap();
        assert_eq!(line.map(|line| line.len()), Some(MAX_LINE));
    }

    /// Serves one connection as a game would, answering requests with `answer`, until the client
    /// goes or `answer` gives `None`, which closes it.
    fn serve(answer: impl Fn(Request) -> Option<String> + Send + 'static) -> Session {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            let mut reader = BufReader::new(stream);
            while let Ok(Some(line)) = read_line(&mut reader) {
                let Some(answer) = answer(serde_json::from_str(&line).unwrap()) else {
                    return;
                };
                writer.write_all(answer.as_bytes()).unwrap();
            }
        });
        Session {
            app: "Sandbox".to_owned(),
            pid: 1,
            address,
            token: "secret".to_owned(),
            log: None,
            engine: "0.1.0 debug".to_owned(),
            aslr_reference: 0x7ff6_1234_5678,
            player: None,
            project: None,
        }
    }

    /// Answers `hello` with the token `secret`, `status` with an object whose keys aren't in order,
    /// `slow` never, and anything else with an error; `close` closes the connection.
    fn game(request: Request) -> Option<String> {
        let answer = match request.method.as_str() {
            "hello" if request.params["token"] == "secret" => {
                Ok(r#"{"app":"Sandbox","pid":1,"engine":"0.1.0 debug","log":null}"#.to_owned())
            }
            "hello" => Err(Failure::new("api.unauthorized", "not the token")),
            "status" => Ok(r#"{"zebra":1,"apple":2}"#.to_owned()),
            "slow" => {
                thread::sleep(Duration::from_millis(500));
                Ok("{}".to_owned())
            }
            "close" => return None,
            method => Err(Failure::new("api.unknown_method", method)),
        };
        Some(answer_line(request.id, answer.as_ref().map(String::as_str)))
    }

    #[test]
    fn a_client_says_hello_and_gets_results_as_the_game_wrote_them() {
        let session = serve(game);
        let timeout = Duration::from_secs(10);
        let mut client = Client::connect(&session, "test", timeout).unwrap();
        assert_eq!(client.welcome().app, "Sandbox");
        let status = client.call("status", Value::Null).unwrap();
        assert_eq!(status.get(), r#"{"zebra":1,"apple":2}"#);
        let error = client.call("nonsense", Value::Null).unwrap_err();
        assert_eq!(error, Failure::new("api.unknown_method", "nonsense"));
        let closed = client.call("close", Value::Null).unwrap_err();
        assert_eq!(closed.code, "api.closed");
        let after = client.call("status", Value::Null).unwrap_err();
        assert_eq!(after.code, "api.closed");
    }

    #[test]
    fn a_client_is_refused_without_the_token_and_gives_up_on_slow_answers() {
        let mut wrong = serve(game);
        wrong.token = "guess".to_owned();
        let error = Client::connect(&wrong, "test", Duration::from_secs(10)).err();
        assert_eq!(
            error.map(|error| error.code).as_deref(),
            Some("api.unauthorized")
        );

        let session = serve(game);
        let mut client = Client::connect(&session, "test", Duration::from_secs(10)).unwrap();
        client.set_timeout(Some(Duration::from_millis(50))).unwrap();
        let error = client.call("slow", Value::Null).unwrap_err();
        assert_eq!(error.code, "api.timeout");
        assert_eq!(
            client.call("status", Value::Null).unwrap_err().code,
            "api.closed"
        );

        let mut gone = serve(game);
        gone.address = "127.0.0.1:1".parse().unwrap();
        let error = Client::connect(&gone, "test", Duration::from_secs(10)).err();
        assert_eq!(
            error.map(|error| error.code).as_deref(),
            Some("api.connect")
        );
    }
}
