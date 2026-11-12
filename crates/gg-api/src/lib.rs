//! The tool API's protocol, which a game's development build serves and tools speak, as
//! ARCHITECTURE.md's Agents and tools describes: requests and their answers as lines of JSON over a
//! TCP connection on the machine's loopback; the session file a running game says where it serves
//! in; what each method takes and answers; the memory a game's view is shared through as it
//! renders, [`view`]; and a client, for tools. It also reads a game's project manifest,
//! [`Project`], which tools find a game by and its development build reads; assets' ID files,
//! [`asset`], which its development build writes beside each texture, model, font, prefab and UI;
//! and a project's fetch list, [`Fetch`], of the files `gg fetch` fetches into it. It depends on no
//! crate of the engine's, so that tools build without the GPU's toolchain.
//!
//! A request is `{"id":1,"method":"step","params":{"ticks":2}}`, without `params` for a method that
//! takes none, and its answer `{"id":1,"result":...}` or `{"id":1,"error":{"code":...,"message":...}}`,
//! each a line. A connection's first request is `hello`, with the session's token. The methods:
//!
//! - `hello`, [`Hello`]: answers [`Welcome`]; names the client, whose edits the connection's
//!   `apply`, `undo` and `redo` are, and the capabilities the connection is kept to.
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
//!   it where it is, and that world is the world as edited from then on. Its ticks change the
//!   very world `unsaved` compares, so `unsaved` is true there once physics moves a body:
//!   [`Status::edited`] says whether an author's edit has applied, which ticks never set.
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
//!   shown, the look among it, or as the game shows it on its own, showing what [`Shown`] says in
//!   place of its light if asked, and answers [`Captured`]; fails with `api.bad_params` for an
//!   image's name as `look` refuses it, and for an `at` that isn't finite or is negative. With `at`
//!   it answers the bytes of that pixel, and with `at` alone writes no PNG.
//! - `view`, [`View`]: has the game publish each frame's view, from the next, into the memory a
//!   client made, as [`view::Reader`] makes it, in place of any it published to before, or stop
//!   publishing for `null`, and answers `{}`; fails with `api.view` if the memory can't be opened.
//!   A game run `--embedded` renders its view at the size the memory asks, and runs a frame once
//!   the client has taken the last; another publishes its view at its own size, while it fits.
//! - `input`, [`Input`]: hands the game's layers events, in order, as its window would, as the
//!   frame starts, and answers `{}`.
//! - `look`, [`Look`]: sets parts of the game's look, its view settings, which any client sets, the
//!   last to set a part winning, and which last until a client changes them or the run ends: the
//!   viewpoint the view is seen from, the outline round several entities and every entity under
//!   each, with a glow past its band, a grid, whether colliders'
//!   outlines show, what the view shows in place of its light, [`Shown`], and the passes it leaves
//!   out, [`Pass`]; and answers [`Looked`], the look once set, or as it is, for `{}` or no params.
//!   It's set as a frame starts,
//!   which draws with it. It fails with `api.bad_params`, saying why, for a viewpoint that isn't
//!   finite, whose rotation doesn't normalize, or whose projection can't project, as
//!   [`Projection`] says; for an outline with an ID that isn't 16 hex digits, whose color isn't
//!   finite, or whose width isn't finite and positive; or for an image's name that's empty or
//!   longer than [`MOST_IMAGE_NAME`] bytes. An outline of no IDs, or of an ID no entity has,
//!   outlines nothing, and isn't refused, nor is an image's name the frame doesn't make.
//! - `pick`, [`Pick`]: answers [`Picked`], the entity the view shows at a pixel, as it's shown, the
//!   look among it, in the frame that reads it, about four frames after it's asked.
//! - `images`: answers [`ImageList`], the images the next frame that renders makes that
//!   [`Shown::Image`] names, each by its debug name, with what it holds and its size, in the order
//!   the frame makes them: as the look has the view drawn, its light or an image every pass's but
//!   antialiasing's, which a frame showing an image doesn't make, and a part of shading none after
//!   the view's passes. It fails with `api.paused` while frames pause.
//! - `entities`, [`Entities`]: answers [`EntityList`], a page of the world's entities, with the
//!   revision of its types, as `types` answers it.
//! - `entity`, [`Entity`]: answers the entity: `{"id":...,"name":...,"parent":...,"children":[...],
//!   "components":{"Transform":{...},...},"space":{"matrix":[...],"rotation":[...]}}`, its
//!   components in the order their types registered; and `space`, where the space its `Transform`
//!   is in sits in the world, as the last tick left it: its parent's pose, as a matrix of 16 floats,
//!   column by column, and a rotation, `[x, y, z, w]`, without the parent's scales; the identity
//!   for a root. Its floats are written as a value's are. `bounds` is a [`Sphere`] in the world,
//!   `{"center":[x,y,z],"radius":r}`, round what the entity and every entity under it draw, as the
//!   last tick left them: meshes, sprites and circles, but not text, particles or lines, nor a mesh
//!   whose extent the engine doesn't know; absent when nothing there draws. An instance's entity, its root or one its
//!   prefab gives, has `instance` too, [`Instanced`]: what of it differs from its prefab, which
//!   `revert` sets back, as a scene saves it; reading it parses the prefab's text, and changes
//!   nothing.
//! - `get`, [`Field`]: answers a component's field, as a value.
//! - `types`: answers [`Types`], the world's component types and the descriptions of the types
//!   they hold, which values are read and written by: a client draws and checks values by them, as
//!   the editor's Inspector does; and their revision. A type's description changes as a script
//!   that declares it reloads with other fields, and as a game rebuilt describes its types
//!   otherwise; the revision changes with each change to the types, so that a client reads them
//!   again as the revision `entities` answers moves on.
//! - `hash`: answers [`struct@Hash`], the world's.
//! - `assets`, [`Assets`]: answers [`AssetList`], a page of the textures, models, fonts, prefabs
//!   and UIs under the game's asset root, in the order of their paths, each with its ID, which its
//!   ID file beside it, [`asset::Meta`], holds, as ARCHITECTURE.md's Project state describes, and
//!   the ID files whose assets aren't there. It answers once the game has looked at the root since it
//!   asked, or at once from its last look while its frames pause, as while its window is minimized,
//!   so that an asset added or renamed since is listed, a run that writes ID files, a game alone or
//!   a lockstep session's host that serves the API, having written each one missing and moved
//!   beside its file each that a rename of the file alone left behind, as it does as it starts and
//!   as it watches the root. A listed asset's `problem` says what's wrong with its ID file:
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
//! - `ui`: answers [`UiList`], each UI the game's layers added, as ARCHITECTURE.md's Events and
//!   layers describes, with its nodes laid out for the view: each one's name, kind, parent, its
//!   rectangle in the canvas's units and in the view's pixels, whether it shows, what it says, and
//!   a button's state, so that a client checks a layout as data rather than by eye; it changes
//!   nothing but a layout that its tree, its texts or the view changed since a frame drew it, which
//!   it lays out again. A button is pressed as a click does it, through `input`, at its pixels.
//! - `snapshot`, [`Snapshot`]: saves the world whole as a file, which a run given `--restore`
//!   starts from, and answers [`Snapshotted`] once it's written: a stopped game's file records that
//!   it's stopped, and one taken while a copy plays saves the world as edited too, so that a run
//!   restored from it goes on as it was, paused if it was. Before the first tick of a game playing
//!   from its start, which applies what the layers queued as they started, it fails with
//!   `api.not_ticked`. With `quit`, the run ends once the snapshot is written, as `quit` ends it,
//!   which then needs `quit` too.
//! - `patch`, [`Patch`]: applies a patch of the game's code, which `dx tools hotpatch` built for the
//!   game's process, between ticks, and answers [`Patched`]; fails with `api.bad_params` if the
//!   table's library, `lib`, isn't an absolute path, and with `api.patch` if the build can't take
//!   patches, as [`Session::aslr_reference`] says, the table's reference is past where the game's
//!   `main` is, so that it isn't for this process, the library doesn't load, or it isn't a patch of
//!   the game's build, as one that exports no `main` isn't.
//! - `apply`, [`Apply`]: applies an edit, [`Operation`]s that undo and redo together, as the
//!   frame starts, between ticks, and answers [`Applied`]; it applies all or none, failing with
//!   the code of the first refused, such as `ecs.no_entity`, which names it. The sets of a
//!   gesture, such as a drag, of one field of one entity or several, join the client's last edit
//!   if that's sets of the same field in the same gesture, so that the gesture undoes whole.
//! - `save_prefab`, [`SavePrefab`]: saves an entity and its descendants as a new prefab, and links
//!   the entity to it, as an edit of the client's, and answers [`SavedPrefab`]. The prefab's file
//!   is `path`, absolute or from the asset root, in a folder there that [`asset::scan`] reaches,
//!   ending in `.prefab`, or else in `prefabs/` under the root, which it makes, named after the
//!   entity's `Name` in at most 64 lowercase letters, digits and `_`, or `prefab`, with `-2`, `-3`
//!   and so on after it while a file of that name, or its ID file, is there; written whole, over
//!   nothing, with its ID file beside it, as the game writes an asset's.
//!   Each of its entities' IDs is the one the instance whose root is the entity mixes to the ID
//!   the entity has, so that linking it, which `undo` unlinks, changes no entity's ID, nor anything
//!   a scene saves but the entity's link; and the prefab's root's `Transform` is the identity, the
//!   instance's own being the entity's. `undo` leaves the file. It fails, writing nothing, with:
//!   - `engine.no_root`, in a game that names no asset root;
//!   - `api.no_scene`, in a game that runs from its package, or a networked one, as `scene`'s
//!     `save` does, and while a copy of the world as edited plays, whose entities `stop` takes
//!     away;
//!   - `ecs.no_entity`, for an ID no living entity has;
//!   - `ecs.prefab_entity`, for an entity an instance's prefab gives, `scene.prefab_variant` for an
//!     instance's root, and `ecs.model_node` for a model's node;
//!   - `asset.outside_root`, for a path outside the asset root, whose folder isn't there, or that
//!     the scan skips, as [`asset::reached`] says, as in a folder whose name starts with `.`, in a
//!     `target` or in a top-level `tests`;
//!   - `asset.exists`, for a path where a file is, or the ID file of one deleted, whose ID scenes
//!     may name;
//!   - `engine.bad_prefab`, for a prefab's text of more than 16 MiB, which no load reads;
//!   - and as saving a scene of the entity fails, as with `scene.outside` for one that refers to an
//!     entity outside its subtree.
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
//!   its layers, which may keep a window open. The requests the main thread takes after a `quit`,
//!   or a `snapshot` with `quit`, fail with `api.quitting`, applying nothing.
//! - `debug`, [`Debug`](struct@Debug): debugs the game's Luau scripts, as ARCHITECTURE.md's Game
//!   code describes,
//!   and answers [`Debugging`]: sets a script's breakpoints, then pauses the scripts, or goes on from
//!   where one has stopped, by a continue or a step, then waits for one to stop, as it's asked. A
//!   script stops inside its tick: the main thread waits there, and with it the frames and the
//!   requests the main thread answers, until a `debug` has it go on. It fails with
//!   `script.not_stopped` for a continue or a step while no script has stopped, and with
//!   `api.bad_params` for a script's path that's empty, a line of 0, or a wait of more than 10,000.
//!
//! The methods but `hello`, `ping`, `logs`, `note` and `debug` are answered by the game's main
//! thread, as a frame starts, those that came before it did, the rest as the next starts. While the
//! window is minimized, and no frames run, it answers them between frames, but `step`, `capture` and
//! `pick`, which need frames, fail with `api.paused`; while a file dialog or the window menu Windows
//! runs is open, holding the main thread, or a script has stopped, they wait. The connection's own
//! thread answers `hello`, `ping`, `logs`, `note` and `debug` as it reads them, so that a `logs` or
//! a `debug` that waits holds up the connection's later requests: follow the logs, or wait for a
//! script to stop, on a connection of their own. A connection has at most [`MOST_IN_FLIGHT`]
//! requests in flight, the game reading no more of it until it has written an answer, so that a
//! `ping` sent behind as many that wait for the main thread waits too.
//!
//! A connection is kept to what its `hello` grants, [`Hello::grant`], every capability without it:
//! a request its grant doesn't cover fails with `api.ungranted`, naming the capability it needs.
//! `ping` and `status` need none; `logs`, `entities`, `entity`, `types`, `hash`, `assets`, `ui`
//! and `edits` need `read`, and `get` `read` of its component's type; `apply` needs `write` of the
//! type each set, add and remove names, and `write` for any other operation, as `undo` and `redo`
//! need; `save_prefab` needs `write` and `scene`, as it writes a file of the project's as `scene`
//! does; `pause`, `resume`, `play`, `stop` and `step` need `play`; `capture`, `view`, `look` and
//! `pick` need `view`, a `pick` answering its entity's name only with `read` of `Name` too;
//! `input`, `scene`, `snapshot`, `patch`, `note`, `debug` and `quit` need theirs; a `capture` or a
//! `snapshot` given a path needs `files` too, and a `snapshot` with `quit` needs `quit`.
//!
//! A value is JSON as its kind is: a number, a string, a `bool`; a vector or a quaternion an array
//! of numbers, `[x, y, z, w]` for a quaternion; a float that isn't finite the string `inf`, `-inf`
//! or `NaN`; an entity its ID, a string of 16 hex digits, or `null` if it isn't alive; a texture
//! its name, its asset ID where its file has one, such as `5f3c9a1e07b2d846`, a model's image the
//! model's and its place, such as `5f3c9a1e07b2d846#0`, or else its path, such as
//! `assets/logo.png`, or `null` for one without; a model, a font or a prefab its name, as a
//! texture's; an option `null` or its value; a list an array; a struct an object of its fields, in
//! their order, a tuple struct's named `0`, `1` and so on; an enum's variant its name, as a string,
//! or for a variant with fields an object of one key, its name, whose value is an object of the
//! fields; and what reflection doesn't read, such as a `MeshDraw`, the string `(opaque)`. A value
//! given, as an operation's, is read the same way, by the type of the field it goes in, as a
//! component's description gives it: an integer where a float goes, a texture, a model, a font or a
//! prefab by its name or a path to its file, which the game loads from the file under its asset
//! root if it hasn't, its model's image with its model, an entity by the ID of one that's alive, or
//! `null` for none alive, and a struct, or a variant's fields, missing a field, which takes its
//! default, as in a scene; what's opaque is kept as it is, or made as the type's own is, as an
//! `(opaque)` that a create or an add gives says.

