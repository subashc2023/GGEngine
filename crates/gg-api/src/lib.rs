//! The tool API's protocol, which a game's development build serves and tools speak, as
//! ARCHITECTURE.md's Agents and tools describes: requests and their answers as lines of JSON over a
//! TCP connection on the machine's loopback; the session file a running game says where it serves
//! in; what each method takes and answers; and a client, for tools. It also reads a game's project
//! manifest, [`Project`], which tools find a game by and its development build reads. It depends on
//! no crate of the engine's, so that tools build without the GPU's toolchain.
//!
//! A request is `{"id":1,"method":"step","params":{"ticks":2}}`, without `params` for a method that
//! takes none, and its answer `{"id":1,"result":...}` or `{"id":1,"error":{"code":...,"message":...}}`,
//! each a line. A connection's first request is `hello`, with the session's token. The methods:
//!
//! - `hello`, [`Hello`]: answers [`Welcome`]; names the client, whose edits the connection's
//!   `apply`, `undo` and `redo` are.
//! - `ping`: answers [`Pulse`] at once, even while the game's main thread is busy.
//! - `status`, `pause` and `resume`: answer [`Status`].
//! - `step`, [`Step`]: runs ticks, pausing the simulation, and answers [`Status`] once they've run.
//! - `capture`, [`Capture`]: writes the game's view as a PNG once a frame renders it, and answers
//!   [`Captured`].
//! - `entities`, [`Entities`]: answers [`EntityList`], a page of the world's entities.
//! - `entity`, [`Entity`]: answers the entity: `{"id":...,"name":...,"parent":...,"children":[...],
//!   "components":{"Transform":{...},...}}`, its components in the order their types registered.
//! - `get`, [`Field`]: answers a component's field, as a value.
//! - `hash`: answers [`struct@Hash`], the world's.
//! - `snapshot`, [`Snapshot`]: saves the world whole as a file, which a run given `--restore`
//!   starts from, and answers [`Snapshotted`] once it's written; before the simulation's first tick
//!   in the run, which applies what the layers queued as they started, it fails with
//!   `api.not_ticked`.
//! - `patch`, [`Patch`]: applies a patch of the game's code, which `dx tools hotpatch` built for the
//!   game's process, between ticks, and answers [`Patched`]; fails with `api.bad_params` if the
//!   table's library, `lib`, isn't an absolute path, and with `api.patch` if the build can't take
//!   patches, as [`Session::aslr_reference`] says, the table's reference is past where the game's
//!   `main` is, so that it isn't for this process, the library doesn't load, or it isn't a patch of
//!   the game's build, as one that exports no `main` isn't.
//! - `apply`, [`Apply`]: applies an edit, [`Operation`]s that undo and redo together, as the
//!   frame starts, between ticks, and answers [`Applied`]; it applies all or none, failing with
//!   the code of the first refused, such as `ecs.no_entity`, which names it.
//! - `undo` and `redo`, [`Undo`]: undo the last edit of an author, the client's own by default,
//!   such as a scene file's, which its path names, or redo the last it undid, and answer
//!   [`Edit`], or `null` if there's none; an edit the world refuses to undo or redo, as when an
//!   entity it changed is gone since, fails, and is dropped.
//! - `edits`, [`Edits`]: answers [`History`], an author's edits, or each author's.
//! - `quit`: answers `{}`, and the run ends cleanly, as when its window closes, but without asking
//!   its layers, which may keep a window open.
//!
//! The methods but `hello` and `ping` are answered by the game's main thread, as a frame starts.
//! While the window is minimized, and no frames run, it answers them between frames, but `step`
//! and `capture`, which need frames, fail with `api.paused`; while a file dialog or the window menu
//! Windows runs is open, holding the main thread, they wait.
//!
//! A value is JSON as its kind is: a number, a string, a `bool`; a vector or a quaternion an array
//! of numbers, `[x, y, z, w]` for a quaternion; a float that isn't finite the string `inf`, `-inf`
//! or `NaN`; an entity its ID, a string of 16 hex digits, or `null` if it isn't alive; a texture
//! its path, such as `assets/logo.png`, or `null` for one without; an option `null` or its value; a
//! list an array; a struct an object of its fields, in their order, a tuple struct's named `0`,
//! `1` and so on; an enum's variant its name, as a string, or for a variant with fields an object
//! of one key, its name, whose value is an object of the fields; and what reflection doesn't read,
//! such as a `MeshDraw`, the string `(opaque)`. A value given, as an operation's, is read the same
//! way, by the type of the field it goes in, as a component's description gives it: an integer
//! where a float goes, a texture by the path a layer added it from, an entity by the ID of one
//! that's alive, or `null` for none alive, and a struct, or a variant's fields, missing a field,
//! which takes its default, as in a scene; what's opaque is kept as it is, or made as the type's
//! own is, as an `(opaque)` that a create or an add gives says.

mod project;

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

pub use project::Project;

/// The most bytes a line may have, its line break aside, past which the side reading it drops the
/// connection: a request's or an answer's, such as a page of entities.
pub const MAX_LINE: usize = 16 << 20;

/// Where a running game serves the API, which its development build writes as a file of JSON in
/// the `sessions` directory beside its executable as the API starts, and removes as the run ends;
/// one that a crash or a kill leaves behind names an address that refuses connections.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// The app's name, as `App::new` gives it, such as `Sandbox`.
    pub app: String,
    /// Its process's ID.
    pub pid: u32,
    /// Where it listens, on the loopback.
    pub address: SocketAddr,
    /// What a client's `hello` gives, so that only processes that can read the file talk to it.
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
fn present<'de, D: serde::Deserializer<'de>>(json: D) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(json).map(Some)
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

/// What `status`, `pause`, `resume` and `step` answer: the game as the frame that answers starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// The ticks the simulation has run, which number the next.
    pub ticks: u64,
    /// Whether the simulation is paused, as `pause` and `step` leave it.
    pub paused: bool,
    /// The frames the game has run.
    pub frames: u64,
    /// The world's living entities.
    pub entities: usize,
    /// The game's view's width and height in pixels, as the last frame handed it to the renderer;
    /// `None` while it had no area.
    pub view: Option<[u32; 2]>,
}

/// What `step` takes: how many ticks to run, 1 by default and at most 10,000, a frame's worth
/// each frame. The simulation stays paused after them; `resume` runs it again, and drops the
/// steps not yet run, answering those waiting.
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

/// What `entities` answers: how many there are in all, and the page asked for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityList {
    pub total: usize,
    pub entities: Vec<Listed>,
}

/// An entity as `entities` lists it: its ID, its name, its parent's ID, and its components' types,
/// in the order they registered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    pub id: String,
    pub name: Option<String>,
    pub parent: Option<String>,
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

/// What `hash` answers: the world's hash, its content's and its layout's, each 16 hex digits, as
/// recordings write them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hash {
    pub content: String,
    pub layout: String,
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

/// What `apply` takes: an edit's operations, applied in order.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Apply {
    pub operations: Vec<Operation>,
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
/// starts the logs afresh.
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
