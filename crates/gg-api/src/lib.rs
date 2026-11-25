//! The tool API's protocol, which a game's development build serves and tools speak
//! (ARCHITECTURE.md's Agents and tools).
//!
//! Also here: the session file ([`Session`]), the view's shared memory ([`view`]), a [`Client`],
//! the project manifest ([`Project`]), assets' ID files ([`asset`]), the fetch list ([`Fetch`]) and
//! a game's source digest ([`build`]). No engine crate is a dependency, so tools and build scripts
//! build without the GPU's toolchain.
//!
//! # Wire
//!
//! Lines of JSON over TCP on the loopback, at most [`MAX_LINE`] bytes each. A request is
//! `{"id":1,"method":"step","params":{"ticks":2}}`, `params` left out for none; its answer
//! `{"id":1,"result":...}` or `{"id":1,"error":{"code":...,"message":...}}` ([`Failure`]; codes are
//! stable dotted names, such as `ecs.no_entity`). The first request is [`Hello`]. The session file
//! names the protocol's revision, [`PROTOCOL`].
//!
//! # Entities
//!
//! Where a request takes an entity (`entity`, `get`, `save_prefab`, `pick`'s `within`, and the `id`
//! and `parent` of [`Operation`]s), its value is an ID or a name. Text of 16 hex digits is an ID.
//! Any other is the `Name` of the one entity that has it, else the request fails with
//! `ecs.no_entity`, naming the text, for none, or `ecs.ambiguous_name`, listing their IDs, for
//! several. The IDs a request makes (`create`'s `id`, `duplicate`'s `copies`) are IDs alone.
//!
//! # Methods
//!
//! `-` is none; `needs` is a [grant](#grants)'s capability. Notes follow the table.
//!
//! | method | params | result | needs |
//! |---|---|---|---|
//! | `hello` | [`Hello`] | [`Welcome`] | - |
//! | `ping` | - | [`Pulse`] | - |
//! | `status` | - | [`Status`] | - |
//! | `pause`, `resume`, `play`, `stop` | - | [`Status`] | `play` |
//! | `step` | [`Step`] | [`Status`] | `play` |
//! | `scene` | [`Scene`] | [`Status`] | `scene` |
//! | `capture` | [`Capture`] | [`Captured`] | `view` |
//! | `frames` | [`Frames`] | [`Framed`] | `view`, `files` |
//! | `view` | [`View`] | `{}` | `view` |
//! | `input` | [`Input`] | `{}` | `input` |
//! | `look` | [`Look`] | [`Looked`] | `view` |
//! | `pick` | [`Pick`] | [`Picked`] | `view` |
//! | `images` | - | [`ImageList`] | `view` |
//! | `entities` | [`Entities`] | [`EntityList`] | `read` |
//! | `entity` | [`Entity`] | see notes | `read` |
//! | `get` | [`Field`] | a value | `read` |
//! | `types` | - | [`Types`] | `read` |
//! | `hash` | - | [`struct@Hash`] | `read` |
//! | `cells` | - | [`CellList`] | `read` |
//! | `assets` | [`Assets`] | [`AssetList`] | `read` |
//! | `ui` | - | [`UiList`] | `read` |
//! | `snapshot` | [`Snapshot`] | [`Snapshotted`] | `snapshot` |
//! | `patch` | [`Patch`] | [`Patched`] | `patch` |
//! | `apply` | [`Apply`] | [`Applied`] | `write` |
//! | `save_prefab` | [`SavePrefab`] | [`SavedPrefab`] | `write`, `scene` |
//! | `undo`, `redo` | [`Undo`] | [`Edit`] or `null` | `write` |
//! | `edits` | [`Edits`] | [`History`] | `read` |
//! | `logs` | [`Logs`] | [`LogEvents`] | `read` |
//! | `note` | [`Note`] | `{}` | `note` |
//! | `quit` | - | `{}` | `quit` |
//! | `debug` | [`Debug`](struct@Debug) | [`Debugging`] | `debug` |
//!
//! ## Notes
//!
//! - `ping`: answers at once, even while the main thread is busy.
//! - `play`, `stop`: nothing in the state already held.
//!
//!   `stop` freezes the world as edited (frames run no ticks; only edits, scene files' saves and
//!   `scene`'s `open` and `new` change it) until `play`, which plays a copy in its place, from its
//!   ticks. Stopping the copy restores the world as edited at its ticks, with its scene files as
//!   last applied (a save since applies) and the authors' edits, which `undo` steps back through
//!   again. The copy's edits and entities go and the IDs it drew are drawn again: no ID learned
//!   while it played holds across a `stop`.
//!
//!   A game playing from its start, which no `play` started, has no world as edited: `stop` stops
//!   it where it is, and that world is the world as edited from then on. Its ticks change the world
//!   `unsaved` compares, so `unsaved` is true once physics moves a body; [`Status::edited`] is
//!   never set by ticks.
//!
//!   `stop` fails waiting `step`s with `api.stopped`, and itself with `api.no_stop` in a recorded
//!   run or a lockstep peer, which play from start to end.
//! - `step`: runs ticks, pausing the simulation; answers after them. `api.stopped` while stopped.
//! - `scene`: opens, saves or empties the open scene: the scene file the game started in or opened
//!   since ([`Status::scene`]).
//!
//!   It owns the entities its file held as last applied and clients' creates and duplicates since,
//!   never the game's code's. `open` replaces them with a scene file's and watches the file; `new`
//!   deletes them, leaving it untitled; `save` writes them to its file or to `path`, which becomes
//!   its file (the watch applies nothing for that).
//!
//!   In a world of cells their entities are the scene's too: `save` writes each streamed root, with
//!   what's under it, to its cell's file, each that would change, and the rest to the world's file;
//!   `new` and `open` delete them and empty the cells. While stopped, cells come in around the
//!   look's viewpoint, and an edited one stays in until saved. A cell's file saved on disk applies
//!   to its cell as the scene's does.
//!
//!   Each acts on the world as edited: `save` writes it while a copy plays; `open` and `new` stop
//!   the copy first, and can't be undone: every author's edits start afresh, as after a restore.
//!
//!   `path` is absolute or from the asset root ([`Status::scene`]), in a project scene folder,
//!   ending `.ron`. Fails with:
//!   - `engine.no_project`: not a project's game;
//!   - `scene.bad_path`: a path outside the scene folders;
//!   - `scene.unsaved`: `open` or `new` with unsaved edits, unless `discard`;
//!   - `scene.untitled`: `save` of an untitled scene without a path;
//!   - `scene.watched`: a path another scene file the game loaded has;
//!   - `scene.changed`: `save` while the file, or a cell's file it would write, differs from what
//!     the game last applied (an edit to it isn't lost);
//!   - `scene.cell_out`: `save` while a streamed root stands in a cell whose file hasn't come in,
//!     as one refused or a root just moved there (its entities would be lost);
//!   - `api.no_scene`: a lockstep peer, and `open` and `new` in a recorded run;
//!   - and as loading or saving a scene fails, as `scene.file` or `scene.outside`.
//! - `capture`: writes the view as a PNG once a frame renders it. `api.bad_params` for an image
//!   name `look` refuses.
//! - `frames`: captures frames of the view in a row, none skipped. One asked while another captures
//!   waits, and so does a `capture` that shows otherwise. Fails with:
//!   - `api.bad_params`: as `capture` and `look` would for its `show` and viewpoints; a `dir` that
//!     isn't an absolute path to a directory; a `count` of 0 or past [`MOST_FRAMES`]; a `rect` of
//!     no area; frames past [`MOST_FRAMES_BYTES`] of pixels;
//!   - `api.no_view`: a frame without a view, with none of the `rect`, or that doesn't render, as
//!     while a window resizes (which would break the row);
//!   - `api.capture_file`: a PNG it can't write.
//! - `view`: has the game publish each frame's view, from the next, into the memory a client made
//!   with [`view::Reader`], in place of any before; `null` stops. `api.view` if the memory can't be
//!   opened. A run `--embedded` renders at the memory's size and runs a frame once the client has
//!   taken the last; another publishes at its own size, while it fits.
//! - `input`: hands the game's layers the events in order, as its window would, as the frame
//!   starts.
//! - `look`: sets parts of the look, the view settings clients share, until a client changes it or
//!   the run ends; the last setter of a part wins. Set as a frame starts, which draws with it; `{}`
//!   or no params answers the look as it is. `api.bad_params`, saying why, for:
//!   - a viewpoint that isn't finite, whose rotation doesn't normalize, or whose projection can't
//!     project ([`Projection`]);
//!   - an outline with an ID that isn't 16 hex digits, a color that isn't finite, or a width that
//!     isn't finite and positive;
//!   - an image name that's empty or past [`MOST_IMAGE_NAME`] bytes.
//!
//!   Not refused: an outline that outlines nothing, an image name the frame doesn't make.
//! - `pick`: the entity the view shows at a pixel as shown (the look among it), in the frame that
//!   reads it, about four frames on. `api.bad_params` for a `within` that isn't 16 hex digits.
//! - `images`: the images [`Shown::Image`] names that the next frame that renders makes, by debug
//!   name, with what each holds and its size, in the frame's order. They follow the look: with the
//!   light drawn or an image shown, every pass's but antialiasing's, which a frame showing an image
//!   doesn't make; with a part of shading, none after the view's passes.
//! - `entity` answers an object of `id`, `name`, `parent`, `children`, `components`
//!   (`{"Transform":{...},...}`) and `space` (`{"matrix":[...],"rotation":[...]}`), and `bounds`
//!   and `instance` where they apply:
//!   - `name`, as [`Listed`] has it; `components`, in their types' registration order;
//!   - `space`: where its `Transform`'s space sits in the world as the last tick left it, as its
//!     parent's pose, 16 floats column by column, and a rotation, `[x, y, z, w]`, without the
//!     parent's scales; the identity for a root. Floats are written as a value's are;
//!   - `bounds`: a [`Sphere`], `{"center":[x,y,z],"radius":r}`, round what the entity and those
//!     under it draw, as the last tick left them: meshes, sprites and circles, not text, particles,
//!     lines or a mesh of unknown extent. Absent where nothing draws;
//!   - `instance` ([`Instanced`]), for an instance's entity: what differs from its prefab, as a
//!     scene saves it. Reading it parses the prefab's text, changing nothing.
//! - `types`: the component types and the descriptions of the types they hold, by which values are
//!   read and written and a client draws and checks them, as the Inspector does. A description
//!   changes as a script declaring it reloads with other fields, or a rebuilt game describes it
//!   otherwise; the revision changes with each, so a client reads them again as `entities`'
//!   revision moves on.
//! - `cells`: in a world whose `Cells` cut it into scene files that come in around its viewers
//!   (gg-engine's `cells` module): which are in, coming, kept, refused or filed; the viewers, the
//!   look's viewpoint among them while stopped; and which `scene`'s `save` would write.
//! - `assets`: a page of the textures, models, fonts, prefabs and UIs under the asset root, with
//!   their ID files' IDs ([`asset::Meta`]). It answers once the game has looked at the root since
//!   it asked, or at once from its last look while frames pause, so an asset added or renamed since
//!   is listed. A run that writes ID files (a game alone or a lockstep host serving the API) has by
//!   then written each missing one and moved beside its file each that a rename left behind, as it
//!   does as it starts and watches the root. An asset's `problem` says what's wrong with its ID
//!   file:
//!   - `asset.no_id`: none, in a run that writes none, a lockstep joiner: its host writes it, as a
//!     game alone does;
//!   - `asset.bad_meta`: one that doesn't read; fix it, or remove it for a fresh one;
//!   - `asset.meta_case`: one whose name differs from `<file>.meta` in case alone; rename it;
//!   - `asset.duplicate_id`: one holding the ID another asset has this run, as a copy's does, or
//!     none while another ID file holds its ID. From the next run the first asset by path whose ID
//!     file holds the ID has it, as the message says; another goes without until its file is
//!     removed, after which a run writes it a fresh one: remove a copy's ID file, not the
//!     original's;
//!   - `asset.id_changed`: one changed since the run read it; the run keeps its ID until it ends;
//!   - `asset.unwritten`: one the run couldn't write, saying why.
//!
//!   An orphan is an ID file left behind, as by a rename the game couldn't pair with its file, as
//!   one made with an edit: it goes beside its asset, named for it, or is deleted. `engine.no_root`
//!   in a game with no asset root; `asset.unlisted` if the root can't be listed.
//! - `ui`: each UI the layers added (ARCHITECTURE.md's Events and layers), its nodes laid out for
//!   the view, so a client checks a layout as data. Changes nothing but a layout whose tree, texts
//!   or view changed since a frame drew it, which it lays out again. A click at a button's pixels,
//!   through `input`, presses it.
//! - `snapshot`: saves the whole world as a file a run given `--restore` starts from. A stopped
//!   game's file records that it's stopped; one taken while a copy plays saves the world as edited
//!   too, so a restored run goes on as it was, paused if it was. `api.not_ticked` before the first
//!   tick of a game playing from its start, which applies what the layers queued as they started.
//! - `patch`: applies, between ticks, a patch of the game's code that `dx tools hotpatch` built for
//!   its process. `api.bad_params` if the table's library, `lib`, isn't an absolute path.
//!   `api.patch` if the build can't take patches ([`Session::aslr_reference`]), the table's
//!   reference is past where `main` is (it isn't for this process), the library doesn't load, or it
//!   isn't a patch of the build, as one exporting no `main` isn't.
//! - `apply`: applies an edit, [`Operation`]s that undo and redo together, as the frame starts,
//!   between ticks. All or none: it fails with the code of the first refused, such as
//!   `ecs.no_entity`, which names it.
//! - `save_prefab`: saves an entity and its descendants as a new prefab and links the entity to it,
//!   as the client's edit. The file is `path`, absolute or from the asset root, in a folder
//!   [`asset::scan`] reaches, ending `.prefab`. Without one, it's in `prefabs/` under the root,
//!   which it makes, named after the entity's `Name` in at most 64 lowercase letters, digits and
//!   `_`, or `prefab`, then `-2`, `-3` and so on while a file of that name or its ID file is there.
//!   It's written whole, over nothing, with its ID file, as the game writes an asset's. Each prefab
//!   entity's ID is the one the instance rooted at the entity mixes to the entity's ID, so linking,
//!   which `undo` unlinks, changes no entity's ID nor anything a scene saves but the link. The
//!   prefab root's `Transform` is the identity, the instance's own being the entity's. `undo`
//!   leaves the file. Fails, writing nothing, with:
//!   - `engine.no_root`: no asset root;
//!   - `api.no_scene`: a game run from its package or networked, as `scene`'s `save` does, and
//!     while a copy of the world as edited plays (`stop` takes its entities away);
//!   - `ecs.no_entity`: an ID or name no living entity has; `ecs.ambiguous_name`: a name several
//!     have;
//!   - `ecs.prefab_entity`: an entity an instance's prefab gives; `scene.prefab_variant`: an
//!     instance's root; `ecs.model_node`: a model's node;
//!   - `asset.outside_root`: a path outside the asset root, whose folder isn't there, or that the
//!     scan skips ([`asset::reached`]), as in a folder starting `.`, a `target` or a top-level
//!     `tests`;
//!   - `asset.exists`: a path where a file is, or the ID file of one deleted, whose ID scenes may
//!     name;
//!   - `engine.bad_prefab`: a prefab's text over 16 MiB, which no load reads;
//!   - and as saving a scene of the entity fails, as `scene.outside` for one that refers outside
//!     its subtree.
//! - `undo`, `redo`: undo an author's last edit, the client's own by default or, as a scene file's,
//!   one its path names; or redo the last it undid. `null` for none. An edit the world refuses to
//!   undo or redo, as when an entity it changed is gone, fails and is dropped.
//! - `logs`: events of the game's JSON log, of the last it keeps, at most 10,000 and 8 MiB of
//!   lines. A `from` given with another run's `run`, as a client following the logs across a
//!   restart gives, reads from the oldest kept, so it misses nothing it can still have.
//! - `note`: has the game log an event the client gives, so those following its logs learn what the
//!   client knows, as `gg run` tells of a failed patch or rebuild. `api.bad_params` for a code that
//!   isn't a stable code's shape, a dotted name such as `gg.patch_failed`, or is past
//!   [`MOST_NOTE_CODE`] bytes; an empty message; a message or fields past [`MOST_NOTE`] bytes.
//! - `quit`: the run ends cleanly, as when its window closes, without asking its layers, which may
//!   keep a window open. Requests the main thread takes after a `quit`, or a `snapshot` with
//!   `quit`, fail with `api.quitting`, applying nothing.
//! - `debug`: debugs the game's Luau scripts (ARCHITECTURE.md's Game code). A stopped script holds
//!   the main thread inside its tick, and with it the frames and the requests the main thread
//!   answers, until a `debug` has it go on. `script.not_stopped` for a continue or a step while
//!   none has stopped; `api.bad_params` for an empty path, a line of 0, or a wait past 10,000.
//!
//! # Threads
//!
//! The main thread answers all but `hello`, `ping`, `logs`, `note` and `debug` as a frame starts,
//! those that came before it, the rest as the next starts. While the window is minimized and no
//! frames run, it answers between frames, but `step`, `capture`, `frames`, `pick` and `images`,
//! which need frames, fail with `api.paused`. While a file dialog or Windows' window menu holds the
//! main thread, or a script has stopped, they wait.
//!
//! The connection's own thread answers `hello`, `ping`, `logs`, `note` and `debug` as it reads
//! them, so a `logs` or `debug` that waits holds up the connection's later requests: follow the
//! logs, or wait for a script, on a connection of its own. At most [`MOST_IN_FLIGHT`] requests are
//! in flight, so a `ping` behind as many that wait for the main thread waits too.
//!
//! # Grants
//!
//! A connection is kept to what its `hello` grants ([`Hello::grant`]), every capability without
//! one. A request beyond it fails with `api.ungranted`, naming the capability. Besides the table's
//! `needs`:
//!
//! - `get` needs `read` of its component's type; `apply`, `write` of the type each `set`, `add`,
//!   `remove` and `revert` names, and of the whole world for any other operation;
//! - `pick` names its entity only with `read` of `Name` too;
//! - a `capture` or a `snapshot` given a path needs `files` too, and a `snapshot` with `quit`,
//!   `quit` too.
//!
//! # Values
//!
//! JSON by kind:
//!
//! - a number, a string, a `bool`;
//! - a vector or a quaternion, an array of numbers, `[x, y, z, w]` for a quaternion;
//! - a float that isn't finite, the string `inf`, `-inf` or `NaN`;
//! - an entity, its ID, a string of 16 hex digits, or `null` if it isn't alive;
//! - a texture, its asset ID where its file has one, such as `5f3c9a1e07b2d846`, a model's image
//!   the model's and its place, such as `5f3c9a1e07b2d846#0`, or else its path, such as
//!   `assets/logo.png`, or `null` for none; a model, a font or a prefab likewise;
//! - an option, `null` or its value; a list, an array;
//! - a struct, an object of its fields in order, a tuple struct's named `0`, `1` and so on;
//! - an enum's variant, its name as a string, or for a variant with fields an object of one key,
//!   its name, holding an object of the fields;
//! - what reflection doesn't read, such as a `MeshDraw`, the string `(opaque)`.
//!
//! A value given, as an operation's, is read alike, by the type of the field it goes in, as a
//! component's description gives it. It also takes an integer where a float goes; a texture, a
//! model, a font or a prefab by name or by a path to its file, which the game loads from under its
//! asset root if it hasn't, a model's image with its model; an entity by a living one's ID, or
//! `null` for none; and a struct or a variant's fields missing a field, which takes its default, as
//! in a scene. What's opaque is kept as it is, or made as the type's own is, as an `(opaque)` that
//! a create or an add gives says.