pub mod asset;
pub mod fetch;
pub mod probe;
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

pub use fetch::Fetch;
pub use project::{Project, UNBUILT};

/// The most bytes a line may have, its line break aside, past which the side reading it drops the
/// connection: a request's or an answer's, such as a page of entities.
pub const MAX_LINE: usize = 16 << 20;

/// The most requests a connection has in flight, read by the game and not yet answered on the wire,
/// past which the game reads no more of it until an answer is written: a client that sends faster
/// than it reads its answers, or than they're answered, is held back by the connection, as TCP holds
/// back a writer, rather than the game holding its requests and answers in memory. [`Client`]
/// has one at a time.
pub const MOST_IN_FLIGHT: usize = 16;

/// The most bytes a connection's first line, its `hello`, may have, its line break aside, past which
/// the game closes the connection unanswered, the rest unread. The request's own fields, with the
/// largest `id`, take under 100, the token 32, and a grant of every capability by name under 100,
/// which leaves over 3.8 KiB for the client's name and the component types a grant names.
pub const MAX_HELLO: usize = 4 << 10;

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

/// What `hello` takes: the session's token, the client's name and the capabilities the connection
/// is kept to. Any other first request, or another token, is refused with `api.unauthorized`, and
/// the connection closes.
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
    /// The capabilities the connection is kept to, as ARCHITECTURE.md's Agents and tools says:
    /// `read` and `write` of the world, whole, or of one component type's, as `read:Shard`, and
    /// `play`, `input`, `view`, `scene`, `snapshot`, `debug`, `patch`, `files`, `note` and `quit`;
    /// every one without it. A capability there's none of fails the `hello` with `grant.unknown`,
    /// and the connection closes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<Vec<String>>,
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
/// the main window has no area, `dialog`, while a file dialog is open, `menu`, while the window
/// menu Windows runs is, or `breakpoint`, while a script has stopped, as `debug` says, which it
/// says before any other, as a stopped script holds the main thread whatever else frames wait for.
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
    /// write it, or, untitled, holds entities. False from an engine older than `scene`. In a game
    /// that plays from its start the world as edited is the world that ticks change, so a body its
    /// physics moves makes this true from the first tick: `edited` says what authors changed.
    #[serde(default)]
    pub unsaved: bool,
    /// Whether an author's edit (`apply`, `append`, `amend`, `undo` or `redo`, from a client or
    /// the editor's Inspector) has applied to the world as edited since the open scene was last
    /// saved, opened or made new, or its own file's save applied. Ticks don't set it, nor scripts'
    /// reloads or other scene files' saves; an edit of a copy that plays goes with the copy at
    /// `stop`; a snapshot keeps it, as across `gg run`'s restarts. An undo back to the saved state
    /// leaves it true. False from an engine older than it.
    #[serde(default)]
    pub edited: bool,
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
/// one, a file in the `captures` directory beside the game's executable, named for the run, as
/// `sandbox-4242-7.png`, the newest 32 of which it keeps, removing older ones as each is written:
/// those a client named stay.
///
/// With `at` and without `path`, it writes no file and keeps none of the newest 32 out: it answers
/// the pixel alone, as [`Captured::pixel`] says.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// A pixel of the view, `[x, y]` from its top left, as [`Pick::at`] is, whose bytes the answer
    /// gives as [`Captured::pixel`]; a number that isn't finite, or is negative, is refused with
    /// `api.bad_params`. The pixel is the one the position falls in, as `pick`'s is.
    #[serde(default)]
    pub at: Option<[f64; 2]>,
    /// Whether to capture the view as the game shows it on its own, through its own viewpoint,
    /// without the look's outline, grid or colliders, in a frame that isn't published to a view's
    /// memory, and that a windowed game shows in its window; without it, the capture shows the
    /// view as it's shown, the look among it.
    #[serde(default)]
    pub own: bool,
    /// What to show in place of the view's light, as [`Shown`] says, in place of what the look
    /// shows, in a frame that isn't published to a view's memory; without it, what the look shows,
    /// unless `own`. Captures of several, one after another while the game is paused, show one
    /// world.
    #[serde(default)]
    pub show: Option<Shown>,
}