pub mod asset;
pub mod build;
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
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

pub use fetch::Fetch;
pub use project::{Project, UNBUILT};

/// The most bytes a line may have, its line break aside: past it the reader drops the connection.
pub const MAX_LINE: usize = 16 << 20;

/// The most requests a connection has in flight, read and not yet answered.
///
/// Past it the game reads no more until an answer is written: TCP holds back a client that outpaces
/// the answers, instead of the game holding requests in memory. [`Client`] has one at a time.
pub const MOST_IN_FLIGHT: usize = 16;

/// The most bytes of a connection's first line, its `hello`, line break aside: past it the game
/// closes the connection unanswered.
///
/// The request's fields with the largest `id` take under 100, the token 32, a grant of every
/// capability by name under 100: over 3.8 KiB is left for the client's name and the component types
/// a grant names.
pub const MAX_HELLO: usize = 4 << 10;

/// The revision of what `gg` and a game's engine rely on of each other.
///
/// The session file names it ([`Session::protocol`]); `gg` warns of a game whose isn't its own
/// (`gg.protocol`). Bump it for a change after which an older `gg` would misread a game, a game it,
/// or either be refused: a method, parameter, answer or session-file field, or error code, renamed,
/// removed or changed in meaning; a required parameter added; a change to the flags `gg` starts a
/// game with or the snapshot files it reads. Adding a method or an optional field needs none.
pub const PROTOCOL: u32 = 1;

/// A running game's session file: where it serves the API.
///
/// Its development build writes it in `sessions` beside its executable as the API starts, and
/// removes it as the run ends; one a crash or kill leaves names an address that refuses
/// connections. Only the game's user can read it: mode 0600 on Unix, a DACL for its owner alone on
/// Windows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// The app's name, as `App::new` gives it, such as `Sandbox`.
    pub app: String,
    pub pid: u32,
    /// On the loopback.
    pub address: SocketAddr,
    /// What `hello` gives, so that only processes that can read the file, the user's own, talk to
    /// it.
    pub token: String,
    /// The run's JSON log, unless it couldn't be created.
    pub log: Option<PathBuf>,
    /// The engine's version and build, such as `0.1.0 debug`.
    pub engine: String,
    /// Where `main` is in the process, which `dx tools hotpatch` builds a patch against
    /// (`--aslr-reference`); the OS loads the executable elsewhere each run. 0 in a build that
    /// can't take patches: one exporting no `main` (cargo's for Windows or Linux), or without debug
    /// assertions, which subsecond needs to look a patch up. cargo's macOS debug build exports
    /// `main`, but only patches of `dx build --fat-binary`'s build suit it.
    pub aslr_reference: u64,
    /// Its player in a lockstep session, 0 for the host and 1 for the joiner; `None` for a game
    /// alone, or from an engine older than lockstep.
    #[serde(default)]
    pub player: Option<u8>,
    /// The project manifest's path, as the game named it, by which tools inside the project reach
    /// it; `None` for an app that isn't a project, or from an engine older than projects.
    #[serde(default)]
    pub project: Option<PathBuf>,
    /// The [`PROTOCOL`] the game's engine was built with; `None` from an engine older than it.
    #[serde(default)]
    pub protocol: Option<u32>,
}

impl Session {
    /// The directory of the session file of a game whose executable is in `exe_dir`.
    pub fn dir(exe_dir: &Path) -> PathBuf {
        exe_dir.join("sessions")
    }

    /// The session file's name for app `app` in process `pid`, such as `sandbox-1234.json`: the
    /// app's name in lower case, each character but ASCII letters and digits a `-`, as its log's.
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

    /// The sessions whose files are in `dir`, with their paths, in name order; none if `dir` can't
    /// be read. A file that isn't a session's is skipped, as one being written is.
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
    /// The result's JSON, as the game wrote it, `null` too, as an `undo` with nothing to undo
    /// answers.
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

/// The line answering request `id`, with its result's JSON or its error.
pub fn answer_line(id: u64, answer: Result<&str, &Failure>) -> String {
    match answer {
        Ok(result) => format!("{{\"id\":{id},\"result\":{result}}}\n"),
        Err(failure) => {
            let failure = serde_json::to_string(failure).expect("strings always serialize");
            format!("{{\"id\":{id},\"error\":{failure}}}\n")
        }
    }
}

/// Reads a line without its `\n` or `\r\n`; `None` once the stream ends, even partway through a
/// line, as a connection cut short does.
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
/// message saying what happened, naming what it's about.
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

/// What `hello` takes.
///
/// Any other first request, or another token, is refused with `api.unauthorized`, and the
/// connection closes.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub token: String,
    /// The client, author of the connection's edits, which its `undo` and `redo` step through;
    /// `api` without one. Connections of one name share them: a client undoes its own edits across
    /// connections, as each of `gg`'s commands does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// The capabilities the connection is kept to ([Grants](crate#grants)): `read` and `write` of
    /// the whole world or of one component type, as `read:Shard`, and `play`, `input`, `view`,
    /// `scene`, `snapshot`, `debug`, `patch`, `files`, `note` and `quit`; every one without it. An
    /// unknown capability fails the `hello` with `grant.unknown`, closing the connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<Vec<String>>,
}

/// What `hello` answers: the game the connection reaches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub app: String,
    pub pid: u32,
    /// As [`Session::engine`].
    pub engine: String,
    /// As [`Session::log`].
    pub log: Option<PathBuf>,
}

/// What `ping` answers: frames run, and why frames are paused.
///
/// `frames` stops counting if the main thread hangs. `waiting`: `minimized`, while the main window
/// has no area; `dialog`, while a file dialog is open; `menu`, while Windows' window menu is;
/// `breakpoint`, while a script has stopped (`debug`), which comes first as it holds the main
/// thread whatever else frames wait for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pulse {
    pub frames: u64,
    pub waiting: Option<String>,
}

/// What `status`, `pause`, `resume`, `play`, `stop` and `step` answer: the game as the answering
/// frame starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// The ticks run, which number the next. After a `stop` that brings back the world as edited,
    /// that world's, so they may go back.
    pub ticks: u64,
    /// As `pause` and `step` leave it, across `stop` and `play`.
    pub paused: bool,
    /// As `stop` and `--stopped` leave it: the world is the world as edited, which no tick
    /// changes, until `play`. False from an engine older than `stop`.
    #[serde(default)]
    pub stopped: bool,
    pub frames: u64,
    /// The world's living entities.
    pub entities: usize,
    /// The view's width and height in pixels, as the last frame handed it to the renderer; `None`
    /// while it had no area.
    pub view: Option<[u32; 2]>,
    /// The open scene's file from the asset root, such as `scenes/arena.ron`: what `scene` acts
    /// on, of the world as edited. `None` while untitled, as after `new`, in a game that isn't a
    /// project's, and from an engine older than `scene`.
    #[serde(default)]
    pub scene: Option<String>,
    /// Whether the open scene, in the world as edited, differs from its file as `save` would write
    /// it, or, untitled, holds entities. False from an engine older than `scene`. For a game
    /// playing from its start, see `stop`.
    #[serde(default)]
    pub unsaved: bool,
    /// Whether an author's edit (`apply`, `append`, `amend`, `undo` or `redo`, from a client or the
    /// Inspector) has applied to the world as edited since the open scene was last saved, opened or
    /// made new, or its own file's save applied. Ticks, scripts' reloads and other scene files'
    /// saves don't set it. A playing copy's edit goes with the copy at `stop`; a snapshot keeps it,
    /// as across `gg run`'s restarts. An undo back to the saved state leaves it true. False from an
    /// engine older than it.
    #[serde(default)]
    pub edited: bool,
    /// In a world of cells (`Cells` cuts it into scene files that come in around its viewers and go
    /// out as they leave): how many are in, and how many out keep what they held. `None` where
    /// cells never came in, and from an engine older than cells.
    #[serde(default)]
    pub cells: Option<CellCounts>,
}

/// How many of a world's cells are in, and how many out keep what they held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellCounts {
    #[serde(rename = "in")]
    pub cells_in: usize,
    pub kept: usize,
}

/// What `cells` answers: the world's cells, `None` in a world without `Cells`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CellList {
    pub world: Option<WorldCells>,
}

/// A world of cells, as `cells` reads it.
///
/// `entity` is the one whose `Cells` it streams by; `folder`, from the asset root, and `size`, a
/// cell's side; `viewers`, each entity's with a `Viewer` and, while the game is stopped, the look's
/// viewpoint; and `cells`, each in, coming in, keeping what it held, refused, holding a streamed
/// root or filed in the folder, ordered by x, then y, then z.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldCells {
    pub entity: String,
    pub folder: String,
    pub size: f32,
    pub viewers: Vec<ViewerAt>,
    pub cells: Vec<CellAt>,
}

/// A viewer: its entity's ID, `None` for the look's viewpoint; where it is, as the last tick left
/// it; and how far from it cells come in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewerAt {
    pub entity: Option<String>,
    pub position: [f64; 3],
    pub radius: f32,
}