/// What `capture` answers: where the PNG is, if it wrote one, its size, the view's in pixels, the
/// bytes of the pixel it was asked for, if it was asked, and the ticks the
/// simulation had run in the frame it shows, as that frame drew them: while paused, as the last
/// tick left the world. A capture waits up to 10 s for a frame in which every texture and mesh
/// added before it has come to the GPU, rather than showing their stand-ins; past that, it takes
/// the next frame all the same, and `streaming` says so.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Captured {
    /// The PNG's path; `None` exactly when [`Capture::at`] was given without a `path`, which writes
    /// none.
    #[serde(default)]
    pub path: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    /// The red, green, blue and alpha bytes of the pixel at [`Capture::at`], as the PNG has them
    /// (its alpha, which the PNG leaves out, as the view drew it), each read as [`Shown`] says of
    /// what the capture shows; `None` where it's outside the view, and without `at`.
    #[serde(default)]
    pub pixel: Option<[u8; 4]>,
    pub ticks: u64,
    /// Whether textures or meshes were still decoding or streaming to the GPU in the frame it
    /// shows, which shows a stand-in in a texture's place, or nothing in a mesh's.
    #[serde(default)]
    pub streaming: bool,
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
    /// The mouse moved `[x, y]` in its own units, as its raw motion goes, with no cursor to move:
    /// positive right and down, as the cursor would, for looking about with the cursor grabbed.
    Motion([f64; 2]),
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