/// A cell of a world's cells.
///
/// `cell` is its place on the grid, as its file's name has it, `3_0_-2` for `[3, 0, -2]`; `file`,
/// whether the folder holds its file, as last listed, else the game's code `made` it; `in`,
/// `coming` in as the next tick starts, or `refused` while a viewer is near; `kept`, the scene
/// texts it keeps, out, of what it held; `roots`, the streamed roots in it; and `unsaved`, whether
/// `scene`'s `save` would write its file, or is refused it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellAt {
    pub cell: [i32; 3],
    pub file: bool,
    pub made: bool,
    #[serde(rename = "in")]
    pub is_in: bool,
    pub coming: bool,
    pub refused: bool,
    pub kept: usize,
    pub roots: usize,
    pub unsaved: bool,
}

/// What `scene` takes: the action; the file to open or save as, absolute or from the asset root;
/// and, for `open` and `new`, whether to discard unsaved edits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub action: SceneAction,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub discard: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SceneAction {
    Open,
    Save,
    New,
}

/// What `step` takes: the ticks to run, 1 by default and at most 10,000, a frame's worth each
/// frame.
///
/// The simulation stays paused after. `resume` runs it again, dropping the steps not yet run and
/// answering those waiting; `stop` drops them too, failing those waiting with `api.stopped`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default = "one")]
    pub ticks: u64,
}

fn one() -> u64 {
    1
}

/// What `capture` takes: the PNG's `path`, and more.
///
/// `path` is an absolute path in a directory that exists. Without it, the PNG goes in `captures`
/// beside the executable, named for the run, as `sandbox-4242-7.png`; the newest 32 there are kept,
/// older ones removed as each is written, but those a client named stay. With `at` and no `path`,
/// it writes nothing and answers the pixel alone ([`Captured::pixel`]).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// A pixel of the view, `[x, y]` from its top left, as [`Pick::at`] is: the one the position
    /// falls in. Negative or non-finite numbers are refused with `api.bad_params`.
    #[serde(default)]
    pub at: Option<[f64; 2]>,
    /// Whether to capture the view as the game alone shows it: through its own viewpoint, without
    /// the look's outline, grid or colliders, in a frame not published to a view's memory. Without
    /// it, the view as shown, the look among it.
    #[serde(default)]
    pub own: bool,
    /// What to show in place of the view's light ([`Shown`]), over the look's, in a frame not
    /// published to a view's memory; without it, the look's, unless `own`. Captures in a row while
    /// paused show one world.
    #[serde(default)]
    pub show: Option<Shown>,
}

/// What `capture` answers: the PNG's `path`, its size, and the pixel asked for.
///
/// `path` is `None` if not written; the size is the view's in pixels; `pixel`, the bytes of the
/// pixel asked for, if any; and `ticks`, those the simulation had run in the frame it shows (while
/// paused, as the last tick left the world). A capture waits up to 10 s for a frame in which every
/// texture and mesh added before it is on the GPU, rather than their stand-ins; then it takes the
/// next frame all the same, and `streaming` says so.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Captured {
    /// `None` exactly when [`Capture::at`] was given without a `path`.
    #[serde(default)]
    pub path: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    /// The RGBA bytes of the pixel at [`Capture::at`], as the PNG has them (alpha, which the PNG
    /// leaves out, as the view drew it), each read as [`Shown`] says; `None` outside the view, and
    /// without `at`.
    #[serde(default)]
    pub pixel: Option<[u8; 4]>,
    pub ticks: u64,
    /// Whether textures or meshes were still decoding or streaming to the GPU in the frame it
    /// shows, which shows a stand-in for a texture, or nothing for a mesh.
    #[serde(default)]
    pub streaming: bool,
}

/// The most frames a `frames` captures.
pub const MOST_FRAMES: u32 = 1_000;

/// The most bytes of pixels a `frames` holds until its PNGs are written: `count` frames of its
/// `rect`, or of the view as it is when asked, four bytes a pixel.
pub const MOST_FRAMES_BYTES: u64 = 1 << 30;

/// What `frames` takes: how many frames of the view to capture in a row, none skipped.
///
/// They're PNGs in `dir`, an absolute path to a directory that exists, named `frame-0001.png`,
/// `frame-0002.png` and on, over any of those names there. The first is the frame after one comes
/// back with nothing on its way to the GPU, as `capture` waits for, or after 10 s all the same;
/// each frame says whether it showed what was on its way. They show the view as shown, the look
/// among it, or what `show` says in place of its light, from frames not published to a view's
/// memory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frames {
    pub dir: PathBuf,
    /// 1 to [`MOST_FRAMES`].
    pub count: u32,
    /// Each frame's viewpoint in turn, drawn as the look's is, the last held for the frames after,
    /// as a scripted move and a hold; frames while it waits for what's on its way are drawn from
    /// the first. Empty: the view's own, the look's or the game's. Each is checked, its rotation
    /// normalized, as `look` does.
    #[serde(default)]
    pub viewpoints: Vec<Viewpoint>,
    /// The part of each frame to keep, `[x, y, width, height]` in pixels from the view's top left,
    /// cropped to the view as each frame comes back; the whole view without.
    #[serde(default)]
    pub rect: Option<[u32; 4]>,
    /// As [`Capture::show`].
    #[serde(default)]
    pub show: Option<Shown>,
}

/// What `frames` answers: each frame captured, in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Framed {
    pub frames: Vec<FrameShot>,
}

/// A frame `frames` captured: its PNG and size, the ticks run in it, whether textures or meshes
/// were still on their way to the GPU ([`Captured::streaming`]), and its viewpoint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameShot {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub ticks: u64,
    pub streaming: bool,
    pub viewpoint: Viewpoint,
}

/// What `view` takes: the memory to publish the view into, as [`view::Reader::name`] names it, or
/// `None` to stop.
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
/// "pressed":true}}`, a key naming it, holding what it takes; `"cursor_left"` for one taking
/// nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum InputEvent {
    /// A key went down or up, by position, as its W3C UI Events `code` names it, such as `KeyW`, or
    /// winit, which names `MetaLeft` and `MetaRight` `SuperLeft` and `SuperRight`. `repeat` marks a
    /// held key's repeats.
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
    /// The mouse's raw motion, `[x, y]` in its own units, moving no cursor: positive right and
    /// down, for looking about with the cursor grabbed.
    Motion([f64; 2]),
    /// A mouse button went down or up.
    Button { button: Button, pressed: bool },
    /// The wheel or a touchpad scrolled: positive `y` up, and positive `x` left.
    Scroll(Scroll),
    /// The view gained or lost the keys.
    Focused(bool),
}

/// A mouse button: `"left"`, `"right"`, `"middle"`, the side buttons `"back"` and `"forward"`, or
/// another by its number, `{"other":9}`, which differs by platform.
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

/// How far a scroll went: `{"lines":[x, y]}`, a wheel's notches, or `{"pixels":[x, y]}`, a
/// touchpad's.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scroll {
    Lines([f32; 2]),
    Pixels([f64; 2]),
}

/// Where the view is seen from and how.
///
/// From `position`, in `f64`s as the engine's positions are, turned by `rotation`, a unit
/// quaternion, through `projection`. World space is right-handed with +Y up; a viewpoint without
/// rotation looks down -Z, +X to its right and +Y up.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Viewpoint {
    pub position: [f64; 3],
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
        self.position.iter().all(|number| number.is_finite())
            && (self.rotation.iter())
                .chain(&projection)
                .all(|number| number.is_finite())
    }
}

/// How a viewpoint projects onto the view, fitted to its aspect ratio.
///
/// Written `{"Orthographic":{"height":2.0,"near":-1.0,"far":1.0}}` or
/// `{"Perspective":{"fov":1.0472,"near":0.01}}`, as `entity` gives a camera's `projection`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Projection {
    /// Without perspective, as 2D games draw: the view spans `height` world units bottom to top,
    /// showing what's `near` to `far` in front. It projects if `height` is a positive normal
    /// number, and `near` and `far` are finite and differ.
    Orthographic { height: f32, near: f32, far: f32 },
    /// With perspective, as 3D games draw: the view spans `fov` radians bottom to top, showing
    /// what's at least `near` in front, however far. It projects if `fov` is more than 0 and less
    /// than π, and `near` is a positive normal number.
    Perspective { fov: f32, near: f32 },
}

/// An outline round what the view shows of some entities and every entity under each.
///
/// `ids` are 16 hex digits; a model's nodes or a prefab's entities are among those under one. One
/// shape where they touch, in one `color`, linear RGBA, alpha not multiplied in. `width` is the
/// solid band reaching out from their pixels, in view pixels, past 8 drawing 8. A glow, at up to
/// half the band's alpha, fades over as far again past it. No IDs, or IDs no entity has, outline
/// nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outline {
    pub ids: Vec<String>,
    pub color: [f32; 4],
    pub width: f32,
}

/// A world axis plane, by its two axes: `"xy"`, 2D games' plane; `"xz"`, the ground; or `"yz"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Plane {
    Xy,
    Xz,
    Yz,
}

/// What `look` takes: each part of the look left out to keep it, `null` to give it back to the
/// game, or its value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Look {
    /// In place of the game's own; its rotation is normalized.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub viewpoint: Option<Option<Viewpoint>>,
    /// In place of any the game's layers draw.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub outline: Option<Option<Outline>>,
    /// The plane of a grid fitted to the view, as the editor draws one.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub grid: Option<Option<Plane>>,
    /// Whether colliders' outlines show, in place of the game's choice.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub colliders: Option<Option<bool>>,
    /// What shows in place of the light ([`Shown`]).
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub show: Option<Option<Shown>>,
    /// The passes left out ([`Pass`]), in any order, a pass named twice left out once; an empty
    /// list, like `null`, gives the part back to the game.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub skip: Option<Option<Vec<Pass>>>,
    /// The most frames a second an embedded run renders, 1 or more, as a window's frame rate
    /// limit; it renders no more than its client takes either way.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_fps: Option<Option<NonZeroU32>>,
    /// The share of the view's pixels rendered across and down, from 0.25 to 1, in place of the
    /// game's own, as a game's graphics settings set it. Any run takes it, headless ones too; a
    /// scale outside that range, or not a number, is refused.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub render_scale: Option<Option<f32>>,
}

/// A pass of the view the look can leave out, to find which makes what the view shows.
///
/// Named as the world `Environment`'s field that turns it on, or `shadows`, the lights'. The view
/// draws as it would were the environment without it, every frame as valid, while the game, its
/// environment and lights among it, is unchanged, and a capture of the game's own view shows it as
/// it is. A pass the view doesn't run leaves nothing out.
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
    /// offset. Given back, temporal antialiasing starts afresh.
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

/// What the view shows in place of its light, to see how it's made.
///
/// A part of what lit shading takes or makes where models draw, by name, as `"roughness"`, or an
/// image the frame makes, by the debug name RenderDoc and ARCHITECTURE.md's Rendering give it, as
/// `{"image":"view depth"}`. It shows as drawn, without antialiasing's offset or the metered
/// exposure. A part of shading shows nothing after it (no antialiasing, fog, depth of field, motion
/// blur, bloom, grade, lens or sky, black behind what's drawn); what draws without lit shading, as
/// sprites, text and particles, draws as usual. An unlit material shows its base color, and black
/// for the rest, which it doesn't read. An image takes the whole view, alone, from a frame
/// drawn as the game's but for antialiasing, so every pass's images but antialiasing's are there,
/// as `images` lists them.
///
/// A capture PNG's pixel reads as each says; a value of 0 to 1 is its byte over 255 in red, green
/// and blue alike.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Shown {
    /// The base color (texture, vertices and factor), stored as a texture's sRGB colors; a
    /// specular-glossiness material's diffuse color.
    BaseColor,
    /// The normal in the world as shading turns it, normal map included: each axis from -1 to 1 as
    /// its byte over 255, times 2, less 1, X in red, Y in green and Z in blue.
    Normal,
    /// The normal before its normal map, read as [`Shown::Normal`].
    SurfaceNormal,
    /// 0 for a specular-glossiness material.
    Metallic,
    /// As shading takes it: from 0.045, the least it takes, to 1, widened where the normal turns
    /// across the pixel.
    Roughness,
    /// The share of ambient light that reaches it, as shading takes it: its material's occlusion
    /// map's and the view's ambient occlusion's.
    Occlusion,
    /// The light it gives off, read as [`Shown::BaseColor`], past 1 shown as 1.
    Emission,
    /// What it reflects head-on, as the frame's lights take it: a metallic-roughness material's
    /// 0.04 mixed with its base color by how metallic it is; a specular-glossiness material's
    /// specular color. Read as [`Shown::BaseColor`].
    SpecularColor,
    /// The light the frame's lights shade it with, the sun's among them, shadows and all, exposed
    /// and tonemapped as the view's light is.
    DirectLight,
    /// The sky's diffuse and specular light, darkened by occlusion. It and the next three read as
    /// [`Shown::DirectLight`].
    SkyLight,
    /// The view's reflection, which takes the place of as much of the sky's specular light.
    Reflection,
    /// The light it diffuses, the frame's lights' and the sky's: with [`Shown::SpecularLight`], all
    /// the light the view shows of it but its emission.
    DiffuseLight,
    /// The light it reflects: the frame's lights' highlights, the sky's specular light and the
    /// view's reflection.
    SpecularLight,
    /// The view's reflection as its ray found it, before the blur by roughness: the light found
    /// times how far it's trusted, black where it found nothing. Every opaque surface's, whatever
    /// its roughness. Read as [`Shown::DirectLight`].
    TracedReflection,
    /// Why each opaque surface's reflection ray found what it did, or nothing, by color:
    ///
    /// - Red how far behind what's there it passed, green how far it's trusted: a trusted hit green.
    /// - Orange: found where the frame before didn't show, whose light it reads.
    /// - Magenta: found from behind a surface no back face bounds.
    /// - Gray: found nothing opaque.
    /// - Yellow: found its own surface, within 2 pixels.
    /// - Cyan: turned back toward the view.
    /// - Navy: passed its reach. Blue: left the view. White: took its last step.
    /// - Black: nothing opaque, or the view has no reflections.
    ReflectionRays,
    /// The view's light as paths traced through its opaque and masked models find it,
    /// [`PATH_SAMPLES`] a pixel: a reference for the light the view shades, read as
    /// [`Shown::DirectLight`]. It leaves out fog, haze, what blends, skinned meshes and games' own
    /// shaders. The view's look grows less noisy frame by frame. A capture waits until every pixel
    /// has taken them, at most 25 s, then fails with `api.path_traced`, saying how many each took.
    /// Black where the GPU has no ray queries, as KosmicKrisp hasn't, warning
    /// `renderer.no_ray_query`.
    PathTraced,
    /// An image the frame makes, by debug name, read by its kind: colors as they are, high dynamic
    /// range tonemapped as the view's light is; depth as `1 + log2(depth) / 24`, white at the near
    /// plane and a 24th darker each time the distance doubles, black where nothing drew; IDs and a
    /// visibility buffer a color per value, black for 0; motion, in world units, and normals read as
    /// [`Shown::Normal`], -1 to 1; one float a pixel as its value. An image the frame doesn't make shows black, warning
    /// `renderer.no_image`.
    Image(String),
}

/// The most bytes a [`Shown::Image`]'s name has.
pub const MOST_IMAGE_NAME: usize = 64;

/// The paths a pixel takes when the view shows [`Shown::PathTraced`].
pub const PATH_SAMPLES: u32 = 1_024;

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
    /// The passes left out, in [`Pass::ALL`]'s order, each once, never empty; `None` from an engine
    /// older than passes left out.
    #[serde(default)]
    pub skip: Option<Vec<Pass>>,
    /// `None` from an engine older than the cap, too.
    #[serde(default)]
    pub max_fps: Option<NonZeroU32>,
    /// `None` from an engine older than the render scale, too.
    #[serde(default)]
    pub render_scale: Option<f32>,
    pub by: By,
    /// How many times the look has changed in the run, which a shared frame gives as the revision
    /// it drew with ([`view::Frame::look`]). A `look` that changes no part's value or setter
    /// doesn't count.
    pub revision: u64,
    /// The run's number, as `logs` gives it, so that a client tells a restarted game, whose look
    /// starts afresh, from an unchanged look.
    pub run: u64,
    /// The viewpoint the last frame handed to the renderer drew from, the look's or the game's;
    /// `None` before the first, and while it has a number that isn't finite, which JSON can't
    /// carry. A `look` setting a viewpoint answers before a frame draws with it.
    pub seen: Option<Viewpoint>,
}

/// Who set each part of the look, by the name it said hello with; `None` where it's the game's
/// own.
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
    #[serde(default)]
    pub max_fps: Option<String>,
    #[serde(default)]
    pub render_scale: Option<String>,
}

/// What `pick` takes: the view's pixel `at`, `[x, y]` from its top left, as `input`'s cursor and a
/// capture's PNG have it.
///
/// `within`, an entity's ID or name, as an editor's pick: where it's the root of the model drawn at
/// the pixel, or one of its nodes, the answer is the node drawn there rather than the root. A `within`
/// of no living entity is as none.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pick {
    pub at: [f64; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<String>,
}

/// What `pick` answers: the entity the view shows at the pixel, by ID, and its name as [`Listed`]
/// has it when answered.
///
/// For a model's node it's the model's root, which tools change in its place, unless
/// [`Pick::within`] names that root or one of its nodes. Both are `None` outside the view or where
/// no entity draws. The name alone is `None` for an entity gone by the answer, whose ID is kept,
/// and for a connection without `read` of `Name`, which `view` alone isn't.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picked {
    pub id: Option<String>,
    pub name: Option<String>,
}

/// What `images` answers: the images [`Shown::Image`] names that the reading frame made, in its
/// order, each name once.
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
    /// What it holds, which says how a show reads it.
    pub kind: ImageKind,
    /// Its `[width, height]` in pixels, which a show stretches over the view.
    pub size: [u32; 2],
}

/// What an image a frame makes holds, as [`Shown::Image`] reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageKind {
    Color,
    Hdr,
    Ids,
    Motion,
    Visibility,
    Depth,
    Scalar,
    Normals,
}

/// What `entities` takes: those with component `with`, or all, in the order queries find them,
/// skipping `skip` and taking at most `take`, 100 by default.
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

/// What `entities` answers: how many in all, the page, and the types' revision, as `types` answers
/// it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityList {
    pub total: usize,
    pub entities: Vec<Listed>,
    /// 0 from an engine older than revisions of the types.
    #[serde(default)]
    pub types: u64,
}

/// An entity as `entities` lists it.
///
/// Its ID; its name, its `Name`'s, or else for a model's node the node's name in the model's file;
/// its parent's ID; its children's, in order; and its components' types, in registration order.
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

/// What `entity` takes: an entity's ID or name.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entity {
    pub id: String,
}

/// A sphere in world space, as `entity` answers `bounds`, in world units.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sphere {
    pub center: [f64; 3],
    pub radius: f32,
}