/// An outline round what the view shows of several entities and of every entity under each, a
/// model's nodes or a prefab's entities among them, by the entities' IDs, 16 hex digits each: one
/// shape where they touch, in one color, in linear RGBA, alpha not multiplied in; and one width,
/// how wide the solid band is that reaches out from their pixels, in the view's pixels, a width
/// past 8 drawing 8. A glow, at up to half the band's alpha, fades over as far again past it. No
/// IDs outline nothing, as an ID no entity has does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outline {
    pub ids: Vec<String>,
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
    /// The outline round several entities and every entity under each, in place of any the game's
    /// layers draw.
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
    /// What the view shows in place of its light, as [`Shown`] says.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub show: Option<Option<Shown>>,
    /// The passes the view leaves out, as [`Pass`] says, in any order, a pass named twice left out
    /// once; an empty list gives the part back to the game, as `null` does.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub skip: Option<Option<Vec<Pass>>>,
}

/// A pass of the game's view that the look can leave out, to narrow down which one makes what the
/// view shows, by the name of the field of the world's `Environment` that turns it on, or
/// `shadows`, the lights' shadows: the view draws as it would were the game's environment without
/// it, every frame as valid, while the game itself, its environment and lights among it, is
/// unchanged, and a capture of the game's own view shows it as it is. A pass the view doesn't run
/// leaves nothing out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pass {
    /// Ambient occlusion: the light from all around reaches every point whole.
    AmbientOcclusion,
    /// Reflections: what's opaque shows the sky's specular light in their place.
    Reflections,
    /// The lights' shadows: none casts any.
    Shadows,
    /// Height fog.
    Fog,
    /// Bloom.
    Bloom,
    /// Depth of field.
    DepthOfField,
    /// Motion blur.
    MotionBlur,
    /// Antialiasing, temporal or SMAA: each pixel shows what's at its center, drawn without an
    /// offset. Given back, temporal antialiasing starts afresh from the frame that draws with it.
    Antialiasing,
    /// The grade: the view's light as it's drawn.
    Grade,
    /// The lens's vignette, chromatic aberration and grain.
    Lens,
    /// The exposure's adaptation: the view is exposed at the environment's exposure.
    Adaptation,
}

impl Pass {
    /// Every pass, in the order of its variants.
    pub const ALL: [Self; 11] = [
        Self::AmbientOcclusion,
        Self::Reflections,
        Self::Shadows,
        Self::Fog,
        Self::Bloom,
        Self::DepthOfField,
        Self::MotionBlur,
        Self::Antialiasing,
        Self::Grade,
        Self::Lens,
        Self::Adaptation,
    ];
}

/// What the game's view shows in place of its light, to see how it's made: a part of what lit
/// shading takes or makes, at each point of what models draw, by its name, as `"roughness"`; or an
/// image the frame makes, by its debug name, as RenderDoc and ARCHITECTURE.md's Rendering name
/// them: `{"image":"view depth"}`. The view shows it as drawn, without the offset of antialiasing
/// nor the exposure metered. A part of shading shows nothing after it: no antialiasing, fog, depth
/// of field, motion blur, bloom, grade or lens; and no sky, black behind what's drawn; and what
/// draws without lit shading, as sprites, text and particles, draws as it does. An image takes the
/// whole view, alone, from a frame drawn as the game's is but for its antialiasing, so that the
/// images of every pass but antialiasing's are there to show, as `images` lists them.
///
/// A pixel's bytes, as a capture's PNG has them, read as each says: a value of 0 to 1 as its byte
/// over 255, in red, green and blue alike.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Shown {
    /// The base color, its texture's, its vertices' and its factor's, as a texture's sRGB colors
    /// store it; a specular-glossiness material's diffuse color.
    BaseColor,
    /// Which way the point faces in the world, as shading turns it, its normal map's bumps among
    /// it: each axis from -1 to 1 as its byte over 255, times 2, less 1, X in red, Y in green and
    /// Z in blue.
    Normal,
    /// Which way the point faces before its normal map, as [`Shown::Normal`] reads.
    SurfaceNormal,
    /// How metallic it is; 0 for a specular-glossiness material.
    Metallic,
    /// How rough it is, as shading takes it: from 0.045, the least it takes, to 1.
    Roughness,
    /// The share of the light from all around that reaches it: its material's occlusion map's and
    /// the view's ambient occlusion's, as shading takes it.
    Occlusion,
    /// The light it gives off, as [`Shown::BaseColor`] reads, past 1 shown as 1.
    Emission,
    /// What it reflects head-on, as the frame's lights take it: a metallic-roughness material's 0.04
    /// mixed with its base color by how metallic it is; a specular-glossiness material's specular
    /// color. Read as [`Shown::BaseColor`] reads.
    SpecularColor,
    /// The light the frame's lights shade it with, the sun's among them, shadows and all, exposed
    /// and tonemapped as the view's light is; none on an unlit material.
    DirectLight,
    /// The light from all around, the sky's diffuse and specular light, darkened by occlusion, as
    /// [`Shown::DirectLight`] reads.
    SkyLight,
    /// The view's reflection, which takes the place of as much of the sky's specular light, as
    /// [`Shown::DirectLight`] reads.
    Reflection,
    /// The light it diffuses, the frame's lights' and the sky's, as [`Shown::DirectLight`] reads:
    /// beside [`Shown::SpecularLight`], all the light the view shows of it but its emission.
    DiffuseLight,
    /// The light it reflects, the frame's lights' highlights, the sky's specular light and the
    /// view's reflection, as [`Shown::DirectLight`] reads.
    SpecularLight,
    /// An image the frame makes, by its debug name, as its kind reads: colors as they are, of high
    /// dynamic range tonemapped as the view's light is; depth as `1 + log2(depth) / 24`, white at
    /// its near plane, and a 24th darker each time the distance doubles, black where nothing drew;
    /// IDs and a visibility buffer as a color for each value, black for 0; motion as normals are,
    /// in units of the world, -1 to 1; a float a pixel as its value. An image the frame doesn't
    /// make shows black, warning `renderer.no_image`.
    Image(String),
}

/// The most bytes a [`Shown::Image`]'s name has.
pub const MOST_IMAGE_NAME: usize = 64;

/// What `look` answers: the look once set, each part `None` where it's the game's own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Looked {
    pub viewpoint: Option<Viewpoint>,
    pub outline: Option<Outline>,
    pub grid: Option<Plane>,
    pub colliders: Option<bool>,
    /// `None` from an engine older than what the view shows.
    #[serde(default)]
    pub show: Option<Shown>,
    /// The passes the view leaves out, in the order of [`Pass::ALL`], each once, never empty;
    /// `None` from an engine older than passes left out.
    #[serde(default)]
    pub skip: Option<Vec<Pass>>,
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
    #[serde(default)]
    pub show: Option<String>,
    #[serde(default)]
    pub skip: Option<String>,
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
/// ID is kept, or for a connection not granted `read` of `Name`, which `view` alone isn't.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picked {
    pub id: Option<String>,
    pub name: Option<String>,
}