/// What `entity` answers under `instance`, for an instance's entity, its root or one its prefab
/// gives:
///
/// - `root`, `prefab`: the root of the outermost instance whose prefab gives it, or its own if none
///   does, and that instance's prefab by path from the asset root;
/// - `link`: likewise its own prefab, if it's an instance's root, nested or not; only an outermost
///   instance's root unlinks;
/// - `overrides`: each component the prefab gives it that differs, by type name, with the differing
///   fields (a struct of named fields' by name, as a scene saves them, else `""` for the whole
///   component), each a path `revert` takes;
/// - `added`: the components the prefab doesn't give it, which a scene saves whole, but the
///   instance's own: a root's `Transform` and `PrefabRoot`, and a `Parent`.
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

/// What `save_prefab` takes: the entity `id` (an ID or name) to save with its descendants, and the
/// prefab's `path`, if any.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavePrefab {
    pub id: String,
    #[serde(default)]
    pub path: Option<PathBuf>,
}

/// What `save_prefab` answers: the prefab's path from the asset root, as [`Status::scene`] names
/// files; its name, its asset ID or else its path, as values name it; and the ticks run as the link
/// applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPrefab {
    pub path: String,
    pub name: String,
    pub ticks: u64,
}

/// What `get` takes: an entity's ID or name, a component type, and a field's path, such as
/// `translation.x` or `rect[1]`, empty for the whole component.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub id: String,
    pub component: String,
    #[serde(default)]
    pub path: String,
}

/// What `types` answers: the component types, the named types they hold, and their revision.
///
/// `components` are in registration order; `types`, the named types, structs and enums, that they
/// are and hold, in description order, each name unique; `revision`, which each change to them
/// changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Types {
    pub components: Vec<ComponentInfo>,
    pub types: Vec<TypeInfo>,
    /// 0 from an engine older than revisions of the types.
    #[serde(default)]
    pub revision: u64,
}

/// A component type: its name, as its description has it, and the value an `add` without one gives
/// it, or why there's none, as when it holds an entity, which has no default.
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
/// variants; or, `"opaque"`, nothing values give or take but `(opaque)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Struct { fields: Vec<FieldInfo>, tuple: bool },
    Enum(Vec<VariantInfo>),
    Opaque,
}

/// A struct's or variant's field.
///
/// Its name, which paths use; its type; the least and most its numbers, vectors' components and
/// elements may be; how the Inspector shows it; its doc comment; and its own default, if its
/// description gives one, which may be `null`, as an option's. Without one, its type's applies, as
/// gg-engine's `Types::field_default` makes it.
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

/// An enum's variant, its fields a tuple's if `tuple`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VariantInfo {
    pub name: String,
    pub fields: Vec<FieldInfo>,
    pub tuple: bool,
}

/// A field's type.
///
/// `"bool"`, `{"int":{"bits":32,"signed":true}}`, `"f32"`, `"f64"`, `"string"`, `"vec2"`, `"vec3"`,
/// `"vec4"`, `"dvec3"` (three `f64`s, as an entity's translation), `"quat"`, `"entity"`,
/// `"texture"`, `"model"`, `"font"`, `"prefab"`, `{"option":...}`, `{"list":...}`, `{"array":[...,
/// 4]}`, `{"named":"Transform"}`, a type `types` describes, or `"opaque"`, a field reflection
/// leaves as it is.
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
    Dvec3,
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

/// What `hash` answers: the world's content and layout hashes, 16 hex digits each, as recordings
/// write them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hash {
    pub content: String,
    pub layout: String,
}

/// What `assets` takes: the assets of `kind`, or all, in path order, skipping `skip` and taking at
/// most `take`, 100 by default.
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

/// What `assets` answers: how many of the kind in all, the page, and every ID file of the kind
/// whose asset isn't there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetList {
    pub total: usize,
    pub assets: Vec<ListedAsset>,
    pub orphans: Vec<ListedOrphan>,
}

/// An asset as `assets` lists it.
///
/// Its ID, 16 hex digits, or `None` while it has none; its path from the asset root, as
/// [`Status::scene`] names files, spelled as on disk; its kind; whether a layer has added it, by
/// any path; and its `problem`, what's wrong with its ID file: why it has no ID, or why its ID
/// isn't the one its file holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedAsset {
    pub id: Option<String>,
    pub path: String,
    pub kind: asset::AssetKind,
    pub loaded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<Failure>,
}

/// An ID file whose asset isn't there, as after a rename that left it behind: its path from the
/// asset root, its asset's kind, and the ID it holds, `None` if it doesn't read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedOrphan {
    pub path: String,
    pub kind: asset::AssetKind,
    pub id: Option<String>,
}

/// What `ui` answers: the view's size in pixels, which nodes' `pixels` are in, as the last frame
/// handed it off, or 1,920 by 1,080 before one has; and each UI the layers added, in that order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiList {
    pub view: [u32; 2],
    pub uis: Vec<ListedUi>,
}

/// A UI as `ui` lists it.
///
/// The name its layer gave it; its file's name, as values name a texture; its file's path from the
/// asset root, as [`Status::scene`] names files; whether the last frame handed off drew it; whether
/// it takes input; the focused button's name; and its nodes, in file order, each after its parent.
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

/// A UI's node as `ui` lists it:
///
/// - `name`, if its file gives one; its `kind`; its `parent`'s place among the nodes, `None` at
///   the top;
/// - its rectangle, `[x, y, width, height]` from the view's top left, +Y down: `rect` in canvas
///   units, 1,080 to the view's height, and `pixels` in view pixels as a capture's PNG has them, a
///   scrolled list's children where it scrolled them;
/// - `visible`, it and each node above it, a hidden one's rectangle of no size;
/// - `text`, what a text, a button or a toggle says, what's typed in a text field, or a drop-down's
///   choice; `value`, what a slider, a toggle or a drop-down holds;
/// - for a button or a widget, whether it's `disabled`, `focused`, or held `pressed`, a drop-down
///   while its options are open.
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

/// A UI node's kind, as its file names it.
///
/// A panel, a background behind the children it stacks; a text; an image; a button; a widget
/// (slider, toggle, drop-down or text field); or a container laying its children out in a row, a
/// column, a stack, or a list, a column that clips and scrolls them.
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

/// What a UI widget holds: a slider's number, whether a toggle is on, or a drop-down's choice by
/// its place among the options, from 0; as `{"number": 0.5}`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiValue {
    Number(f32),
    On(bool),
    Choice(usize),
}

/// The methods that change nothing, which a client may send again whether or not the first was
/// taken.
const READS: [&str; 14] = [
    "hello", "ping", "status", "entities", "entity", "get", "types", "hash", "assets", "ui",
    "edits", "logs", "pick", "cells",
];

/// Whether `method` may change the game, its world, run or files.
///
/// All but the reads may, an unknown one too. An unanswered request of it may have applied, so it's
/// sent again only once a read says it hasn't.
pub fn changes(method: &str) -> bool {
    !READS.contains(&method)
}

/// What an unanswered `method`'s error adds ([`Client::call`]): that it may have applied, if it
/// [`changes`] the game.
pub fn unanswered(method: &str) -> &'static str {
    if changes(method) {
        "; the game may have taken it, and it may yet apply, or have applied: read what it changes \
         before sending it again"
    } else {
        ""
    }
}

/// What `snapshot` takes: a `path`, and whether to `quit`.
///
/// `path` is an absolute path in a directory that exists. Without it, the file goes in `snapshots`
/// beside the executable, named for the run, as `sandbox-4242-7.snapshot`; the newest 8 there are
/// kept, older ones removed as each is written, but those a client named stay, as `gg run`'s, in a
/// directory of its own. With `quit`, the run ends cleanly, as `quit` ends it, once the snapshot is
/// written and answered, nothing applying between, so that a run started again from it loses no
/// edit answered before.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quit: bool,
}

/// What `snapshot` answers: its path, the ticks run as it was taken, which a restored run goes on
/// from, its entities, and its size in bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshotted {
    pub path: PathBuf,
    pub ticks: u64,
    pub entities: usize,
    pub bytes: u64,
}

/// What `patch` takes: the jump table `dx tools hotpatch` printed, as printed, which names the
/// patch's library and maps each function it replaces to its new address.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub table: Value,
}

/// What `patch` answers: the ticks run as the patch applied; the next is the first to run the
/// patched code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patched {
    pub ticks: u64,
}

/// An authoring change to the world, as `apply` takes it.
///
/// Written `{"set":{"id":...,"component":...,"path":...,"value":...}}`, a key naming it, holding an
/// object of what it takes. It names entities by ID or name ([Entities](crate#entities)), and
/// carries values as [Values](crate#values) say.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Spawns an entity with `components`, by type name, each its whole value, as the last of
    /// `parent`'s children, or a root. Its ID is `id`, or one drawn, which [`Applied`] gives.
    Create {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        components: serde_json::Map<String, Value>,
    },
    /// Spawns a copy of entity `id` and its descendants, the copy the last of its parent's
    /// children: components as they are, references among them pointing to the copies, each
    /// exclusive field (as a camera's `primary`) false. IDs are `copies`, `id`'s copy's first, each
    /// parent's before its children's, or drawn ones, which [`Applied`] gives.
    Duplicate {
        id: String,
        #[serde(default)]
        copies: Vec<String>,
    },
    /// Despawns entity `id` and its descendants.
    Delete { id: String },
    /// Gives entity `id` a `component` it hasn't: `value`, or the type's default.
    Add {
        id: String,
        component: String,
        #[serde(default)]
        value: Option<Value>,
    },
    /// Takes entity `id`'s `component` away.
    Remove { id: String, component: String },
    /// Sets the field at `path`, such as `translation.x`, of entity `id`'s `component` to `value`;
    /// the whole component for an empty path. An exclusive field set true is made false on every
    /// other entity with the component.
    Set {
        id: String,
        component: String,
        #[serde(default)]
        path: String,
        value: Value,
    },
    /// Makes entity `id` the last of `parent`'s children, or a root for `null`, keeping its
    /// `Transform`, so it moves with its new parent. With `in_place` it keeps its place in the
    /// world, as an editor's reparent does: the same edit sets its transform to its place in the
    /// new parent's space as the request finds the two, losing any shear between them.
    Reparent {
        id: String,
        /// Required, `null` for a root, so that one left out isn't taken for a root.
        #[serde(deserialize_with = "Option::deserialize")]
        parent: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        in_place: bool,
    },
    /// Unlinks the prefab instance rooted at entity `id`, taking its `PrefabRoot` away; its
    /// entities stay as they are, under their IDs, its own from then on.
    Unlink { id: String },
    /// Sets the field at `path` of entity `id`'s `component`, or the whole component for an empty
    /// path, to what the instance's prefab has there: for an instance's root, all but its own
    /// `Transform`; or for an entity an instance's prefab gives.
    Revert {
        id: String,
        component: String,
        #[serde(default)]
        path: String,
    },
}

/// What `apply` takes: an edit's operations, applied in order, and a `gesture`.
///
/// `gesture` numbers a client's gesture, such as a drag or a number typed, the same for all its
/// sets. A set of the field the client's last edit set, in the same gesture, joins that edit,
/// taking its value and keeping what undoes it, so the gesture undoes whole, of one entity or
/// several. Any other edit of the client's that reaches the world ends the gesture, refused there
/// or not; one refused as it's read (a bad ID, an unknown type, a value of another kind) doesn't. A
/// client's edits are those of every connection of its name, so a gesture's number must be unique
/// across them, as one counted from a random start is. With a gesture an apply is only sets, each
/// of one component's field, no two of one entity, or it's refused with `api.bad_params`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Apply {
    pub operations: Vec<Operation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gesture: Option<u64>,
}

/// What `apply` answers: the ticks run as the edit applied, the next the first after it, and the
/// IDs its creates and duplicates spawned, in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    pub ticks: u64,
    pub entities: Vec<String>,
}

/// What `undo` and `redo` take: whose last edit, by author name; the client's own without one.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Undo {
    #[serde(default)]
    pub author: Option<String>,
}

/// An edit, as `undo`, `redo` and `edits` give it: its author and operations in words, such as
/// ``set `Spin.speed` of 2b7e151628aed2a6``.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub author: String,
    pub operations: Vec<String>,
}

/// What `edits` takes: whose edits, by author name; every author's without one.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edits {
    #[serde(default)]
    pub author: Option<String>,
}

/// What `edits` answers: each author's log.
///
/// In the order the authors first edited, of the world the game runs since a world last took its
/// place, as a restore does. `play` starts the logs afresh for the copy, and `stop` brings back the
/// world as edited's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    pub authors: Vec<Authored>,
}

/// An author's log: the operations in words of each edit `undo` steps back through, the first
/// applied first, so `undo` undoes the last; and of each `redo` steps through, the last undone
/// last.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authored {
    pub author: String,
    pub done: Vec<Vec<String>>,
    pub undone: Vec<Vec<String>>,
}

/// The names `logs`' `level` takes, the most severe first.
pub const LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

/// What `logs` takes: where to read from, how many, how severe, and how long to wait.
///
/// `from` is the event to read from, an earlier answer's `next`, with its `run`; without it, the
/// `last` events, or else all, from the oldest kept. `level` is the least severe to read, one of
/// [`LEVELS`], or all the game's filter let through; `wait_ms`, how long to wait for an event if
/// none is there, in milliseconds, 0 by default and at most 10,000.
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

/// What `logs` answers: the events read, and where to read on.
///
/// `run`, drawn at random as the API started, for the next `logs` to give with `next`, the event to
/// read from then; `end`, the next event's number; `missed`, how many events of any level it
/// skipped between where it read and `next`, dropped for newer ones or too long to keep, as a line
/// over 1 MiB is; and `events`, in order, each the JSON log's line as written, without its line
/// break.
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
/// errors as rustc renders them, in a log line the game keeps, which may have 1 MiB.
pub const MOST_NOTE: usize = 64 << 10;

/// What `note` takes: an event's code, level, message and fields.
///
/// The code is a stable code, as the API's errors have, such as `gg.patch_failed`; the message says
/// what happened, naming what it's about; the fields, if any, are a JSON object, which the game
/// logs as one field, `fields`, its JSON as a string. The game logs it with the `hello`'s client
/// name as `client`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Note {
    pub code: String,
    pub level: NoteLevel,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Map<String, Value>>,
}

/// A note's level: `"info"` or `"warn"`, never an error, as a headless run's status counts the
/// run's own errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteLevel {
    Info,
    Warn,
}

/// What `debug` takes, each part optional, done in this order.
///
/// The breakpoints to set in a script, in place of its own; an `action`; and `wait_ms`, how long to
/// wait for a script to stop if none has, in milliseconds, 0 by default and at most 10,000. `{}`
/// answers the debugger as it is.
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

/// A script's breakpoints: its path from the project's root, as [`Debugging::scripts`] names it,
/// and their lines, from 1; none clears them. A script no layer has loaded keeps them until one
/// does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Breakpoints {
    pub script: String,
    pub lines: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugAction {
    /// Stops at the next line a script runs, breaking into a running one, such as an endless loop;
    /// nothing while one has stopped.
    Pause,
    /// From a stop, goes on to the next.
    Continue,
    /// From a stop, stops at the next line that runs, in a function it calls too.
    StepInto,
    /// From a stop, stops at the stopped function's next line, past its calls but for a breakpoint
    /// in one, or once it returns, at its caller's.
    StepOver,
    /// From a stop, stops at the caller's next line once the stopped function returns.
    StepOut,
}

/// What `debug` answers: where a script has stopped, and the debugger's state.
///
/// `stopped` is where one has, while one has; `asked`, the pause or step under way that a stop to
/// come answers; `stops`, the run's stops so far, which number them; `breakpoints`, in order of
/// script path, then line asked; and `scripts`, those the layers have loaded, by path from the
/// project's root, in order.
///
/// A step out of a call into a script, from a tick or a chunk loading, or over its last line, stops
/// at the next line a script runs, such as the next entity's tick. A call steps only if it started
/// while the debugger was armed, by a breakpoint placed or a pause or step asked: a step from where
/// a pause broke into another call stops at the next line a script runs after it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Debugging {
    pub stopped: Option<Stop>,
    pub asked: Option<DebugAction>,
    pub stops: u64,
    pub breakpoints: Vec<Breakpoint>,
    pub scripts: Vec<String>,
}

/// A breakpoint: its script, the line asked, and the line Luau placed it at.
///
/// That is the next with code from the line asked; `None` while its script hasn't loaded, or has no
/// code there or after. It's placed once its script's chunk has run, so none stops the chunk as it
/// loads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakpoint {
    pub script: String,
    pub line: u32,
    pub placed: Option<u32>,
}

/// Where a script has stopped: the stop's number, from 1; why; and the stack, the stopped function
/// first, then its caller, and so on to the call into the script.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub number: u64,
    pub why: StopReason,
    pub frames: Vec<StackFrame>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StopReason {
    Breakpoint,
    Step,
    Pause,
}

/// A function on a stopped script's stack: its name, `(chunk)` for a chunk's top level and
/// `(anonymous)` for one without; its script's path; its line; and its locals and upvalues there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StackFrame {
    pub function: String,
    pub script: String,
    pub line: u32,
    pub locals: Vec<Variable>,
    pub upvalues: Vec<Variable>,
}

/// A local or an upvalue, its value as JSON.
///
/// `null` for `nil`; a number or a string as its kind is, the string cut to [`MOST_SHOWN`] bytes; a
/// vector `[x, y, z, w]`; an entity its ID, or `null` for `gg.entity` and one not alive; anything
/// else its kind in parentheses, such as `(table)`, `(function)`, or `(handle)` for a field's
/// handle. A float that isn't finite is `inf`, `-inf` or `NaN`, as a value's is.
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
    /// The next request's ID.
    next: u64,
    welcome: Welcome,
    /// Set once an answer was late or the connection broke, after which a line may be cut short:
    /// every call fails then.
    broken: bool,
}

impl Client {
    /// Connects to the game `session` names and says hello as client `name`, the author of its
    /// edits, waiting at most `timeout` to connect and for each answer.
    ///
    /// Fails with `api.connect` if nothing listens there, as once the game has ended;
    /// `api.unauthorized` if the session's token isn't the game's; and as [`call`](Self::call)
    /// does.
    pub fn connect(session: &Session, name: &str, timeout: Duration) -> Result<Self, Failure> {
        Self::connect_granted(session, name, None, timeout)
    }