/// What `images` answers: the images the frame that read it made that [`Shown::Image`] names, in
/// the order the frame made them, each name once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageList {
    pub images: Vec<FrameImage>,
}

/// An image a frame makes, as `images` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameImage {
    /// Its debug name, which [`Shown::Image`] takes, as RenderDoc and ARCHITECTURE.md's Rendering
    /// name it: `"view depth"`, `"bloom 1"`.
    pub name: String,
    /// What it holds, which says how a show reads it, as [`Shown::Image`] says.
    pub kind: ImageKind,
    /// Its `[width, height]` in pixels, which a show stretches over the view.
    pub size: [u32; 2],
}

/// What an image a frame makes holds, as [`Shown::Image`] reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageKind {
    /// Colors.
    Color,
    /// Colors of high dynamic range, which a show tonemaps as the view's light is.
    Hdr,
    /// IDs, a color for each.
    Ids,
    /// Motion, in units of the world, as normals are shown.
    Motion,
    /// A visibility buffer, a color for each value.
    Visibility,
    /// Depth, as `1 + log2(depth) / 24`.
    Depth,
    /// A float a pixel, as its value.
    Scalar,
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

/// A sphere in the world's space, as `entity` answers `bounds`: the center, in world units, and the
/// radius.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sphere {
    pub center: [f32; 3],
    pub radius: f32,
}

/// What `entity` answers of an instance's entity, its root or one its prefab gives, under
/// `instance`: the ID of the root of the outermost instance whose prefab gives it, or its own for
/// the root of an instance none gives, and that instance's prefab, by its path from the asset
/// root; the entity's own prefab, likewise, if it's an instance's root, nested or not, which alone
/// an outermost instance's root unlinks; each component the prefab gives it that differs from what
/// the prefab gives, by its type's name, with the fields that differ, a struct of named fields' by
/// name, as a scene saves them, and else `""`, the whole component, each a path `revert` takes; and
/// the components it has that the prefab doesn't give it, which a scene saves whole, but the
/// instance's own: a root's `Transform` and `PrefabRoot`, and a `Parent`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instanced {
    pub root: String,
    pub prefab: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default)]
    pub overrides: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub added: Vec<String>,
}

/// What `save_prefab` takes: the ID of the entity to save with its descendants, and the path of
/// the prefab's file, absolute or from the asset root, or `None` for one in `prefabs/` named after
/// the entity.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavePrefab {
    pub id: String,
    #[serde(default)]
    pub path: Option<PathBuf>,
}

/// What `save_prefab` answers: the prefab's file, by its path from the asset root, with `/` between
/// its parts, as [`Status::scene`] names files; its name, its asset ID or else its path, as values
/// name it; and the ticks the simulation had run as the link applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPrefab {
    pub path: String,
    pub name: String,
    pub ticks: u64,
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
/// `"prefab"`, `{"option":...}`, `{"list":...}`, `{"array":[..., 4]}`, `{"named":"Transform"}`, a
/// type `types` describes, or `"opaque"`, a field reflection leaves as it is.
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
    Prefab,
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

/// What `ui` answers: the view's size in its pixels, as the last frame handed it off, or 1,920 by
/// 1,080 before one has, which each node's `pixels` are in; and each UI the game's layers added, in
/// the order they added them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiList {
    pub view: [u32; 2],
    pub uis: Vec<ListedUi>,
}

/// A UI as `ui` lists it: the name its layer gave it; its file's name, its asset ID or else its
/// path, as values name a texture; its file's path from the asset root, as [`Status::scene`] names
/// files; whether the last frame handed off drew it; whether it takes input; the button with the
/// focus, by its name; and its nodes, in the order of its file, each after its parent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListedUi {
    pub name: String,
    pub asset: String,
    pub path: String,
    pub drawn: bool,
    pub input: bool,
    pub focus: Option<String>,
    pub nodes: Vec<UiNode>,
}

/// A node of a UI as `ui` lists it: its name, if its file gives it one; its kind; its parent's
/// place among the UI's nodes, `None` for one at its top; its rectangle, `[x, y, width, height]`
/// from the view's top left, +Y down, in the canvas's units, 1,080 to the view's height, and in the
/// view's pixels, as a capture's PNG has them, a list's children's where it's scrolled them; whether
/// it shows, it and each node above it visible, a hidden one's rectangle of no size; what a text, a
/// button or a toggle says, what's typed in a text field, or a drop-down's choice; what a slider,
/// a toggle or a drop-down holds; and a button's or a widget's whether it's disabled, has the
/// focus, or is held pressed, a drop-down's while its options are open.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiNode {
    pub name: Option<String>,
    pub kind: UiKind,
    pub parent: Option<usize>,
    pub rect: [f32; 4],
    pub pixels: [f32; 4],
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<UiValue>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub focused: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pressed: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// What a node of a UI is, as its file names it: a panel, a background behind the children it
/// stacks; a text; an image; a button; a widget, a slider, a toggle, a drop-down or a text field;
/// or a container that lays its children out in a row, a column, stacked over each other, or in a
/// list, a column that clips and scrolls them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiKind {
    Panel,
    Text,
    Image,
    Button,
    Slider,
    Toggle,
    Dropdown,
    Field,
    Row,
    Column,
    Stack,
    List,
}

/// What a widget of a UI holds, as `ui` lists it: a slider's number, a toggle's whether it's on,
/// or a drop-down's choice, by its place among its options, from 0; as `{"number": 0.5}`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiValue {
    Number(f32),
    On(bool),
    Choice(usize),
}

/// The methods that change nothing of the game but what they're asked to read, which a client may
/// send again whether or not the first was taken.
const READS: [&str; 13] = [
    "hello", "ping", "status", "entities", "entity", "get", "types", "hash", "assets", "ui",
    "edits", "logs", "pick",
];

/// Whether `method` may change the game, its world, its run or its files, as every method but those
/// that only read does, and one there's none of may: a request of it that went unanswered may have
/// applied, so it's sent again only once a read says it hasn't.
pub fn changes(method: &str) -> bool {
    !READS.contains(&method)
}

/// What an error of `method` that went unanswered adds, as [`Client::call`] says: that it may have
/// applied, if it [`changes`] the game; else nothing.
pub fn unanswered(method: &str) -> &'static str {
    if changes(method) {
        "; the game may have taken it, and it may yet apply, or have applied: read what it changes \
         before sending it again"
    } else {
        ""
    }
}