    /// As [`connect`](Self::connect), kept to the capabilities `grant` names ([`Hello::grant`]), or
    /// every one for `None`.
    ///
    /// Fails as `connect` does, and with `grant.unknown` for an unknown capability.
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
    /// its result's JSON as the game wrote it.
    ///
    /// Fails with the game's error; `api.timeout` if its answer is late; `api.closed` if the
    /// connection closes first, as when the game ends; and `api.protocol` for an answer that isn't
    /// one. After `api.timeout` or `api.closed`, every call fails with `api.closed`. An unanswered
    /// or late request the game took may still apply: either's message says so of a method that
    /// [`changes`] the game.
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

/// Runs `body` with `call` on one world: `status` first, then `pause` if the game plays, neither
/// paused nor stopped, then `body`, and `resume` if and only if it paused it.
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
            protocol: Some(PROTOCOL),
        };
        for pid in [2, 1] {
            let text = serde_json::to_string(&session(pid)).unwrap();
            fs::write(dir.join(Session::file_name("Sandbox", pid)), text).unwrap();
        }
        // Being written, beside its own; and something else.
        let whole = serde_json::to_string(&session(3)).unwrap();
        fs::write(dir.join("sandbox-3.json.tmp"), whole).unwrap();
        fs::write(dir.join("sandbox-4.json"), "{\"app\":").unwrap();
        // Of an engine older than projects and the protocol's revisions.
        let mut older = serde_json::to_value(session(5)).unwrap();
        older.as_object_mut().unwrap().remove("project");
        older.as_object_mut().unwrap().remove("protocol");
        fs::write(dir.join("sandbox-5.json"), older.to_string()).unwrap();
        let listed = Session::list(&dir);
        let pids: Vec<_> = listed.iter().map(|(_, session)| session.pid).collect();
        assert_eq!(pids, [1, 2, 5]);
        assert_eq!(listed[0].1, session(1));
        assert_eq!((&listed[2].1.project, listed[2].1.protocol), (&None, None));
        assert!(Session::list(&dir.join("none")).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Operations are one-key objects, defaults left out; a reparent names its parent, `null` for a
    /// root.
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

    /// A `null` default is a default, unlike none; bounds keep integers apart from floats, a
    /// `u64`'s widest too.
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

    /// `logs` takes only what its docs say, defaulting the rest; its answer keeps each event as the
    /// log wrote it.
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

    /// A part left out is kept, `null` gives it back to the game, a value sets it; a projection is
    /// written as reflection writes a camera's.
    #[test]
    fn looks_read_as_their_docs_say() {
        let json = r#"{"viewpoint": {"position": [1000000.3, 1, 2.5], "rotation": [0, 0, 0, 1],
            "projection": {"Perspective": {"fov": 1.0, "near": 0.01}}}, "outline": null,
            "grid": "xz"}"#;
        let look: Look = serde_json::from_str(json).unwrap();
        let viewpoint = Viewpoint {
            position: [1_000_000.3, 1.0, 2.5],
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
            max_fps: None,
            render_scale: None,
        };
        assert_eq!(look, expected);
        let written = serde_json::to_value(&look).unwrap();
        assert_eq!(written.get("outline"), Some(&Value::Null), "given back");
        assert_eq!(written.get("colliders"), None, "kept");
        let capped: Look = serde_json::from_str(r#"{"max_fps": 30}"#).unwrap();
        assert_eq!(capped.max_fps, Some(NonZeroU32::new(30)));
        let uncapped: Look = serde_json::from_str(r#"{"max_fps": null}"#).unwrap();
        assert_eq!(uncapped.max_fps, Some(None), "given back");
        let scaled: Look = serde_json::from_str(r#"{"render_scale": 0.5}"#).unwrap();
        assert_eq!(scaled.render_scale, Some(Some(0.5)));
        let own: Look = serde_json::from_str(r#"{"render_scale": null}"#).unwrap();
        assert_eq!(own.render_scale, Some(None), "given back");
        let text = serde_json::to_string(&own).unwrap();
        assert_eq!(text, r#"{"render_scale":null}"#);
        let text = serde_json::to_string(&scaled).unwrap();
        assert_eq!(text, r#"{"render_scale":0.5}"#);
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
            (r#""traced_reflection""#, Shown::TracedReflection),
            (r#""reflection_rays""#, Shown::ReflectionRays),
            (r#""path_traced""#, Shown::PathTraced),
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
            r#"{"max_fps": 0}"#,
            r#"{"max_fps": -1}"#,
            r#"{"max_fps": 2.5}"#,
            r#"{"render_scale": "half"}"#,
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
            ImageKind::Normals,
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
            "normals",
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
            // From an engine older than what the view shows, the passes it leaves out and the cap,
            // which names none of them.
            show: None,
            skip: None,
            max_fps: None,
            render_scale: None,
            by: By {
                viewpoint: None,
                outline: Some("editor".to_owned()),
                grid: Some("gg".to_owned()),
                colliders: None,
                show: None,
                skip: None,
                max_fps: None,
                render_scale: None,
            },
            revision: 3,
            run: 7,
            seen: Some(seen),
        };
        assert_eq!(looked, expected);
        let shown = Looked {
            show: Some(Shown::Roughness),
            skip: Some(vec![Pass::Shadows, Pass::Adaptation]),
            max_fps: NonZeroU32::new(30),
            render_scale: Some(0.5),
            by: By {
                show: Some("gg".to_owned()),
                skip: Some("agent".to_owned()),
                max_fps: Some("editor".to_owned()),
                render_scale: Some("editor".to_owned()),
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
        let sphere = r#"{"center": [1000000.3, 2.5, -3], "radius": 0.5}"#;
        let expected = Sphere {
            center: [1_000_000.3, 2.5, -3.0],
            radius: 0.5,
        };
        assert_eq!(serde_json::from_str::<Sphere>(sphere).unwrap(), expected);
        let written = serde_json::to_string(&expected).unwrap();
        assert_eq!(written, r#"{"center":[1000000.3,2.5,-3.0],"radius":0.5}"#);
        let extra = r#"{"center": [0, 0, 0], "radius": 1, "color": 0}"#;
        assert!(serde_json::from_str::<Sphere>(extra).is_err());
    }

    /// A pick names a pixel and answers an entity or none; a capture is of the view as shown unless
    /// it asks for the game's own.
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

    /// A capture's answer has a path unless it asked for a pixel alone, and the pixel's bytes when
    /// asked; an older engine's has a path, no pixel.
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

    /// Frames need a directory and a count, and take viewpoints, a rect and a show; each answered
    /// frame names its PNG and viewpoint.
    #[test]
    fn frames_read_as_their_docs_say() {
        let frames: Frames = serde_json::from_str(r#"{"dir": "/c/f", "count": 3}"#).unwrap();
        assert_eq!(
            (frames.viewpoints.len(), frames.rect, frames.show),
            (0, None, None)
        );
        let given = r#"{"dir": "/c/f", "count": 2, "rect": [1, 2, 3, 4], "show": "normal",
            "viewpoints": [{"position": [0, 1, 2], "rotation": [0, 0, 0, 1],
            "projection": {"Perspective": {"fov": 1.0, "near": 0.1}}}]}"#;
        let frames: Frames = serde_json::from_str(given).unwrap();
        assert_eq!(frames.viewpoints[0].position, [0.0, 1.0, 2.0]);
        assert_eq!(
            (frames.rect, frames.show),
            (Some([1, 2, 3, 4]), Some(Shown::Normal))
        );
        for refused in [
            r#"{"count": 3}"#,
            r#"{"dir": "/c/f"}"#,
            r#"{"dir": "/c/f", "count": 3, "own": true}"#,
            r#"{"dir": "/c/f", "count": 3, "rect": [1, 2, 3]}"#,
        ] {
            assert!(
                serde_json::from_str::<Frames>(refused).is_err(),
                "{refused}"
            );
        }
        let framed = Framed {
            frames: vec![FrameShot {
                path: PathBuf::from("/c/f/frame-0001.png"),
                width: 4,
                height: 2,
                ticks: 7,
                streaming: false,
                viewpoint: frames.viewpoints[0],
            }],
        };
        let text = serde_json::to_string(&framed).unwrap();
        assert_eq!(serde_json::from_str::<Framed>(&text).unwrap(), framed);
    }

    /// A scene request names the action, with a path and `discard` when given; an older engine's
    /// status names no scene and has nothing unsaved.
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
            (status.scene, status.unsaved, status.edited, status.cells),
            (None, false, false, None)
        );
        let streaming = r#"{"ticks": 1, "paused": false, "frames": 2, "entities": 3, "view": null,
            "cells": {"in": 4, "kept": 2}}"#;
        let status: Status = serde_json::from_str(streaming).unwrap();
        let cells = CellCounts {
            cells_in: 4,
            kept: 2,
        };
        assert_eq!(status.cells, Some(cells));
        let written = serde_json::to_value(cells).unwrap();
        assert_eq!(written, serde_json::json!({"in": 4, "kept": 2}));
    }

    /// A world's cells read as `cells` writes them: `in` for a cell in, the look's viewpoint a
    /// viewer of no entity; no cells as `null`.
    #[test]
    fn cell_lists_read_as_their_docs_say() {
        let json = r#"{"world": {"entity": "00000000000000a0", "folder": "scenes/cells",
            "size": 64, "viewers": [{"entity": null, "position": [0, 10, 0], "radius": 512}],
            "cells": [{"cell": [3, 0, -2], "file": true, "made": false, "in": true, "coming": false,
            "refused": false, "kept": 0, "roots": 2, "unsaved": true}]}}"#;
        let list: CellList = serde_json::from_str(json).unwrap();
        let world = list.world.as_ref().unwrap();
        let cell = CellAt {
            cell: [3, 0, -2],
            file: true,
            made: false,
            is_in: true,
            coming: false,
            refused: false,
            kept: 0,
            roots: 2,
            unsaved: true,
        };
        assert_eq!(world.cells, [cell]);
        assert_eq!(
            (world.viewers[0].entity.as_deref(), world.size),
            (None, 64.0)
        );
        let written = serde_json::to_value(&list).unwrap();
        assert_eq!(serde_json::from_value::<CellList>(written).unwrap(), list);
        let none: CellList = serde_json::from_str(r#"{"world": null}"#).unwrap();
        assert_eq!(none, CellList::default());
    }

    /// `assets` takes a kind or none and a page, defaulting each; its answer gives kinds in
    /// lowercase and a `problem` only where there is one.
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
            protocol: Some(PROTOCOL),
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

    /// A read's error doesn't say so.
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