/// What `snapshot` takes: where to write the snapshot, an absolute path, whose directory exists;
/// without one, a file in the `snapshots` directory beside the game's executable, named for the
/// run, as `sandbox-4242-7.snapshot`, the newest 8 of which it keeps, removing older ones as each
/// is written: those a client named stay, as `gg run`'s, in a directory of its own. With `quit`, the run ends cleanly once the snapshot is
/// written and answered, as `quit` ends it, nothing applying between the two, so that a run started
/// again from the snapshot loses no edit answered before.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quit: bool,
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
    /// Makes entity `id` the last of `parent`'s children, or a root for `null`: keeping its own
    /// `Transform`, so that it moves as its new parent is placed; or, `in_place`, keeping where it
    /// is in the world, as an editor's reparent does, its transform set, in the same edit, to its
    /// place in its new parent's space as the request finds the two, any shear they give it
    /// between them lost.
    Reparent {
        id: String,
        /// Given, `null` for a root, so that one left out isn't taken for a root.
        #[serde(deserialize_with = "Option::deserialize")]
        parent: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        in_place: bool,
    },
    /// Unlinks the instance of a prefab whose root is entity `id`: takes its `PrefabRoot` away,
    /// its entities staying as they are, under their IDs, its own from then on.
    Unlink { id: String },
    /// Sets the field at `path` of entity `id`'s `component`, or the whole component for an empty
    /// path, to what the prefab of the instance that gives it has there: an instance's root's, but
    /// its `Transform`, which is its own; or an entity's that an instance's prefab gives.
    Revert {
        id: String,
        component: String,
        #[serde(default)]
        path: String,
    },
}

/// What `apply` takes: an edit's operations, applied in order; and, for a set that's part of a
/// gesture, such as a drag or a number typed, the gesture, a number the client gives each of its
/// own, the same for each of its sets: a set of the field that the client's last edit set, with the
/// same gesture, joins that edit, taking its value and keeping what undoes it, so that the gesture
/// undoes whole, whether it sets that field of one entity or of several (a set of another entity
/// is added to the edit), and any other edit of the client's that reaches the world ends it, refused there
/// or not; one refused as it's read, as for an ID that isn't one, a type there's none of or a value
/// of another kind, doesn't. Since a client's edits are those of every connection with its name, a
/// gesture's number is its own across them, as one drawn at random to start from is. With a
/// gesture, an apply is of sets, each of one component's one field, no two of one entity, or
/// refused with `api.bad_params`.
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

/// What `debug` takes, each part of it optional, done in this order: the breakpoints to set in a
/// script, in place of those it had; what to do, `action`; and how long to wait for a script to
/// stop, if none has, `wait_ms`, in milliseconds, 0 by default and at most 10,000. `{}` answers the
/// debugger as it is.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Debug {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoints: Option<Breakpoints>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<DebugAction>,
    #[serde(default)]
    pub wait_ms: u64,
}

/// A script's breakpoints, as `debug` sets them: its path from the project's root, as
/// [`Debugging::scripts`] names it, and their lines, from 1; none clears them. A script no layer has
/// loaded keeps them until one does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Breakpoints {
    pub script: String,
    pub lines: Vec<u32>,
}

/// What `debug` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugAction {
    /// Stops the next line a script runs, breaking into one that's running, such as a loop that
    /// doesn't end; nothing while one has stopped.
    Pause,
    /// From a stop, goes on to the next.
    Continue,
    /// From a stop, stops at the next line that runs, in a function it calls too.
    StepInto,
    /// From a stop, stops at the next line of the function stopped in, past the calls it makes but
    /// for a breakpoint in one, or once it returns, of its caller.
    StepOver,
    /// From a stop, stops at the next line once the function stopped in returns, of its caller.
    StepOut,
}

/// What `debug` answers: where a script has stopped, while one has; what a stop to come answers,
/// `asked`, a pause or a step under way; how many stops the run has had, which number them; the
/// breakpoints, by script, in the order of the scripts' paths, then of the lines asked; and the
/// scripts the game's layers have loaded, by their paths from the project's root, in order.
///
/// A step out of a call into a script, from a tick or a chunk as it loads, or over its last line,
/// stops at the next line a script runs, such as the next entity's tick. A call steps only if it
/// started while the debugger was armed, a breakpoint placed or a pause or a step asked: a step
/// from where a pause broke into one that started otherwise stops at the next line a script runs
/// after it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Debugging {
    pub stopped: Option<Stop>,
    pub asked: Option<DebugAction>,
    pub stops: u64,
    pub breakpoints: Vec<Breakpoint>,
    pub scripts: Vec<String>,
}

/// A breakpoint: its script, the line asked, and the line Luau placed it at, the next with code
/// from the line asked, or `None` while its script hasn't loaded, or has no code there or after.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakpoint {
    pub script: String,
    pub line: u32,
    pub placed: Option<u32>,
}

/// Where a script has stopped: the stop's number, from 1; why; and the stack, the function stopped
/// in first, then the one that called it, and so on to the call into the script.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub number: u64,
    pub why: StopReason,
    pub frames: Vec<StackFrame>,
}

/// Why a script stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StopReason {
    Breakpoint,
    Step,
    Pause,
}

/// A function of the stack where a script has stopped: its name, `(chunk)` for a chunk's top level
/// and `(anonymous)` for one without; its script's path; the line it's at; and its locals and
/// upvalues, as their values are there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StackFrame {
    pub function: String,
    pub script: String,
    pub line: u32,
    pub locals: Vec<Variable>,
    pub upvalues: Vec<Variable>,
}

/// A local or an upvalue: its name, and its value as JSON: `null` for `nil`, a number or a string
/// as its kind is, the string at most [`MOST_SHOWN`] bytes, cut; a vector `[x, y, z, w]`; an entity
/// its ID, or `null` for `gg.entity` and for one that isn't alive; and anything else its kind in
/// parentheses, such as `(table)`, `(function)`, or `(handle)` for a field's handle. A float that
/// isn't finite is `inf`, `-inf` or `NaN`, as a value's is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub value: Value,
}

/// The most bytes of a string a [`Variable`] shows.
pub const MOST_SHOWN: usize = 1 << 10;

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
        Self::connect_granted(session, name, None, timeout)
    }

    /// As [`connect`](Self::connect), the connection kept to the capabilities `grant` names, as
    /// [`Hello::grant`] says, or granted every one for `None`.
    ///
    /// Fails as `connect` does, and with `grant.unknown` for a capability there's none of.
    pub fn connect_granted(
        session: &Session,
        name: &str,
        grant: Option<Vec<String>>,
        timeout: Duration,
    ) -> Result<Self, Failure> {
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
            grant,
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
    /// call fails with `api.closed`. Unanswered, a request the game took may still apply, as one
    /// answered late does: the message of either says so of a method that [`changes`] the game.
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
                let message = format!(
                    "the connection to the game closed before it answered `{method}`{}",
                    unanswered(method)
                );
                return Err(Failure::new("api.closed", message));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                self.broken = true;
                let message = format!(
                    "the game didn't answer `{method}` in time; it may be hung or busy{}",
                    unanswered(method)
                );
                return Err(Failure::new("api.timeout", message));
            }
            Err(error) => {
                self.broken = true;
                let message = format!(
                    "the connection to the game broke before it answered `{method}`: {error}{}",
                    unanswered(method)
                );
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

/// What `body` does with `call`, sending each request through `call`: `status` first, then `pause`
/// if the game plays, neither paused nor stopped, then `body`, which sees one world, and `resume` if
/// and only if it paused it.
///
/// Fails as `body` does, over a failed `resume`.
pub fn while_paused<T>(
    mut call: impl FnMut(&str, Value) -> Result<Box<RawValue>, Failure>,
    body: impl FnOnce(
        &mut dyn FnMut(&str, Value) -> Result<Box<RawValue>, Failure>,
    ) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let status: Status = serde_json::from_str(call("status", Value::Null)?.get())
        .map_err(|error| Failure::new("api.protocol", format!("`status` answered {error}")))?;
    let plays = !status.paused && !status.stopped;
    if plays {
        call("pause", Value::Null)?;
    }
    let done = body(&mut call);
    let resumed = if plays {
        call("resume", Value::Null).map(drop)
    } else {
        Ok(())
    };
    // A failed body is the failure to report, over a failed resume.
    let done = done?;
    resumed?;
    Ok(done)
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
            in_place: false,
        };
        assert_eq!(root, expected);
        let json = r#"{"reparent": {"id": "a", "parent": "b", "in_place": true}}"#;
        let expected = Operation::Reparent {
            id: "a".to_owned(),
            parent: Some("b".to_owned()),
            in_place: true,
        };
        assert_eq!(read(json).unwrap(), expected);
        let kept = serde_json::to_value(&expected).unwrap();
        assert_eq!(kept["reparent"]["in_place"], true);
        let moved = serde_json::to_value(Operation::Reparent {
            id: "a".to_owned(),
            parent: Some("b".to_owned()),
            in_place: false,
        });
        assert_eq!(moved.unwrap()["reparent"].get("in_place"), None, "left out");
        let revert = read(r#"{"revert": {"id": "a", "component": "Sprite"}}"#).unwrap();
        let expected = Operation::Revert {
            id: "a".to_owned(),
            component: "Sprite".to_owned(),
            path: String::new(),
        };
        assert_eq!(revert, expected);
        let unlink = read(r#"{"unlink": {"id": "a"}}"#).unwrap();
        assert_eq!(unlink, Operation::Unlink { id: "a".to_owned() });
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
            {"cursor": [1.5, 2]}, {"text": "a"}, {"focused": false}, {"motion": [3, -4.5]}]}"#;
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
            InputEvent::Motion([3.0, -4.5]),
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
            show: None,
            skip: None,
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
        let set = r#"{"outline": {"ids": ["2b7e151628aed2a6", "0123456789abcdef"],
            "color": [1, 0.5, 0, 1], "width": 2}, "colliders": false}"#;
        let look: Look = serde_json::from_str(set).unwrap();
        let outline = Outline {
            ids: vec!["2b7e151628aed2a6".to_owned(), "0123456789abcdef".to_owned()],
            color: [1.0, 0.5, 0.0, 1.0],
            width: 2.0,
        };
        assert_eq!(look.outline, Some(Some(outline.clone())));
        let written = serde_json::to_value(&outline).unwrap();
        assert_eq!(
            written["ids"],
            serde_json::json!(["2b7e151628aed2a6", "0123456789abcdef"])
        );
        let empty = r#"{"outline": {"ids": [], "color": [1, 0, 0, 1], "width": 1}}"#;
        let none: Look = serde_json::from_str(empty).unwrap();
        assert_eq!(none.outline.unwrap().unwrap().ids, Vec::<String>::new());
        assert_eq!(look.colliders, Some(Some(false)));
        // What the view shows: a part of shading by its name, or an image by its own.
        let shows = [
            (r#""base_color""#, Shown::BaseColor),
            (r#""surface_normal""#, Shown::SurfaceNormal),
            (r#""direct_light""#, Shown::DirectLight),
            (r#""specular_color""#, Shown::SpecularColor),
            (r#""diffuse_light""#, Shown::DiffuseLight),
            (r#""specular_light""#, Shown::SpecularLight),
            (
                r#"{"image": "view depth"}"#,
                Shown::Image("view depth".to_owned()),
            ),
        ];
        for (json, show) in shows {
            let look: Look = serde_json::from_str(&format!(r#"{{"show": {json}}}"#)).unwrap();
            assert_eq!(look.show, Some(Some(show.clone())), "{json}");
            let written = serde_json::to_value(&show).unwrap();
            assert_eq!(written, serde_json::from_str::<Value>(json).unwrap());
        }
        let look: Look = serde_json::from_str(r#"{"show": null}"#).unwrap();
        assert_eq!(look.show, Some(None), "given back");
        // The passes left out, by their names, written as they're read.
        let look: Look = serde_json::from_str(r#"{"skip": ["reflections", "depth_of_field"]}"#)
            .expect("passes by their names");
        let skip = vec![Pass::Reflections, Pass::DepthOfField];
        assert_eq!(look.skip, Some(Some(skip)));
        let written = serde_json::to_value(&look).unwrap();
        let names = serde_json::json!({"skip": ["reflections", "depth_of_field"]});
        assert_eq!(written, names);
        let look: Look = serde_json::from_str(r#"{"skip": null}"#).unwrap();
        assert_eq!(look.skip, Some(None), "given back");
        let names = Pass::ALL.map(|pass| serde_json::to_value(pass).unwrap());
        let expected = [
            "ambient_occlusion",
            "reflections",
            "shadows",
            "fog",
            "bloom",
            "depth_of_field",
            "motion_blur",
            "antialiasing",
            "grade",
            "lens",
            "adaptation",
        ];
        assert_eq!(names, expected.map(|name| serde_json::json!(name)));
        for refused in [
            r#"{"skip": ["Reflections"]}"#,
            r#"{"skip": ["tonemapper"]}"#,
            r#"{"skip": "fog"}"#,
            r#"{"show": "BaseColor"}"#,
            r#"{"show": "albedo"}"#,
            r#"{"show": {"image": "view depth", "level": 0}}"#,
            r#"{"grid": "XZ"}"#,
            r#"{"colliders": true, "camera": null}"#,
            r#"{"outline": {"ids": ["2b7e151628aed2a6"], "color": [1, 0, 0, 1]}}"#,
            r#"{"outline": {"id": "2b7e151628aed2a6", "color": [1, 0, 0, 1], "width": 2}}"#,
            r#"{"viewpoint": {"position": [0, 0, 0], "rotation": [0, 0, 0, 1],
                "projection": {"perspective": {"fov": 1.0, "near": 0.01}}}}"#,
            r#"{"viewpoint": {"position": [0, 0, 0], "rotation": [0, 0, 0, 1],
                "projection": {"Perspective": {"fov": 1.0, "near": 0.01, "far": 9}}}}"#,
        ] {
            assert!(serde_json::from_str::<Look>(refused).is_err(), "{refused}");
        }
    }

    /// An image list reads and writes each image's name, its kind by the name of what it holds, and
    /// its size.
    #[test]
    fn images_read_and_write_by_their_names() {
        let json = serde_json::json!({"images": [
            {"name": "view depth", "kind": "depth", "size": [64, 32]},
            {"name": "bloom 1", "kind": "hdr", "size": [32, 16]},
        ]});
        let listed: ImageList = serde_json::from_value(json.clone()).expect("a list");
        let bloom = FrameImage {
            name: "bloom 1".to_owned(),
            kind: ImageKind::Hdr,
            size: [32, 16],
        };
        assert_eq!(listed.images[1], bloom);
        assert_eq!(listed.images[0].kind, ImageKind::Depth);
        assert_eq!(serde_json::to_value(&listed).expect("written"), json);
        let kinds = [
            ImageKind::Color,
            ImageKind::Hdr,
            ImageKind::Ids,
            ImageKind::Motion,
            ImageKind::Visibility,
            ImageKind::Depth,
            ImageKind::Scalar,
        ];
        let names = kinds.map(|kind| serde_json::to_value(kind).expect("a name"));
        let want = [
            "color",
            "hdr",
            "ids",
            "motion",
            "visibility",
            "depth",
            "scalar",
        ];
        assert_eq!(names, want.map(|name| serde_json::json!(name)));
    }

    /// Each part of the look `null` where it's the game's own, beside who set the others.
    #[test]
    fn looked_reads_as_its_docs_say() {
        let json = r#"{"viewpoint": null, "grid": "xy", "colliders": null,
            "outline": {"ids": ["2b7e151628aed2a6"], "color": [1, 0.5, 0, 1], "width": 2},
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
                ids: vec!["2b7e151628aed2a6".to_owned()],
                color: [1.0, 0.5, 0.0, 1.0],
                width: 2.0,
            }),
            grid: Some(Plane::Xy),
            colliders: None,
            // From an engine older than what the view shows and the passes it leaves out, which
            // names neither.
            show: None,
            skip: None,
            by: By {
                viewpoint: None,
                outline: Some("editor".to_owned()),
                grid: Some("gg".to_owned()),
                colliders: None,
                show: None,
                skip: None,
            },
            revision: 3,
            run: 7,
            seen: Some(seen),
        };
        assert_eq!(looked, expected);
        let shown = Looked {
            show: Some(Shown::Roughness),
            skip: Some(vec![Pass::Shadows, Pass::Adaptation]),
            by: By {
                show: Some("gg".to_owned()),
                skip: Some("agent".to_owned()),
                ..expected.by
            },
            ..expected
        };
        let back: Looked = serde_json::from_str(&serde_json::to_string(&shown).unwrap()).unwrap();
        assert_eq!(back, shown);
    }

    /// A sphere reads as `entity` writes `bounds`, and takes no other field.
    #[test]
    fn spheres_read_as_their_docs_say() {
        let sphere = r#"{"center": [1, 2.5, -3], "radius": 0.5}"#;
        let expected = Sphere {
            center: [1.0, 2.5, -3.0],
            radius: 0.5,
        };
        assert_eq!(serde_json::from_str::<Sphere>(sphere).unwrap(), expected);
        let extra = r#"{"center": [0, 0, 0], "radius": 1, "color": 0}"#;
        assert!(serde_json::from_str::<Sphere>(extra).is_err());
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
        assert_eq!(
            (capture.path, capture.own, capture.show),
            (None, false, None)
        );
        let capture: Capture = serde_json::from_str(r#"{"own": true}"#).unwrap();
        assert!(capture.own);
        let capture: Capture = serde_json::from_str(r#"{"show": "normal"}"#).unwrap();
        assert_eq!(capture.show, Some(Shown::Normal));
        assert!(serde_json::from_str::<Capture>(r#"{"bare": true}"#).is_err());
        let capture: Capture = serde_json::from_str(r#"{"at": [3, 4.5]}"#).unwrap();
        assert_eq!((capture.at, capture.path), (Some([3.0, 4.5]), None));
        assert!(serde_json::from_str::<Capture>(r#"{"at": [3]}"#).is_err());
    }

    /// A capture's answer has a path unless it was asked for a pixel alone, and the pixel's bytes
    /// when asked; one from an engine older than `at` has a path and no pixel.
    #[test]
    fn captured_reads_with_or_without_its_path_and_pixel() {
        let old = r#"{"path": "/c/a.png", "width": 4, "height": 2, "ticks": 7}"#;
        let captured: Captured = serde_json::from_str(old).unwrap();
        assert_eq!(captured.path, Some(PathBuf::from("/c/a.png")));
        assert_eq!(captured.pixel, None);
        let probe =
            r#"{"path": null, "width": 4, "height": 2, "pixel": [1, 2, 3, 255], "ticks": 7}"#;
        let captured: Captured = serde_json::from_str(probe).unwrap();
        assert_eq!(
            (captured.path, captured.pixel),
            (None, Some([1, 2, 3, 255]))
        );
        let bare = r#"{"width": 4, "height": 2, "ticks": 7}"#;
        assert_eq!(serde_json::from_str::<Captured>(bare).unwrap().path, None);
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
        assert_eq!(
            (status.scene, status.unsaved, status.edited),
            (None, false, false)
        );
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
    /// `slow`, `apply` and `hash` late, and anything else with an error; `close` closes the
    /// connection.
    fn game(request: Request) -> Option<String> {
        let answer = match request.method.as_str() {
            "hello" if request.params["token"] == "secret" => {
                Ok(r#"{"app":"Sandbox","pid":1,"engine":"0.1.0 debug","log":null}"#.to_owned())
            }
            "hello" => Err(Failure::new("api.unauthorized", "not the token")),
            "status" => Ok(r#"{"zebra":1,"apple":2}"#.to_owned()),
            "slow" | "apply" | "hash" => {
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

    /// A call that changes the game, answered too late or not at all, may have applied, which its
    /// error says, and a read's doesn't.
    #[test]
    fn an_unanswered_change_is_said_to_maybe_have_applied() {
        let mut said = Vec::new();
        for method in ["apply", "hash", "close"] {
            let session = serve(game);
            let mut client = Client::connect(&session, "test", Duration::from_secs(10)).unwrap();
            client.set_timeout(Some(Duration::from_millis(50))).unwrap();
            let error = client.call(method, Value::Null).unwrap_err();
            said.push((error.code, error.message.contains("may yet apply")));
        }
        let said: Vec<(&str, bool)> = (said.iter())
            .map(|(code, may)| (code.as_str(), *may))
            .collect();
        assert_eq!(
            said,
            [
                ("api.timeout", true),
                ("api.timeout", false),
                ("api.closed", true)
            ]
        );
        assert!(changes("apply") && changes("snapshot") && !changes("status"));
    }
}
