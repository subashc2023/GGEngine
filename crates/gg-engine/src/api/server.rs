//! The API's server: it listens on the machine's loopback, at a port the OS picks, which the session
//! file in the `sessions` directory beside the executable gives with a token. A thread accepts
//! connections, and each has a thread that reads its requests, at most [`gg_api::MOST_IN_FLIGHT`]
//! ahead of the answers written, answering `hello`, `ping`, `logs` and `note` itself, `logs` from
//! the events the log keeps, passing the rest on to the main loop, and, once its `hello` is granted,
//! one that writes its answers. The main loop answers those that came before a frame started as it
//! starts, before its ticks, so that a `patch` applies between ticks, a `play`, a `stop` or a
//! `scene` holds from the frame's ticks, and a `look` reaches the frame; a `step` once its ticks
//! have run; a `capture` once a frame that rendered brings its view back, whose PNG a thread of its
//! own writes; a `pick` once the frame that read it brings its answer back; and a `snapshot` once a
//! thread of its own has written the copy of the world the main thread took.
//!
//! A capture of the game's own view rides with a bare frame, which draws without the look, as the
//! game would on its own; isn't published to a view's memory; and leaves the picks to the next
//! frame that isn't bare, so that no pick reads a view a client isn't shown. A windowed game shows
//! its bare frames in its own window, for a frame or two.

use std::cell::RefCell;
use std::fmt::Display;
use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::mem;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use gg_api::asset::AssetKind;
use gg_api::view::{Pacer, Writer};
use gg_api::{
    Applied, Apply, AssetList, Authored, Button, By, Capture, Captured, CellCounts, EntityList,
    Failure, Field, FrameShot, Framed, Hash, Hello, History, ImageList, Input, InputEvent, Listed,
    ListedAsset, ListedOrphan, Looked, Patched, Picked, Pulse, Request, SavePrefab, SavedPrefab,
    Scene, SceneAction, Session, Snapshotted, Status, Step, View, Welcome,
};
use gg_core::log::Events;
use gg_core::math::{DMat4, DVec3, Quat, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{MAX_PICKS, Meshes, Projection, Viewpoint};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use subsecond::JumpTable;
use tracing::{Level, debug, info, warn};

use super::{Shown, Waiting, json};
use crate::assets::Asset;
use crate::bounds;
use crate::edits::{Done, Edit};
use crate::grant::{Capability, Grant};
use crate::grid::Plane;
use crate::layer::{Offered, Outlining, Stack};
use crate::luau::Debugger;
use crate::main_loop::Stepper;
use crate::net::Ticket;
use crate::reflect::{self, Ident, Type};
use crate::render_thread::{Extracted, Publish, Shot};
use crate::scene_files::{self, Open, SceneFile};
use crate::snapshot::Run;
use crate::{
    Context, Entity, EntityId, Event, Font, Key, Model, MouseButton, Operation, Pose, Prefab,
    PrefabRoot, Scroll, Texture, World, patch, snapshot,
};

/// The methods, as `api.unknown_method` lists them.
const METHODS: [&str; 35] = [
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

/// The most of the clients' picks a frame asks: half as many as a frame reads back, so that the
/// clients leave the game's layers the rest.
const MOST_PICKS: usize = MAX_PICKS / 2;

/// The client a connection whose `hello` names none is, the author of its edits.
const UNNAMED: &str = "api";

/// The most ticks a `step` runs.
const MOST_STEPS: u64 = 10_000;

/// The most entities or assets a page of `entities` or `assets` lists.
const MOST_LISTED: usize = 10_000;

/// How long a connection has to say hello, its whole `hello` come, before it's dropped.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// The most connections saying hello at once, past which more are refused as they come: any local
/// process may open connections that never say it, each holding a thread until it's dropped.
const MOST_SAYING_HELLO: usize = 8;

/// The longest a `logs` waits for an event to come, or a `debug` for a script to stop, in
/// milliseconds.
const MOST_WAIT_MS: u64 = 10_000;

/// The longest a `capture` waits for a frame that shows every texture and mesh added before it,
/// none still decoding or streaming to the GPU; past it, it takes the next frame all the same,
/// saying so.
const CAPTURE_WAIT: Duration = Duration::from_secs(10);

/// How many PNGs the `captures` directory beside the executable keeps, the newest, as each capture
/// without a path is written there: older ones, a long session's and those of runs before, go.
const KEPT_CAPTURES: usize = 32;

/// How many snapshots the `snapshots` directory beside the executable keeps, the newest, as each
/// snapshot without a path is written there; those `gg run` takes, in a directory of its own
/// inside it, are its to remove.
const KEPT_SNAPSHOTS: usize = 8;

/// The longest a run that ends once it has answered a `snapshot` with `quit` waits for the answer
/// to be written to its connection, which takes as long as a client that doesn't read holds it.
const ANSWERING: Duration = Duration::from_secs(5);

/// The levels of `tracing`, in the order [`gg_api::LEVELS`] names them.
const LEVELS: [Level; 5] = [
    Level::ERROR,
    Level::WARN,
    Level::INFO,
    Level::DEBUG,
    Level::TRACE,
];

/// The tool API, from when it starts until the run ends, as it drops, which removes its session
/// file.
pub(crate) struct Api {
    /// The session file.
    session: PathBuf,
    /// Where captures without a path go, and how their names start.
    captures: PathBuf,
    stem: String,
    /// The requests the connections' threads pass on, for the main loop to answer, and how many it
    /// has received.
    incoming: Receiver<Incoming>,
    received: u64,
    /// A request taken while waiting for one, answered first at the next turn.
    held: Option<Incoming>,
    shared: Arc<Shared>,
    /// `step`s waiting for the simulation to have run as many ticks as each says.
    steps: Vec<(Reply, u64)>,
    /// `assets` waiting for the thread that watches the asset root to have looked since each came.
    listing: Vec<Listing>,
    /// `capture`s waiting for a frame handed off of their kind, bare or not, in the order they
    /// came.
    asked: Vec<Capturing>,
    /// `capture`s riding with the frame handed off last, all of its kind, which shows the world
    /// after `riding_ticks` ticks, until a frame that rendered brings its view back.
    riding: Vec<Capturing>,
    riding_ticks: u64,
    /// `frames` in the order they came, the first capturing its frames, the rest waiting for it;
    /// whether the frame being made is the first's, of its kind and drawn from its viewpoint, if it
    /// gives them; and whether the frame made last drew from a film's viewpoint in place of the
    /// look's.
    films: Vec<Filming>,
    film_frame: bool,
    film_viewpoint: bool,
    /// Whether the frame being made is bare, and what it shows in place of the view's light, as
    /// the film capturing frames, or else the oldest capture waiting, decided as the frame started:
    /// a frame of either is apart, not published.
    bare: bool,
    shown: Option<Shown>,
    /// The look as the clients set it, each part `None` where it's the game's own, and the run's
    /// number; its `seen` is filled in as it's answered.
    look: Looked,
    /// `pick`s waiting for their answers.
    picks: Vec<Picking>,
    /// `images` waiting for a frame handed off, and those riding with the frames handed off since
    /// they came, until one that rendered brings its images back.
    images_asked: Vec<Reply>,
    images_riding: Vec<Reply>,
    /// The captures written without a path, which number their files.
    numbered: u64,
    /// The memory a `view` asked for the view to be published to, or `None` to stop, until the
    /// next frame back takes it to the render thread; and the name of the memory published to, or
    /// asked for, which a `view` naming it again leaves as it is.
    viewing: Option<Option<Writer>>,
    memory: Option<String>,
    /// What paces frames by the client's taking them, while the view is published.
    pacer: Option<Pacer>,
    /// Whether a frame that wasn't published has been warned of, since the view was asked for.
    view_warned: bool,
    /// To the thread that writes captures' PNGs.
    pngs: Sender<Writing>,
    /// Where snapshots without a path go, and how many have, which number their files.
    snapshots: PathBuf,
    snapshotted: u64,
    /// To the thread that writes snapshots.
    taken: Sender<Taken>,
    /// Edits and patches offered to a lockstep session, waiting for the tick they apply before.
    offered: Vec<(Reply, Ticket, Offer)>,
    /// The session file as written, which is written again as the run's player changes, as a
    /// session the game hosts or joins starts or ends.
    written: Session,
}

/// An `assets` waiting for a look at the asset root, as `Context::ask_assets` numbered it, and the
/// page it asks for.
struct Listing {
    reply: Reply,
    look: u64,
    kind: Option<AssetKind>,
    skip: usize,
    take: usize,
}

/// A `pick` waiting for its answer: at a pixel of the view, within the model of an entity, if it
/// names one, as [`gg_api::Pick::within`] says, with the ticket the frame that asked it gave, once
/// one has; and whether it answers its entity's name, as it does to a connection granted `read` of
/// `Name`.
struct Picking {
    at: [f64; 2],
    within: Option<EntityId>,
    ticket: Option<crate::Pick>,
    named: bool,
    reply: Reply,
}

/// A `capture` waiting for its frame: where its answer goes, where its PNG goes, whether it's of
/// the game's own view, which a bare frame shows, what it shows in place of the view's light, if it
/// asks, and when it came, which it waits from, as [`CAPTURE_WAIT`] says. With `at` and no path it
/// answers the pixel and writes no PNG.
struct Capturing {
    reply: Reply,
    path: Option<PathBuf>,
    at: Option<[f64; 2]>,
    own: bool,
    show: Option<Shown>,
    since: Instant,
}

/// A `frames` capturing frames in a row, or waiting to: where its answer and its PNGs go, how many
/// frames it captures, the viewpoints they're drawn from, the part of each it keeps, what they show
/// in place of the view's light, and when it came, as [`CAPTURE_WAIT`] counts from. Its frames are
/// those handed off from the first made after a frame comes back with nothing on its way, `started`
/// from then: each frame handed off as its own, the ticks it shows; and those back, in order.
struct Filming {
    reply: Reply,
    dir: PathBuf,
    count: u32,
    viewpoints: Vec<Viewpoint>,
    rect: Option<[u32; 4]>,
    show: Option<Shown>,
    since: Instant,
    started: bool,
    handed: Vec<u64>,
    back: Vec<FilmShot>,
}

impl Filming {
    /// The viewpoint the frame made next draws from, if it's one of the film's: its first until the
    /// film starts, then each in turn, the last held; `None` without viewpoints, or once every frame
    /// of the film is handed off.
    fn viewpoint(&self) -> Option<Viewpoint> {
        let next = self.handed.len();
        if next >= self.count as usize {
            return None;
        }
        let last = self.viewpoints.len().checked_sub(1)?;
        self.viewpoints.get(next.min(last)).copied()
    }
}

/// A frame of a `frames` back from the renderer, its view cropped to the film's rect, with the
/// ticks it shows, whether anything was on its way to the GPU in it, and its viewpoint.
struct FilmShot {
    shot: Arc<Shot>,
    ticks: u64,
    streaming: bool,
    viewpoint: Viewpoint,
}

/// What an edit or a patch offered to a lockstep session was asked as, which its answer reads as.
enum Offer {
    Apply,
    Step {
        author: String,
    },
    Patch,
    /// A save as a prefab's link, of the prefab whose file's path from the asset root and name
    /// these are.
    Prefab {
        path: String,
        name: String,
    },
}

/// What the main loop shares with the connections' threads.
struct Shared {
    /// The frames that have started, which `ping` answers.
    frames: AtomicU64,
    /// The requests the connections' threads have passed on to the main loop, each counted once
    /// it's sent, which a frame answers those of that came before it started.
    passed: AtomicU64,
    /// What wakes the main loop as a request comes, so that it answers while frames pause.
    waker: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// The events of the run's log that `logs` reads, and the number drawn at random that tells
    /// its cursors from another run's.
    events: &'static Events,
    run: u64,
    /// The debugger of the run's scripts, which `debug` asks.
    debugger: Arc<Debugger>,
}

/// A request for the main loop to answer, the client of its connection, whether that's granted
/// `read` of `Name`, which a `pick` answers, and where its answer goes.
struct Incoming {
    call: Call,
    client: Arc<str>,
    names: bool,
    reply: Reply,
}

/// The requests the main loop answers, as read.
#[derive(Debug, PartialEq)]
enum Call {
    Status,
    Pause,
    Resume,
    Play,
    Stop,
    Step(u64),
    /// What to do to the open scene, with the path to open or save it as, as given, checked to be
    /// a `.ron` file's, and whether to discard its unsaved edits.
    Scene(Scene),
    Capture {
        path: Option<PathBuf>,
        /// A pixel of the view to answer the bytes of, checked finite and not negative.
        at: Option<[f64; 2]>,
        own: bool,
        show: Option<gg_api::Shown>,
    },
    /// Frames in a row: the directory their PNGs go in, how many, their viewpoints, each checked,
    /// its rotation normalized, the part of each kept, of some area, and what they show.
    Frames {
        dir: PathBuf,
        count: u32,
        viewpoints: Vec<gg_api::Viewpoint>,
        rect: Option<[u32; 4]>,
        show: Option<gg_api::Shown>,
    },
    /// The name of the memory to publish the view to, or `None` to stop.
    View(Option<String>),
    /// Events to hand the layers, each of which names a key that's a key's.
    Input(Vec<InputEvent>),
    /// The parts of the look to set, each checked, its viewpoint's rotation normalized.
    Look(gg_api::Look),
    /// The pixel of the view to pick at, and the entity whose model it picks within, if any.
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
    /// A snapshot, at the path given, if one is, and whether the run ends once it's written.
    Snapshot {
        path: Option<PathBuf>,
        quit: bool,
    },
    Patch(JumpTable),
    /// An edit's operations, and the gesture it's part of, if it's sets of one field.
    Apply(Vec<gg_api::Operation>, Option<u64>),
    /// The entity to save as a prefab and link to it, and the path of the prefab's file, as given,
    /// checked to be a `.prefab` file's.
    SavePrefab {
        id: EntityId,
        path: Option<PathBuf>,
    },
    /// An undo, or a redo, of the edits of the author named, or else of the client's own.
    Undo {
        author: Option<String>,
        redo: bool,
    },
    Edits(Option<String>),
    Quit,
}

/// Where a request's answer goes: its connection's writer, which drops it once the connection has
/// gone; and the request's place among those its connection has in flight.
struct Reply {
    id: u64,
    to: Sender<Answer>,
    flight: Flight,
}

/// An answer's line, for its connection's writer, with its request's place among those in flight,
/// which goes once the line is written.
type Answer = (String, Flight);

impl Reply {
    fn send(self, answer: Result<String, Failure>) {
        let line = gg_api::answer_line(self.id, answer.as_deref());
        let _ = self.to.send((line, self.flight));
    }

    fn result(self, result: &impl Serialize) {
        self.send(Ok(
            serde_json::to_string(result).expect("a result serializes")
        ));
    }

    /// What's told once its answer is written to its connection, or can't be, the connection
    /// gone: so that a run that ends once it has answered ends after the client has its answer.
    fn landing(&mut self) -> Receiver<()> {
        let (landed, landing) = mpsc::channel();
        self.flight.1 = Some(landed);
        landing
    }
}

/// A snapshot for the thread that writes them: the copy of the world, the ticks the simulation had
/// run as it was taken, where it goes, and where the answer goes.
struct Taken {
    world: World,
    /// The scene files the layers loaded, which the restored run watches, and its open scene.
    scenes: Vec<SceneFile>,
    /// The scene files' texts the run keeps for its scene changes.
    cached: Vec<SceneFile>,
    open: Open,
    /// The scripts the layers loaded, each a path and its text as last applied, and the mods.
    scripts: Vec<(String, String)>,
    mods: Vec<crate::luau::ModText>,
    /// What the run was doing, with a copy of the world as edited it held, if any, which the
    /// restored run goes on doing.
    run: Run,
    /// Whether the simulation was paused, as the restored run is then.
    paused: bool,
    ticks: u64,
    path: PathBuf,
    /// Whether the engine named the path, the request giving none, which prunes the directory.
    named: bool,
    reply: Reply,
    /// Told whether the snapshot was written once it's answered, for a snapshot the run waits for.
    written: Option<Sender<bool>>,
}

/// A capture's PNG for the thread that writes them: the view, where it goes, the ticks the frame
/// that shows it had run, whether textures or meshes were still decoding or streaming in it, and
/// where the answer goes.
struct Png {
    shot: Arc<Shot>,
    /// Where it goes; `None` for a capture of a pixel alone, which writes nothing.
    path: Option<PathBuf>,
    /// The pixel whose bytes the answer gives.
    at: Option<[f64; 2]>,
    /// Whether the engine named the path, the request giving none, which prunes the directory.
    named: bool,
    ticks: u64,
    streaming: bool,
    reply: Reply,
}

/// What the thread that writes PNGs writes: a capture's, or a `frames`' every frame, into its
/// directory, numbered from 1, answered once all are written.
enum Writing {
    Capture(Png),
    Frames {
        dir: PathBuf,
        shots: Vec<FilmShot>,
        reply: Reply,
    },
}

impl Api {
    /// Starts serving app `app`'s run, whose layers are `layers`, from the bottom up, whose JSON
    /// log is at `log`, if it has one, and keeps its last events in `events`, which plays `player`
    /// in a lockstep session, if it's a peer of one, and whose project's manifest is at `project`,
    /// if it's a project's game: listens on the loopback, starts its threads and writes the session
    /// file, which only the user the game runs as can read, from before it holds the token, as
    /// `dirs::write_private` writes it: the token is all that keeps another process from `patch`,
    /// which loads a library into the game, and from the writes of `capture` and `snapshot`.
    ///
    /// Fails with `api.start` if it can't.
    pub(crate) fn start(
        app: &str,
        layers: &[&str],
        log: Option<&Path>,
        events: &'static Events,
        player: Option<u8>,
        project: Option<&Path>,
    ) -> Result<Self> {
        let failed = |what: String| move |error| Error::new("api.start", what).with_source(error);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(failed("can't listen on the loopback".to_owned()))?;
        let address = listener
            .local_addr()
            .map_err(failed("can't tell where it listens".to_owned()))?;
        let pid = std::process::id();
        // std seeds its hashers' keys from the OS's randomness, and SipHash's output under them
        // tells nothing of them.
        let random = RandomState::new();
        let token = format!("{:016x}{:016x}", random.hash_one(1), random.hash_one(2));
        // Under 2^53, so that JSON's readers that take every number for a double, as JavaScript's
        // do, which agents' MCP clients may be written in, read it whole.
        let run = random.hash_one(3) & ((1 << 53) - 1);
        let session = Session {
            app: app.to_owned(),
            pid,
            address,
            token,
            log: log.map(Path::to_owned),
            engine: crate::recording::engine(),
            aslr_reference: patch::aslr_reference(),
            player,
            project: project.map(Path::to_owned),
            protocol: Some(gg_api::PROTOCOL),
        };
        let welcome = Welcome {
            app: session.app.clone(),
            pid,
            engine: session.engine.clone(),
            log: session.log.clone(),
        };
        let welcome = serde_json::to_string(&welcome).expect("a welcome serializes");
        let (to_main, incoming) = mpsc::channel();
        let shared = Arc::new(Shared {
            frames: AtomicU64::new(0),
            passed: AtomicU64::new(0),
            waker: Mutex::new(None),
            events,
            run,
            debugger: Arc::default(),
        });
        let (pngs, to_write) = mpsc::channel();
        thread::Builder::new()
            .name("gg-api-png".to_owned())
            .spawn(move || write_pngs(&to_write))
            .map_err(failed(
                "can't start the thread that writes captures".to_owned(),
            ))?;
        let (taken, to_save) = mpsc::channel();
        let named = app.to_owned();
        let stack: Vec<String> = layers.iter().map(|&name| name.to_owned()).collect();
        thread::Builder::new()
            .name("gg-api-snapshot".to_owned())
            .spawn(move || write_snapshots(&to_save, &named, &stack))
            .map_err(failed(
                "can't start the thread that writes snapshots".to_owned(),
            ))?;
        let token = session.token.clone();
        let accepting = shared.clone();
        thread::Builder::new()
            .name("gg-api".to_owned())
            .spawn(move || accept(&listener, &token, &welcome, &to_main, &accepting))
            .map_err(failed(
                "can't start the thread that accepts connections".to_owned(),
            ))?;
        let beside = crate::beside_exe("sessions");
        let path = beside.join(Session::file_name(app, pid));
        let text = serde_json::to_string_pretty(&session).expect("a session serializes");
        crate::dirs::write_private(&path, text.as_bytes()).map_err(failed(format!(
            "can't write the session file {}",
            path.display()
        )))?;
        info!(
            address = %address,
            session = %path.display(),
            "Serving the tool API"
        );
        let stem = Session::file_name(app, pid);
        let stem = stem.strip_suffix(".json").unwrap_or(&stem).to_owned();
        Ok(Self {
            session: path,
            captures: crate::beside_exe("captures"),
            stem,
            incoming,
            received: 0,
            held: None,
            shared,
            steps: Vec::new(),
            listing: Vec::new(),
            asked: Vec::new(),
            riding: Vec::new(),
            riding_ticks: 0,
            films: Vec::new(),
            film_frame: false,
            film_viewpoint: false,
            bare: false,
            shown: None,
            look: Looked {
                viewpoint: None,
                outline: None,
                grid: None,
                colliders: None,
                show: None,
                skip: None,
                by: By::default(),
                revision: 0,
                run,
                seen: None,
            },
            picks: Vec::new(),
            images_asked: Vec::new(),
            images_riding: Vec::new(),
            numbered: 0,
            viewing: None,
            memory: None,
            pacer: None,
            view_warned: false,
            pngs,
            snapshots: crate::beside_exe("snapshots"),
            snapshotted: 0,
            taken,
            offered: Vec::new(),
            written: session,
        })
    }

    /// Writes the session file again with `player`, if that's not the player it names, as a
    /// session the game hosts or joins starts or ends, so that tools reach a session's peers as
    /// they do those its flags started; warns where it can't (`api.session`).
    fn follow_player(&mut self, player: Option<u8>) {
        if self.written.player == player {
            return;
        }
        self.written.player = player;
        let text = serde_json::to_string_pretty(&self.written).expect("a session serializes");
        if let Err(error) = crate::dirs::write_private(&self.session, text.as_bytes()) {
            warn!(
                code = "api.session",
                session = %self.session.display(),
                "Can't write the session file again with the run's player: {error}"
            );
        }
    }

    /// Has `wake` wake the main loop as each request comes.
    pub(crate) fn set_waker(&mut self, wake: Box<dyn Fn() + Send>) {
        *self
            .shared
            .waker
            .lock()
            .expect("no thread panics holding it") = Some(wake);
    }

    /// The debugger of the run's scripts, which `debug` asks.
    pub(crate) fn debugger(&self) -> Arc<Debugger> {
        self.shared.debugger.clone()
    }

    /// As a frame starts, before its ticks, answers the requests that had come as it started, from
    /// `cx`, whose layers are `stack`'s and whose simulation `stepper` steps, so that clients that
    /// keep sending them can't keep the frame from going on: first the `step`s whose ticks have
    /// run, then the rest in the order they came, `look`s setting the look the frame draws with,
    /// `capture`s and `pick`s once a frame has rendered the view, as
    /// [`frame_back`](Self::frame_back) says, and `snapshot`s once their files are written, but
    /// refused before the first tick of a run playing from its start. Then makes the frame of the
    /// kind of the oldest capture riding, whose frame may not render, or else of the film that has
    /// frames to capture, or else of the oldest capture asked: bare if it's of the game's own view,
    /// and showing what it shows in place of the view's light; a film's frame drawn from its
    /// viewpoint, if it gives them, as [`Filming::viewpoint`] says, in place of the look's, which
    /// the next frame not of a film's gets back. Unless the frame's bare, which reads no picks, has
    /// it ask the picks waiting, as many as [`MOST_PICKS`], the rest waiting for the frames after.
    /// Returns whether a request asked the run to end.
    pub(crate) fn serve(
        &mut self,
        cx: &mut Context,
        stack: &mut Stack,
        stepper: &mut Stepper,
    ) -> bool {
        self.shared.frames.fetch_add(1, Ordering::Relaxed);
        self.follow_player(cx.player());
        let quit = self.answer(cx, stack, stepper, false);
        let film = (self.films.first()).filter(|film| film.handed.len() < film.count as usize);
        let kind = match (self.riding.first(), film, self.asked.first()) {
            (Some(capturing), ..) | (None, None, Some(capturing)) => {
                Some((capturing.own, capturing.show.clone()))
            }
            (None, Some(film), _) => Some((false, film.show.clone())),
            (None, None, None) => None,
        };
        (self.bare, self.shown) = kind.unwrap_or_default();
        cx.look.bare = self.bare;
        cx.look.captured = self.shown.clone();
        let filming = film.filter(|film| !self.bare && self.shown == film.show);
        let filmed = filming.and_then(Filming::viewpoint);
        if let Some(viewpoint) = filmed {
            cx.look.viewpoint = Some(viewpoint);
        } else if self.film_viewpoint {
            cx.look.viewpoint = self.look.viewpoint.as_ref().map(engine_viewpoint);
        }
        self.film_frame = filming.is_some();
        self.film_viewpoint = filmed.is_some();
        if self.bare {
            return quit;
        }
        let origin = cx.view().map_or([0, 0], |view| view.origin).map(f64::from);
        let unasked = (self.picks.iter_mut()).filter(|picking| picking.ticket.is_none());
        for Picking { at, ticket, .. } in unasked.take(MOST_PICKS) {
            *ticket = cx.pick([origin[0] + at[0], origin[1] + at[1]]);
            if ticket.is_none() {
                break;
            }
        }
        quit
    }

    /// As [`serve`](Self::serve), between frames while they pause for a minimized window, without
    /// counting a frame: a `capture`, a `pick`, an `images` or a `step`, which need frames, fail
    /// with `api.paused`.
    pub(crate) fn serve_paused(
        &mut self,
        cx: &mut Context,
        stack: &mut Stack,
        stepper: &mut Stepper,
    ) -> bool {
        self.answer(cx, stack, stepper, true)
    }

    /// As frames pause, answers each `assets` waiting for a look from `cx`'s database as it is, as
    /// `api::frames_paused` says.
    pub(crate) fn frames_paused(&mut self, cx: &Context) {
        for listing in self.listing.drain(..) {
            let Listing {
                reply,
                kind,
                skip,
                take,
                ..
            } = listing;
            reply.send(assets(cx, kind, skip, take));
        }
    }

    /// Answers the requests that have come, as [`serve`](Self::serve) says, `paused` if frames
    /// have paused.
    fn answer(
        &mut self,
        cx: &mut Context,
        stack: &mut Stack,
        stepper: &mut Stepper,
        paused: bool,
    ) -> bool {
        let ran = self
            .steps
            .extract_if(.., |&mut (_, until)| until <= stepper.ticks);
        for (reply, _) in ran.collect::<Vec<_>>() {
            reply.result(&self.status(cx, stepper));
        }
        let answered = cx.assets_answered();
        let looked = self
            .listing
            .extract_if(.., |listing| listing.look <= answered);
        for listing in looked {
            let Listing {
                reply,
                kind,
                skip,
                take,
                ..
            } = listing;
            reply.send(assets(cx, kind, skip, take));
        }
        let applied = self
            .offered
            .extract_if(.., |(_, ticket, _)| stack.offered_is_done(*ticket));
        for (reply, ticket, offer) in applied.collect::<Vec<_>>() {
            if let Some((ticks, done)) = stack.offered(ticket) {
                answer_done(reply, &offer, ticks, done);
            }
        }
        let mut quit = false;
        let came = self.shared.passed.load(Ordering::Acquire);
        while let Some(Incoming {
            call,
            client,
            names,
            reply,
        }) = self.next(came)
        {
            let world = cx.world();
            match call {
                // What a quit leaves would go with the run, and a snapshot that quits is what a
                // run started again goes on from: so nothing past either applies, nor says so.
                _ if quit => {
                    let message = "the game is quitting, as a request before this one asked, and \
                                   took nothing of it; a run `gg run` starts again takes requests \
                                   once it serves";
                    reply.send(Err(Failure::new("api.quitting", message)));
                }
                Call::Step(_)
                | Call::Capture { .. }
                | Call::Frames { .. }
                | Call::Pick(..)
                | Call::Images
                    if paused =>
                {
                    let message = "frames have paused while the window is minimized, and steps, \
                                   captures, picks and images run in frames; restore the window \
                                   first";
                    reply.send(Err(Failure::new("api.paused", message)));
                }
                Call::Step(_) if stepper.stopped() => {
                    let message = "the game is stopped, and runs no ticks until `play`";
                    reply.send(Err(Failure::new("api.stopped", message)));
                }
                Call::Status => reply.result(&self.status(cx, stepper)),
                Call::Pause => {
                    stepper.pause();
                    reply.result(&self.status(cx, stepper));
                }
                Call::Resume => {
                    stepper.resume();
                    let status = self.status(cx, stepper);
                    for (waiting, _) in self.steps.drain(..) {
                        waiting.result(&status);
                    }
                    reply.result(&status);
                }
                Call::Play => {
                    if stepper.stopped() {
                        stack.play(cx, stepper.ticks);
                        stepper.play();
                    }
                    reply.result(&self.status(cx, stepper));
                }
                Call::Stop => match stack.refuses_stop() {
                    Some(why) => {
                        let message = format!("the game can't stop: {why}");
                        reply.send(Err(Failure::new("api.no_stop", message)));
                    }
                    None => {
                        if !stepper.stopped() {
                            self.stop(cx, stack, stepper);
                        }
                        reply.result(&self.status(cx, stepper));
                    }
                },
                Call::Scene(scene) => match self.scene(cx, stack, stepper, scene) {
                    Ok(()) => reply.result(&self.status(cx, stepper)),
                    Err(error) => reply.send(Err(failure(&error))),
                },
                Call::Step(ticks) => {
                    stepper.step_by(ticks);
                    let until = stepper.ticks + stepper.steps();
                    self.steps.push((reply, until));
                }
                Call::Capture {
                    path,
                    at,
                    own,
                    show,
                } => self.asked.push(Capturing {
                    reply,
                    path,
                    at,
                    own,
                    show: show.as_ref().map(Shown::of),
                    since: Instant::now(),
                }),
                Call::Frames {
                    dir,
                    count,
                    viewpoints,
                    rect,
                    show,
                } => {
                    // A view of no area, or none, fails as the frames come back without one.
                    let size = (rect.map(|[_, _, width, height]| [width, height]))
                        .or(cx.view().map(|view| view.size));
                    let held = size.map_or(0, |[width, height]| {
                        u64::from(count) * u64::from(width) * u64::from(height) * 4
                    });
                    if held > gg_api::MOST_FRAMES_BYTES {
                        let [width, height] = size.unwrap_or_default();
                        let what = format!(
                            "holds at most {} bytes of pixels, and {count} frames of \
                             {width}x{height} would hold {held}: ask for fewer frames, or a \
                             smaller `rect`",
                            gg_api::MOST_FRAMES_BYTES
                        );
                        reply.send(Err(bad_params("frames", what)));
                    } else {
                        self.films.push(Filming {
                            reply,
                            dir,
                            count,
                            viewpoints: viewpoints.iter().map(engine_viewpoint).collect(),
                            rect,
                            show: show.as_ref().map(Shown::of),
                            since: Instant::now(),
                            started: false,
                            handed: Vec::new(),
                            back: Vec::new(),
                        });
                    }
                }
                // A second writer of the memory would write the slot the first holds.
                Call::View(memory) if memory.is_some() && memory == self.memory => {
                    reply.send(Ok("{}".to_owned()));
                }
                Call::View(memory) => match memory.as_deref().map(Writer::open).transpose() {
                    Ok(writer) => {
                        match &memory {
                            Some(memory) => info!(memory, "Publishing the view"),
                            None => info!("No longer publishing the view"),
                        }
                        self.pacer = writer.as_ref().map(Writer::pacer);
                        self.viewing = Some(writer);
                        self.memory = memory;
                        self.view_warned = false;
                        reply.send(Ok("{}".to_owned()));
                    }
                    Err(error) => {
                        let name = memory.unwrap_or_default();
                        let message = format!("can't open the view's memory {name:?}: {error}");
                        reply.send(Err(Failure::new("api.view", message)));
                    }
                },
                Call::Input(events) => {
                    let origin = cx.view().map_or([0, 0], |view| view.origin);
                    for given in &events {
                        let event = input_event(given, origin);
                        let _ = stack.event(cx, &event.expect("checked as it was read"));
                    }
                    reply.send(Ok("{}".to_owned()));
                }
                Call::Look(given) => {
                    self.set_look(cx, given, &client);
                    let seen = cx.look.seen.as_ref().map(api_viewpoint);
                    // The game's own camera may be placed by numbers that aren't finite, which JSON
                    // has none for, and a client couldn't read back.
                    self.look.seen = seen.filter(gg_api::Viewpoint::is_finite);
                    reply.result(&self.look);
                }
                Call::Pick(at, within) => self.picks.push(Picking {
                    at,
                    within,
                    ticket: None,
                    named: names,
                    reply,
                }),
                Call::Images => self.images_asked.push(reply),
                Call::Entities { with, skip, take } => {
                    reply.send(list(world, with.as_deref(), skip, take));
                }
                Call::Entity(id) => reply.send(entity(world, cx.meshes(), id)),
                Call::Get {
                    id,
                    component,
                    path,
                } => reply.send(field(world, id, &component, &path)),
                Call::Types => reply.result(&json::describe(world)),
                Call::Hash => {
                    let hash = world.hash();
                    reply.result(&Hash {
                        content: format!("{:016x}", hash.content),
                        layout: format!("{:016x}", hash.layout),
                    });
                }
                Call::Assets { kind, skip, take } => {
                    // While frames pause, a look the thread takes would wait for the next request
                    // to be answered, so the database as the thread last sent it answers; and a run
                    // that no thread watches the root for scans it here.
                    let look = if paused { None } else { cx.ask_assets() };
                    match look {
                        Some(look) => self.listing.push(Listing {
                            reply,
                            look,
                            kind,
                            skip,
                            take,
                        }),
                        None => {
                            if !cx.watches_assets() {
                                cx.scan_assets(stack.watches_files());
                            }
                            reply.send(assets(cx, kind, skip, take));
                        }
                    }
                }
                Call::Ui => reply.result(&cx.ui_list()),
                Call::Cells => reply.result(&cx.cells_read(stepper.stopped(), stepper.ticks)),
                // A run stopped, or playing a copy, is snapshotted before its first tick too, so
                // that a restart keeps the world as edited; what the layers queued as they attached
                // goes, as it does at a `stop` that brings that world back. One restored from a
                // snapshot, whose layers attached nothing, is too, as one restored paused may not
                // tick for a while.
                Call::Snapshot { .. }
                    if stepper.fresh() && !stepper.stopped() && cx.edited.is_none() =>
                {
                    let message = "the simulation hasn't ticked since the run started, and a \
                                   snapshot before then would miss what the layers queued for its \
                                   first tick; ask once it has";
                    reply.send(Err(Failure::new("api.not_ticked", message)));
                }
                Call::Snapshot { path, quit: quits } => {
                    let named = path.is_none();
                    let path = path.unwrap_or_else(|| {
                        self.snapshotted += 1;
                        let name = format!("{}-{}.snapshot", self.stem, self.snapshotted);
                        self.snapshots.join(name)
                    });
                    let run = match &cx.edited {
                        Some(edited) => Run::Held {
                            world: Box::new(edited.world.clone()),
                            scenes: edited.scenes.clone(),
                            open: edited.open.clone(),
                            ticks: edited.ticks,
                        },
                        None if stepper.stopped() => Run::Stopped,
                        None => Run::Playing,
                    };
                    let mut reply = reply;
                    let landing = quits.then(|| reply.landing());
                    let taken = Taken {
                        world: world.clone(),
                        scenes: cx.scenes.files.clone(),
                        cached: cx.scenes.cached.clone(),
                        open: cx.scenes.open.clone(),
                        scripts: cx.hosts.applied(),
                        mods: cx.mods.applied(),
                        run,
                        paused: stepper.paused(),
                        ticks: stepper.ticks,
                        path,
                        named,
                        reply,
                        written: None,
                    };
                    if quits {
                        // Written before the run ends, which ends the thread with it: the main
                        // thread waits for it, as the run would only end anyway, and the snapshots
                        // asked before it are written first.
                        let (written, done) = mpsc::channel();
                        let taken = Taken {
                            written: Some(written),
                            ..taken
                        };
                        // One that fails leaves the run going, as one that isn't taken does.
                        quit = self.taken.send(taken).is_ok() && done.recv().unwrap_or(false);
                        // Its answer goes out before the run ends, which would end the thread that
                        // writes it, and a client would take the snapshot for one that failed.
                        if quit && let Some(landing) = landing {
                            let _ = landing.recv_timeout(ANSWERING);
                        }
                    } else {
                        // The thread runs until the API drops.
                        let _ = self.taken.send(taken);
                    }
                }
                Call::Patch(table) => match stack.offer_patch(table) {
                    Ok(ticket) => self.offered.push((reply, ticket, Offer::Patch)),
                    Err(table) => {
                        let patched = stack.patch(table, stepper.ticks);
                        let done = patched.map(|()| Done::Applied(Vec::new()));
                        answer_done(reply, &Offer::Patch, stepper.ticks, done);
                    }
                },
                Call::Apply(given, gesture) => {
                    // What its values name that isn't loaded is loaded first, as a scene's is.
                    let named = assets_named(world, &given);
                    cx.load_named(named, false);
                    match operations(cx.world(), cx.world(), given) {
                        Ok(operations) => {
                            let author = client.to_string();
                            let edit = match gesture {
                                Some(gesture) => Edit::Amend {
                                    author,
                                    gesture,
                                    operations,
                                },
                                None => Edit::Apply { author, operations },
                            };
                            let offered = stack.offer(cx, edit);
                            self.answer_offered(reply, offered, Offer::Apply, stepper.ticks);
                        }
                        Err(failure) => reply.send(Err(failure)),
                    }
                }
                Call::SavePrefab { id, path } => match save_prefab(cx, stack, id, path) {
                    Ok(saved) => {
                        let name = cx.world().assets().prefab_name(saved.prefab).to_owned();
                        // Checked as the save wrote the file, in this world as it is: it applies.
                        let link = Operation::Link {
                            entity: id,
                            prefab: saved.prefab,
                        };
                        let edit = Edit::Apply {
                            author: client.to_string(),
                            operations: vec![link],
                        };
                        let offered = stack.offer(cx, edit);
                        let path = saved.path;
                        let offer = Offer::Prefab { path, name };
                        self.answer_offered(reply, offered, offer, stepper.ticks);
                    }
                    Err(error) => reply.send(Err(failure(&error))),
                },
                Call::Undo { author, redo } => {
                    let author = author.unwrap_or_else(|| client.to_string());
                    let edit = if redo {
                        Edit::Redo {
                            author: author.clone(),
                        }
                    } else {
                        Edit::Undo {
                            author: author.clone(),
                        }
                    };
                    let offered = stack.offer(cx, edit);
                    self.answer_offered(reply, offered, Offer::Step { author }, stepper.ticks);
                }
                Call::Edits(author) => {
                    let logs = cx.authors.logs(world);
                    let logs =
                        logs.filter(|(name, _)| author.as_deref().is_none_or(|a| a == *name));
                    let authors = logs.map(|(name, log)| Authored {
                        author: name.to_owned(),
                        done: log.edits().map(words).collect(),
                        undone: log.undone().map(words).collect(),
                    });
                    reply.result(&History {
                        authors: authors.collect(),
                    });
                }
                Call::Quit => {
                    reply.send(Ok("{}".to_owned()));
                    quit = true;
                }
            }
        }
        quit
    }

    /// Stops the game, which plays, as `stop` asks of a run that can stop: brings back the world as
    /// edited, if a copy of it plays, at its ticks, as [`Stack::stop`] says; runs no ticks from
    /// then on; and fails the `step`s waiting with `api.stopped`.
    fn stop(&mut self, cx: &mut Context, stack: &mut Stack, stepper: &mut Stepper) {
        if let Some(ticks) = stack.stop(cx) {
            stepper.jump_to(ticks);
        }
        stepper.stop();
        let message = "the game stopped before the ticks ran: its world is the world as edited";
        for (waiting, _) in self.steps.drain(..) {
            waiting.send(Err(Failure::new("api.stopped", message)));
        }
    }

    /// Does to the open scene what `scene` asks, as gg-api's docs say: in a project's game, with
    /// its path resolved, as [`scene_files::resolve`] does, saves the open scene of the world as
    /// edited, as [`scene_files::save`] does; or, once [`scene_files::check_open`] or
    /// [`scene_files::check_unsaved`] passes, stops a copy that plays, as `stop` does, and opens a
    /// scene file in its place or empties it, as [`scene_files::open`] and [`scene_files::new`] do.
    ///
    /// Fails with `engine.no_project` in a game that isn't a project's; with `api.no_scene` in a
    /// game that runs from its package, which has no scene files, for a save in a lockstep peer,
    /// whose scene files the other's must stay as, and for an open or a
    /// new in a run that can't stop, as [`Stack::refuses_stop`] says, since neither the history
    /// they restart nor a copy's stopping is recorded or sent to a peer; and as those calls do.
    fn scene(
        &mut self,
        cx: &mut Context,
        stack: &mut Stack,
        stepper: &mut Stepper,
        scene: Scene,
    ) -> Result<()> {
        if cx.start.is_none() {
            return Err(scene_files::no_project());
        }
        if cx.reads_package() {
            let message = "the game can't open, save or empty the open scene: it runs from its \
                           package, which holds no scene files to open or save";
            return Err(Error::new("api.no_scene", message));
        }
        let existing = scene.action == SceneAction::Open;
        let resolve = |path: &Path| scene_files::resolve(cx, path, existing);
        let file = scene.path.as_deref().map(resolve).transpose()?;
        if scene.action == SceneAction::Save {
            if stack.is_peer() {
                let message = "the game can't save the open scene: the run is networked, and \
                               its scene files would part from the session's others'";
                return Err(Error::new("api.no_scene", message));
            }
            return scene_files::save(cx, file);
        }
        if let Some(why) = stack.refuses_stop() {
            let what = match scene.action {
                SceneAction::Open => "open a scene file in place of",
                SceneAction::New | SceneAction::Save => "empty",
            };
            let message = format!(
                "the game can't {what} the open scene, which starts every author's edits afresh \
                 and stops a copy that plays, as this run can't: {why}"
            );
            return Err(Error::new("api.no_scene", message));
        }
        let (ticks, discard) = (stepper.ticks, scene.discard);
        let opening = match file {
            Some(file) => Some(scene_files::check_open(cx, file, ticks, discard)?),
            None => {
                scene_files::check_unsaved(cx, ticks, discard)?;
                None
            }
        };
        if cx.edited.is_some() {
            self.stop(cx, stack, stepper);
        }
        match opening {
            Some(opening) => scene_files::open(cx, stack, opening),
            None => scene_files::new(cx, stack),
        }
    }

    /// Answers `reply`, asked as `offer`, with what an edit did, if it applied now, the next tick
    /// to run being `ticks`; or keeps it until the tick it applies before in a lockstep session.
    fn answer_offered(&mut self, reply: Reply, offered: Offered, offer: Offer, ticks: u64) {
        match offered {
            Offered::Now(done) => answer_done(reply, &offer, ticks, done),
            Offered::Later(ticket) => self.offered.push((reply, ticket, offer)),
        }
    }

    /// Sets the parts of the look `given` sets, as client `client`, a change of any part's value or
    /// setter counting as a revision; and hands the look to `cx`, for the frames to draw with.
    fn set_look(&mut self, cx: &mut Context, given: gg_api::Look, client: &str) {
        let Looked {
            viewpoint,
            outline,
            grid,
            colliders,
            show,
            skip,
            by,
            revision,
            ..
        } = &mut self.look;
        let changed = [
            set_part(viewpoint, &mut by.viewpoint, given.viewpoint, client),
            set_part(outline, &mut by.outline, given.outline, client),
            set_part(grid, &mut by.grid, given.grid, client),
            set_part(colliders, &mut by.colliders, given.colliders, client),
            set_part(show, &mut by.show, given.show, client),
            set_part(skip, &mut by.skip, given.skip, client),
        ];
        if changed.contains(&true) {
            *revision += 1;
        }
        let look = &mut cx.look;
        look.viewpoint = viewpoint.as_ref().map(engine_viewpoint);
        look.outline = outline.as_ref().map(|outline| Outlining {
            ids: (outline.ids.iter())
                .map(|id| id.parse::<EntityId>().expect("checked as it was read"))
                .collect(),
            color: Vec4::from_array(outline.color),
            width: outline.width,
        });
        look.grid = grid.map(|plane| match plane {
            gg_api::Plane::Xy => Plane::XY,
            gg_api::Plane::Xz => Plane::XZ,
            gg_api::Plane::Yz => Plane::YZ,
        });
        look.colliders = *colliders;
        look.show = show.as_ref().map(Shown::of);
        look.skip = skip.clone().unwrap_or_default();
        look.revision = *revision;
    }

    /// The game as the frame that answers starts, among it the open scene of the world as edited
    /// and whether it's unsaved, as [`scene_files::unsaved`] finds it.
    fn status(&self, cx: &mut Context, stepper: &Stepper) -> Status {
        let unsaved = scene_files::unsaved(cx, stepper.ticks);
        let entities = cx.world().entities(None);
        Status {
            ticks: stepper.ticks,
            paused: stepper.paused(),
            stopped: stepper.stopped(),
            frames: self.shared.frames.load(Ordering::Relaxed),
            entities: entities.map_or(0, |entities| entities.len()),
            view: cx.view().map(|view| view.size),
            scene: scene_files::title(cx).map(str::to_owned),
            unsaved,
            // The world as edited's, as `unsaved` is.
            edited: cx
                .edited
                .as_ref()
                .map_or(cx.scenes.open.edited, |edited| edited.open.edited),
            cells: (cx.world().cell_counts()).map(|(cells_in, kept)| CellCounts { cells_in, kept }),
        }
    }

    /// With the renderer back from a frame, `frame`, takes the view it read back, if it rode with
    /// captures and rendered, for a thread of its own to write each capture's PNG and answer it:
    /// if every texture and mesh added before it is there in the frame, none decoding or streaming
    /// still, and otherwise only for those that have waited [`CAPTURE_WAIT`] since they came,
    /// saying so, the rest riding on with the next; answers the picks whose answers came back into
    /// `cx`, each naming its entity as `cx`'s world does now; and has the next frame handed off,
    /// which `cx` made and shows the world after `ticks` ticks, read its view back for the captures
    /// of its kind, bare or not, asked since, and for those whose frame had nothing to render
    /// into, or showed what was still on its way, and publish it to the memory a `view` asked
    /// for, unless it's bare, warning once of a frame that wasn't, since the view was asked for.
    /// Likewise answers the `images` riding with frames handed off with the images `frame` listed,
    /// if it rendered, and has the next list them for those asked since and those still riding.
    pub(crate) fn frame_back(&mut self, frame: &mut Extracted, cx: &Context, ticks: u64) {
        if let Some(viewing) = self.viewing.take() {
            frame.publish = viewing.map(|writer| Publish {
                writer,
                skip: false,
                ticks,
                started: cx.started,
                failed: None,
            });
        }
        if let Some(publish) = &mut frame.publish {
            // For the client, which grabs its own cursor for the game.
            publish.writer.set_cursor_grabbed(cx.cursor_grabbed());
            if let Some(error) = publish.failed.take()
                && !mem::replace(&mut self.view_warned, true)
            {
                let consequence = "frames go unpublished while that lasts";
                warn!(code = error.code(), "{error:#}; {consequence}");
            }
            publish.skip = self.apart();
            publish.ticks = ticks;
            publish.started = cx.started;
        }
        let back = (frame.captured.take()).map(|shot| shot.map(Arc::new));
        let streaming = frame.loading > 0 || !frame.streaming.is_empty();
        self.film_back(back.as_ref(), streaming, frame.viewpoint);
        match back {
            Some(Ok(shot)) => {
                let now = Instant::now();
                let taken = (self.riding).extract_if(.., |capturing| {
                    !streaming || now.duration_since(capturing.since) >= CAPTURE_WAIT
                });
                for Capturing {
                    reply, path, at, ..
                } in taken
                {
                    let named = path.is_none() && at.is_none();
                    let path = match path {
                        Some(path) => Some(path),
                        None if at.is_some() => None,
                        None => {
                            self.numbered += 1;
                            let name = format!("{}-{}.png", self.stem, self.numbered);
                            Some(self.captures.join(name))
                        }
                    };
                    let png = Png {
                        shot: shot.clone(),
                        path,
                        at,
                        named,
                        ticks: self.riding_ticks,
                        streaming,
                        reply,
                    };
                    // The thread runs until the API drops.
                    let _ = self.pngs.send(Writing::Capture(png));
                }
            }
            Some(Err(error)) => {
                let failure = failure(&error);
                for capturing in self.riding.drain(..) {
                    capturing.reply.send(Err(failure.clone()));
                }
            }
            None => {}
        }
        let picked = cx.picked();
        let answered = |ticket: Option<crate::Pick>| {
            let ticket = ticket?;
            picked
                .iter()
                .find(|(pick, _)| *pick == ticket)
                .map(|&(_, id)| id)
        };
        let world = cx.world();
        let back = (self.picks).extract_if(.., |picking| answered(picking.ticket).is_some());
        for Picking {
            within,
            ticket,
            named,
            reply,
            ..
        } in back
        {
            let shown = answered(ticket).flatten();
            // A model's node is its root's, which tools pick and change in its place, but within
            // the model, as an editor that has picked its root or a node goes into it.
            let drawn = shown.and_then(|id| world.entity(id));
            let root = drawn.map(|entity| world.node_root(entity));
            let within = within.and_then(|id| world.entity(id));
            let within = within.map(|entity| world.node_root(entity));
            let entity = if within.is_some() && within == root {
                drawn
            } else {
                root
            };
            let id = entity.map_or(shown, |entity| world.id(entity));
            let name = entity.filter(|_| named);
            let name = name.and_then(|entity| world.name(entity));
            reply.result(&Picked {
                id: id.map(|id| id.to_string()),
                name: name.map(str::to_owned),
            });
        }
        let (bare, shown) = (self.bare, &self.shown);
        let kind = (self.asked).extract_if(.., |capturing| {
            capturing.own == bare && capturing.show == *shown
        });
        self.riding.extend(kind);
        if !self.riding.is_empty() {
            self.riding_ticks = ticks;
        }
        let filming = self.film_hands(streaming, ticks);
        frame.capture = !self.riding.is_empty() || filming;
        if let Some(images) = frame.images.take() {
            let listed = ImageList {
                images: images.iter().map(frame_image).collect(),
            };
            for reply in self.images_riding.drain(..) {
                reply.result(&listed);
            }
        }
        self.images_riding.append(&mut self.images_asked);
        frame.list_images = !self.images_riding.is_empty();
    }

    /// With a frame back, whose view `back` is, if it read it, which showed what was on its way to
    /// the GPU if `streaming`, from `viewpoint`: keeps it as the first film's next frame, cropped
    /// to its rect, if that film has a frame in flight, which is this one; and once it has all its
    /// frames, has the thread that writes PNGs write them and answer it. A frame of it back without
    /// its view, or with none of its rect, fails it.
    fn film_back(
        &mut self,
        back: Option<&Result<Arc<Shot>>>,
        streaming: bool,
        viewpoint: Viewpoint,
    ) {
        let Some(film) = self.films.first_mut() else {
            return;
        };
        if film.back.len() == film.handed.len() {
            return;
        }
        let shot = match back {
            Some(Ok(shot)) => crop(shot, film.rect).ok_or_else(|| {
                let message = format!(
                    "a frame's view, of {}x{} pixels, has none of the rect {:?}",
                    shot.size[0], shot.size[1], film.rect
                );
                Failure::new("api.no_view", message)
            }),
            Some(Err(error)) => Err(failure(error)),
            None => {
                let message = "a frame of them didn't render, as while a window resizes, so they \
                               wouldn't be in a row: ask again";
                Err(Failure::new("api.no_view", message))
            }
        };
        match shot {
            Ok(shot) => film.back.push(FilmShot {
                shot,
                ticks: film.handed[film.back.len()],
                streaming,
                viewpoint,
            }),
            Err(failure) => {
                self.film_frame = false;
                self.films.remove(0).reply.send(Err(failure));
                return;
            }
        }
        if film.back.len() == film.count as usize {
            let Filming {
                reply, dir, back, ..
            } = self.films.remove(0);
            // The thread runs until the API drops.
            let _ = self.pngs.send(Writing::Frames {
                dir,
                shots: back,
                reply,
            });
        }
    }

    /// Whether the frame handed off next, made after `ticks` ticks, is to read its view back for
    /// the first film: if it was made as the film's, as [`serve`](Self::serve) decided, and the
    /// film has started, as it does at the first frame back that showed nothing on its way, unless
    /// `streaming`, or once it has waited [`CAPTURE_WAIT`] since it came.
    fn film_hands(&mut self, streaming: bool, ticks: u64) -> bool {
        let film = (self.films.first_mut()).filter(|_| self.film_frame);
        let Some(film) = film else {
            return false;
        };
        film.started |= !streaming || film.since.elapsed() >= CAPTURE_WAIT;
        if film.started {
            film.handed.push(ticks);
        }
        film.started
    }

    /// Whether the simulation is paused or stopped with nothing asked of the frames: no ticks to
    /// step, no capture, `frames`, pick or `images` waiting and no request held.
    pub(crate) fn is_idle(&self, stepper: &Stepper) -> bool {
        (stepper.paused() || stepper.stopped())
            && stepper.steps() == 0
            && self.asked.is_empty()
            && self.riding.is_empty()
            && self.films.is_empty()
            && self.picks.is_empty()
            && self.images_asked.is_empty()
            && self.images_riding.is_empty()
            && self.held.is_none()
    }

    /// While the view is published, the number of the frame to be handed off next, as the client
    /// will take it; `None` while the frame is apart, which isn't published.
    pub(crate) fn handing(&self) -> Option<u64> {
        let pacer = self.pacer.as_ref().filter(|_| !self.apart())?;
        Some(pacer.published().saturating_add(1))
    }

    /// Whether the frame being made is apart from those the view's client is shown, and isn't
    /// published: bare, or showing what a capture asks in place of the view's light.
    fn apart(&self) -> bool {
        self.bare || self.shown.is_some()
    }

    /// While the view is published, waits until the client has taken frame `handed`, as
    /// [`handing`](Self::handing) gave it, for at most `timeout`.
    pub(crate) fn pace(&self, handed: u64, timeout: Duration) {
        let Some(pacer) = &self.pacer else {
            return;
        };
        let start = Instant::now();
        while pacer.taken() < handed && start.elapsed() < timeout {
            thread::sleep(Duration::from_micros(250));
        }
    }

    /// Waits for a request for at most `timeout`, holding it for the next turn to answer first.
    pub(crate) fn wait(&mut self, timeout: Duration) {
        if self.held.is_none() {
            self.held = self.incoming.recv_timeout(timeout).ok();
            self.received += u64::from(self.held.is_some());
        }
    }

    /// The next request to answer: the one held, if any, then those of the first `came` the
    /// connections' threads passed on that haven't been received; `None` once they have.
    fn next(&mut self, came: u64) -> Option<Incoming> {
        if let Some(held) = self.held.take() {
            return Some(held);
        }
        // Each counted is sent before it's counted, so it's there to receive.
        if self.received >= came {
            return None;
        }
        let incoming = self.incoming.try_recv().ok()?;
        self.received += 1;
        Some(incoming)
    }
}

impl Drop for Api {
    /// Removes the session file, so that tools don't find the run once it has ended: a crash or a
    /// kill leaves it, naming an address that refuses connections.
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.session);
    }
}

/// Answers `reply`, asked as `offer`, with what its edit or patch did, `done`, before tick `ticks`.
fn answer_done(reply: Reply, offer: &Offer, ticks: u64, done: Result<Done>) {
    match (offer, done) {
        (_, Err(error)) => reply.send(Err(failure(&error))),
        (Offer::Apply, Ok(Done::Applied(made))) => reply.result(&Applied {
            ticks,
            entities: made.iter().map(EntityId::to_string).collect(),
        }),
        (Offer::Step { author }, Ok(Done::Stepped(stepped))) => {
            let edit = stepped.map(|operations| gg_api::Edit {
                author: author.clone(),
                operations: words(&operations),
            });
            reply.result(&edit);
        }
        (Offer::Patch, Ok(_)) => reply.result(&Patched { ticks }),
        (Offer::Prefab { path, name }, Ok(Done::Applied(_))) => reply.result(&SavedPrefab {
            path: path.clone(),
            name: name.clone(),
            ticks,
        }),
        (Offer::Apply | Offer::Prefab { .. }, Ok(Done::Stepped(_))) => {
            unreachable!("an apply applies")
        }
        (Offer::Step { .. }, Ok(Done::Applied(_))) => unreachable!("an undo or a redo steps"),
    }
}

/// The page of `world`'s entities `entities` asks for.
fn list(world: &World, with: Option<&str>, skip: usize, take: usize) -> Result<String, Failure> {
    let mut listing = world.entities(with).map_err(|error| failure(&error))?;
    let total = listing.len();
    let page = listing.by_ref().skip(skip).take(take);
    let entities = page.map(|entity| listed(world, entity)).collect();
    let list = EntityList {
        total,
        entities,
        types: world.types_revision(),
    };
    Ok(serde_json::to_string(&list).expect("a list serializes"))
}

/// The page of the assets under `cx`'s root that `assets` asks for, and the orphaned ID files, as
/// the asset database holds them.
///
/// Fails with `engine.no_root` for a game that names no asset root, and with `asset.unlisted`,
/// naming the root, if the root couldn't be listed.
fn assets(
    cx: &Context,
    kind: Option<AssetKind>,
    skip: usize,
    take: usize,
) -> Result<String, Failure> {
    let Some(root) = &cx.root else {
        let message = "the game names no asset root, which App::root or App::project gives it";
        return Err(Failure::new("engine.no_root", message));
    };
    let root = root.display().to_string();
    let catalog = cx.catalog();
    if let Some(error) = catalog.unlisted() {
        let message = format!("can't scan the asset root, {root}: {error}");
        return Err(Failure::new("asset.unlisted", message));
    }
    let of_kind = |of: AssetKind| kind.is_none_or(|kind| of == kind);
    let total = catalog
        .listed()
        .filter(|(_, asset)| of_kind(asset.kind))
        .count();
    let page = (catalog.listed())
        .filter(|(_, asset)| of_kind(asset.kind))
        .skip(skip)
        .take(take);
    let assets = page.map(|(path, asset)| ListedAsset {
        id: asset.id.map(|id| id.to_string()),
        path: path.to_owned(),
        kind: asset.kind,
        loaded: cx.has_added(path),
        problem: (asset.problem.as_ref())
            .map(|problem| Failure::new(problem.code, &*problem.message)),
    });
    let orphans = (catalog.orphans().iter())
        .filter(|&&(_, kind, _)| of_kind(kind))
        .map(|(path, kind, id)| ListedOrphan {
            path: path.to_string(),
            kind: *kind,
            id: id.map(|id| id.to_string()),
        });
    let list = AssetList {
        total,
        assets: assets.collect(),
        orphans: orphans.collect(),
    };
    Ok(serde_json::to_string(&list).expect("a list serializes"))
}

/// Living `entity` as `entities` lists it.
fn listed(world: &World, entity: Entity) -> Listed {
    let id = |entity| world.id(entity).map(|id| id.to_string());
    let parent = world.parent(entity).ok().flatten();
    let components = world
        .components_of(entity)
        .expect("a listed entity is alive");
    let children = world.children(entity).expect("a listed entity is alive");
    Listed {
        id: id(entity).expect("a listed entity is alive"),
        name: world.name(entity).map(str::to_owned),
        parent: parent.and_then(id),
        children: children
            .map(|child| id(child).expect("a child is alive"))
            .collect(),
        components: components.map(str::to_owned).collect(),
    }
}

/// The entity of ID `id` in `world`, as `entity` answers it, its `bounds` by the extents `meshes`
/// keeps.
fn entity(world: &World, meshes: &Meshes, id: EntityId) -> Result<String, Failure> {
    let entity = found(world, id)?;
    let about = |error: Error| failure(&error);
    let mut out = String::from("{\"id\":");
    json::string(&mut out, &id.to_string());
    out.push_str(",\"name\":");
    match world.name(entity) {
        Some(name) => json::string(&mut out, name),
        None => out.push_str("null"),
    }
    out.push_str(",\"parent\":");
    match world
        .parent(entity)
        .map_err(about)?
        .and_then(|up| world.id(up))
    {
        Some(up) => json::string(&mut out, &up.to_string()),
        None => out.push_str("null"),
    }
    out.push_str(",\"children\":[");
    for (i, child) in world.children(entity).map_err(about)?.enumerate() {
        if i > 0 {
            out.push(',');
        }
        let child = world.id(child).expect("a child is alive");
        json::string(&mut out, &child.to_string());
    }
    out.push_str("],\"components\":{");
    for (i, name) in world.components_of(entity).map_err(about)?.enumerate() {
        if i > 0 {
            out.push(',');
        }
        json::string(&mut out, name);
        out.push(':');
        let value = world.get_field(entity, name, "").map_err(about)?;
        json::write(&mut out, &value, world);
    }
    // Where the space its transform is in sits: its parent's pose.
    let space = match world.parent(entity).map_err(about)? {
        Some(parent) => world.pose(parent).map_err(about)?,
        None => Pose::IDENTITY,
    };
    out.push_str("},\"space\":{\"matrix\":");
    json::float64s(&mut out, &DMat4::from(space.matrix).to_cols_array());
    out.push_str(",\"rotation\":");
    json::floats(&mut out, &space.rotation.to_array());
    out.push('}');
    if let Some((center, radius)) = bounds::of(world, meshes, entity) {
        out.push_str(",\"bounds\":{\"center\":");
        json::float64s(&mut out, &center.to_array());
        out.push_str(",\"radius\":");
        json::float32(&mut out, radius as f32);
        out.push('}');
    }
    // An instance whose prefab's text no longer loads gives none, as its load warned.
    if let Some(instanced) = instanced(world, entity) {
        out.push_str(",\"instance\":");
        out.push_str(&serde_json::to_string(&instanced).expect("an instance serializes"));
    }
    out.push('}');
    Ok(out)
}

/// Saves the entity of ID `id` and its descendants as a prefab at `path`, as `save_prefab` asks,
/// as [`Context::save_prefab`] does, for its link to be offered.
///
/// Fails with `api.no_scene` in a game that runs from its package, which reads no file under its
/// root, and in a networked one, whose files would part from the session's others', as `scene`'s
/// `save` does; while a copy of the world as edited plays, whose entities would be saved and
/// linked, though `stop` takes the copy away; and as `Context::save_prefab` does.
fn save_prefab(
    cx: &mut Context,
    stack: &Stack,
    id: EntityId,
    path: Option<PathBuf>,
) -> Result<crate::prefab::Saved> {
    let why = if cx.reads_package() {
        Some("it runs from its package, which holds no files under its root to write")
    } else if stack.is_peer() {
        Some("the run is networked, and its prefabs would part from the session's others'")
    } else if cx.edited.is_some() {
        Some(
            "a copy of the world as edited plays, whose entities `stop` takes away, the link \
             with them: stop it first",
        )
    } else {
        None
    };
    if let Some(why) = why {
        let message = format!("the game can't save a prefab: {why}");
        return Err(Error::new("api.no_scene", message));
    }
    cx.save_prefab(id, path.as_deref())
}

/// What of `entity`, which is alive, differs from its prefab, as `entity` answers it, if it's an
/// instance's, as [`World::overrides`] finds it; `None` for any other, or one whose prefab's text no
/// longer loads.
pub(crate) fn instanced(world: &World, entity: Entity) -> Option<gg_api::Instanced> {
    let overrides = world.overrides(entity).ok()??;
    let assets = world.assets();
    let link = world.get::<PrefabRoot>(entity);
    Some(gg_api::Instanced {
        root: overrides.root.to_string(),
        prefab: assets.prefab_path(overrides.prefab).to_owned(),
        link: link.map(|&PrefabRoot(prefab)| assets.prefab_path(prefab).to_owned()),
        overrides: overrides.fields.into_iter().collect(),
        added: overrides.added,
    })
}

/// The field at `path` of the component `component` of the entity of ID `id` in `world`, as `get`
/// answers it.
fn field(world: &World, id: EntityId, component: &str, path: &str) -> Result<String, Failure> {
    let entity = found(world, id)?;
    let value = world
        .get_field(entity, component, path)
        .map_err(|error| failure(&error))?;
    let mut out = String::new();
    json::write(&mut out, &value, world);
    Ok(out)
}

/// The textures, models, fonts and prefabs that the values of the operations `given` asks for name
/// that
/// none loaded has, as [`operations`] reads them in `world`: which a client's edit names, to load
/// before it's read.
fn assets_named(world: &World, given: &[gg_api::Operation]) -> Vec<Asset<String>> {
    let gathering = Gathering {
        world,
        missing: RefCell::default(),
    };
    let _ = operations(world, &gathering, given.to_vec());
    gathering.missing.into_inner()
}

/// What reads values as a world does, `world`, but for a texture, a model, a font or a prefab that
/// none loaded has, which it keeps the name of, reading on as though it were the engine's white
/// texture, or the first model, font or prefab.
struct Gathering<'w> {
    world: &'w World,
    missing: RefCell<Vec<Asset<String>>>,
}

impl Gathering<'_> {
    /// Keeps `asset`, which none loaded has, unless it's kept.
    fn missed(&self, asset: Asset<&str>) {
        let mut missing = self.missing.borrow_mut();
        if !missing.iter().any(|missed| missed.as_deref() == asset) {
            missing.push(asset.owned());
        }
    }
}

impl json::Names for Gathering<'_> {
    fn types(&self) -> &reflect::Types {
        self.world.types()
    }

    fn id(&self, entity: Entity) -> Option<EntityId> {
        self.world.id(entity)
    }

    fn name(&self, texture: Texture) -> Option<&str> {
        self.world.assets().name(texture)
    }

    fn entity(&self, id: EntityId) -> Result<reflect::Value> {
        json::Names::entity(self.world, id)
    }

    fn texture(&self, name: &str) -> Result<reflect::Value> {
        if let Some(texture) = self.world.assets().texture(name) {
            return Ok(reflect::Value::Texture(texture));
        }
        self.missed(Asset::Texture(name));
        Ok(reflect::Value::Texture(Texture::WHITE))
    }

    fn model_name(&self, model: Model) -> Option<&str> {
        Some(self.world.assets().model_name(model))
    }

    fn model(&self, name: &str) -> Result<reflect::Value> {
        if let Some(model) = self.world.assets().model(name) {
            return Ok(reflect::Value::Model(model));
        }
        self.missed(Asset::Model(name));
        Ok(reflect::Value::Model(Model::new(0)))
    }

    fn font_name(&self, font: Font) -> Option<&str> {
        Some(self.world.assets().font_name(font))
    }

    fn font(&self, name: &str) -> Result<reflect::Value> {
        if let Some(font) = self.world.assets().font(name) {
            return Ok(reflect::Value::Font(font));
        }
        self.missed(Asset::Font(name));
        Ok(reflect::Value::Font(Font::new(0)))
    }

    fn prefab_name(&self, prefab: Prefab) -> Option<&str> {
        Some(self.world.assets().prefab_name(prefab))
    }

    fn prefab(&self, name: &str) -> Result<reflect::Value> {
        if let Some(prefab) = self.world.assets().prefab(name) {
            return Ok(reflect::Value::Prefab(prefab));
        }
        self.missed(Asset::Prefab(name));
        Ok(reflect::Value::Prefab(Prefab::new(0)))
    }
}

/// The engine's operations `given` asks for, their values read by the types of `world`'s components
/// and the fields they go in, as [`json::read`] reads them with `names`.
///
/// Fails, naming the operation, with `ecs.bad_id` for an ID that isn't one, `reflect.unknown_type`
/// for a component type that isn't registered, as a path to a field it hasn't fails, and as
/// `json::read` fails for a value.
fn operations(
    world: &World,
    names: &dyn json::Names,
    given: Vec<gg_api::Operation>,
) -> Result<Vec<Operation>, Failure> {
    let count = given.len();
    let operations = given.into_iter().enumerate().map(|(i, operation)| {
        let read = match operation {
            gg_api::Operation::Reparent {
                id,
                parent,
                in_place: true,
            } => reparent_in_place(world, &id, parent.as_deref()),
            operation => operation_of(world, names, operation).map(|operation| vec![operation]),
        };
        read.map_err(|error| {
            let failure = failure(&error);
            if count == 1 {
                return failure;
            }
            Failure::new(
                failure.code,
                format!("operation {}: {}", i + 1, failure.message),
            )
        })
    });
    let operations: Vec<Vec<Operation>> = operations.collect::<Result<_, _>>()?;
    Ok(operations.into_iter().flatten().collect())
}

/// The operations a `reparent` `in_place` of entity `id` under `parent` asks for, as
/// [`World::reparent_in_place`] finds them in `world` as the request comes, which an edit applies
/// at once, outside a lockstep session.
fn reparent_in_place(world: &World, id: &str, parent: Option<&str>) -> Result<Vec<Operation>> {
    let parent = parent.map(str::parse).transpose()?;
    Ok(world.reparent_in_place(id.parse()?, parent))
}

/// The engine's operation `given` asks for, as [`operations`] reads it.
fn operation_of(
    world: &World,
    names: &dyn json::Names,
    given: gg_api::Operation,
) -> Result<Operation> {
    use gg_api::Operation as Given;

    let id = |text: &str| text.parse::<EntityId>();
    let maybe = |text: Option<String>| text.as_deref().map(id).transpose();
    // The type of `component`'s field at `path`, as its description gives it.
    let field = |component: &str, path: &str| -> Result<Type> {
        let types = world.types();
        let index = types.find(component).ok_or_else(|| {
            let names = world.component_types().map(|name| format!("`{name}`"));
            let message = format!(
                "no component type is named `{component}`; the world's are {}",
                reflect::list(names)
            );
            Error::new("reflect.unknown_type", message)
        })?;
        let resolved = reflect::resolve(types, &Type::Named(index), path)?;
        Ok(resolved.ty)
    };
    let value = |component: &str, path: &str, json: &Value| {
        let ty = field(component, path)?;
        json::read(json, &ty, names)
            .map_err(|error| reflect::within(error, &reflect::join(component, path)))
    };
    Ok(match given {
        Given::Create {
            id: made,
            parent,
            components,
        } => Operation::Create {
            entity: maybe(made)?,
            parent: maybe(parent)?,
            components: components
                .iter()
                .map(|(name, json)| Ok((Ident::from(name.clone()), value(name, "", json)?)))
                .collect::<Result<_>>()?,
        },
        Given::Duplicate { id: of, copies } => Operation::Duplicate {
            entity: id(&of)?,
            copies: copies.iter().map(|copy| id(copy)).collect::<Result<_>>()?,
        },
        Given::Delete { id: of } => Operation::Delete { entity: id(&of)? },
        Given::Add {
            id: of,
            component,
            value: given,
        } => Operation::Add {
            entity: id(&of)?,
            value: match given {
                Some(json) => value(&component, "", &json)?,
                None => world.default_of(&component)?,
            },
            component: component.into(),
        },
        Given::Remove { id: of, component } => Operation::Remove {
            entity: id(&of)?,
            component: component.into(),
        },
        Given::Set {
            id: of,
            component,
            path,
            value: json,
        } => Operation::Set {
            entity: id(&of)?,
            value: value(&component, &path, &json)?,
            component: component.into(),
            path,
        },
        Given::Reparent { id: of, parent, .. } => Operation::Reparent {
            entity: id(&of)?,
            parent: maybe(parent)?,
        },
        Given::Unlink { id: of } => Operation::Unlink { entity: id(&of)? },
        Given::Revert {
            id: of,
            component,
            path,
        } => {
            // Refused as a set of the path is, for a type or a field there's none of.
            field(&component, &path)?;
            Operation::Revert {
                entity: id(&of)?,
                component: component.into(),
                path,
            }
        }
    })
}

/// `operations` in words, as an edit is reported.
fn words(operations: &[Operation]) -> Vec<String> {
    operations.iter().map(ToString::to_string).collect()
}

/// The living entity of ID `id` in `world`.
///
/// Fails with `ecs.no_entity` if none has it.
fn found(world: &World, id: EntityId) -> Result<Entity, Failure> {
    world.entity(id).ok_or_else(|| {
        let message = format!("no living entity has ID {id}");
        Failure::new("ecs.no_entity", message)
    })
}

/// `error` as the API answers it.
fn failure(error: &Error) -> Failure {
    Failure::new(error.code(), format!("{error:#}"))
}

/// Sets a part of the look, `part`, whose setter `by` names, as `change` asks, if it asks, for
/// client `client`: `null` gives it back to the game, which no client sets. Returns whether its
/// value or its setter changed.
fn set_part<T: PartialEq>(
    part: &mut Option<T>,
    by: &mut Option<String>,
    change: Option<Option<T>>,
    client: &str,
) -> bool {
    let Some(value) = change else {
        return false;
    };
    let setter = value.is_some().then(|| client.to_owned());
    let changed = *part != value || *by != setter;
    (*part, *by) = (value, setter);
    changed
}

/// `image`, which a frame listed, as `images` answers it.
fn frame_image(image: &gg_renderer::FrameImage) -> gg_api::FrameImage {
    use gg_api::ImageKind as Api;
    use gg_renderer::ImageKind;

    gg_api::FrameImage {
        name: image.name.to_owned(),
        kind: match image.kind {
            ImageKind::Color => Api::Color,
            ImageKind::Hdr => Api::Hdr,
            ImageKind::Ids => Api::Ids,
            ImageKind::Motion => Api::Motion,
            ImageKind::Visibility => Api::Visibility,
            ImageKind::Depth => Api::Depth,
            ImageKind::Scalar => Api::Scalar,
        },
        size: image.size,
    }
}

/// `viewpoint` as the tool API gives one.
pub(crate) fn api_viewpoint(viewpoint: &Viewpoint) -> gg_api::Viewpoint {
    let projection = match viewpoint.projection {
        Projection::Orthographic { height, near, far } => {
            gg_api::Projection::Orthographic { height, near, far }
        }
        Projection::Perspective { fov, near } => gg_api::Projection::Perspective { fov, near },
    };
    gg_api::Viewpoint {
        position: viewpoint.position.to_array(),
        rotation: viewpoint.rotation.to_array(),
        projection,
    }
}

/// The viewpoint the tool API's `given` is.
fn engine_viewpoint(given: &gg_api::Viewpoint) -> Viewpoint {
    Viewpoint {
        position: DVec3::from_array(given.position),
        rotation: Quat::from_array(given.rotation),
        projection: engine_projection(given.projection),
    }
}

/// The projection the tool API's `given` is.
fn engine_projection(given: gg_api::Projection) -> Projection {
    match given {
        gg_api::Projection::Orthographic { height, near, far } => {
            Projection::Orthographic { height, near, far }
        }
        gg_api::Projection::Perspective { fov, near } => Projection::Perspective { fov, near },
    }
}

/// Accepts connections on `listener`, serving each on threads of its own, as [`connection`] does,
/// until the process ends; but while [`MOST_SAYING_HELLO`] are saying hello, it closes each that
/// comes, warning `api.too_many_hellos` as it starts to, once until one is served again.
fn accept(
    listener: &TcpListener,
    token: &str,
    welcome: &str,
    to_main: &Sender<Incoming>,
    shared: &Arc<Shared>,
) {
    let saying_hello = Arc::new(AtomicUsize::new(0));
    let mut refusing = false;
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                debug!(code = "api.accept", "Can't accept a connection: {error}");
                // An error that comes again, such as running out of handles, mustn't spin.
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        // This thread alone adds to the count, so it can't pass the most between the two.
        if saying_hello.load(Ordering::Relaxed) >= MOST_SAYING_HELLO {
            if !mem::replace(&mut refusing, true) {
                warn!(
                    code = "api.too_many_hellos",
                    most = MOST_SAYING_HELLO,
                    "Refusing connections while {MOST_SAYING_HELLO} are saying hello, as one that \
                     never says it holds a thread until it's dropped"
                );
            }
            continue;
        }
        refusing = false;
        saying_hello.fetch_add(1, Ordering::Relaxed);
        let place = SayingHello(saying_hello.clone());
        let (token, welcome) = (token.to_owned(), welcome.to_owned());
        let (to_main, shared) = (to_main.clone(), shared.clone());
        let serving = thread::Builder::new()
            .name("gg-api-read".to_owned())
            .spawn(move || connection(stream, &token, &welcome, &to_main, &shared, place));
        if let Err(error) = serving {
            debug!(code = "api.accept", "Can't serve a connection: {error}");
        }
    }
}

/// A connection's place among those saying hello, which [`MOST_SAYING_HELLO`] caps, given up as it
/// drops.
struct SayingHello(Arc<AtomicUsize>);

impl Drop for SayingHello {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// How many requests a connection has in flight, read and not yet answered on the wire, which
/// [`gg_api::MOST_IN_FLIGHT`] caps.
#[derive(Default)]
struct InFlight {
    count: Mutex<usize>,
    /// Notified as a request lands, its place given up.
    landed: Condvar,
}

impl InFlight {
    /// A place among those in flight, once one is free.
    fn take(self: &Arc<Self>) -> Flight {
        let mut count = self.count.lock().expect("no thread panics holding it");
        while *count >= gg_api::MOST_IN_FLIGHT {
            count = (self.landed.wait(count)).expect("no thread panics holding it");
        }
        *count += 1;
        Flight(self.clone(), None)
    }
}

/// A request's place among those its connection has in flight, given up as it drops: once its
/// answer is written, or as it goes unanswered, as when the connection has gone; and what's told
/// then, if anything waits for it, as [`Reply::landing`] has it.
struct Flight(Arc<InFlight>, Option<Sender<()>>);

impl Drop for Flight {
    fn drop(&mut self) {
        *self.0.count.lock().expect("no thread panics holding it") -= 1;
        self.0.landed.notify_one();
        if let Some(landed) = self.1.take() {
            let _ = landed.send(());
        }
    }
}

/// Serves connection `stream`, which holds `place` among those saying hello until it has: its first
/// request must be `hello` with `token`, as [`hello`] reads it within [`HELLO_TIMEOUT`], which it
/// answers with `welcome`; then it answers `ping`, `logs`, `note` and `debug` itself, and passes the
/// rest on to the main loop through `to_main`, waking it, each answer going back through a thread
/// that writes them as they come, which starts once the `hello` has been granted. It reads a request
/// only while fewer than [`gg_api::MOST_IN_FLIGHT`] are in flight, so that a client that sends
/// faster than it reads, or than the game answers, is held back by the connection, rather than its
/// requests and answers growing without bound in the game. A line that isn't a request ends it.
fn connection(
    stream: TcpStream,
    token: &str,
    welcome: &str,
    to_main: &Sender<Incoming>,
    shared: &Shared,
    place: SayingHello,
) {
    let _ = stream.set_nodelay(true);
    let mut reader = BufReader::new(stream);
    let Some((id, client, grant)) = hello(&mut reader, token, Instant::now() + HELLO_TIMEOUT)
    else {
        return;
    };
    drop(place);
    let _ = reader.get_ref().set_read_timeout(None);
    let Ok(writing) = reader.get_ref().try_clone() else {
        return;
    };
    let (to, answers) = mpsc::channel::<Answer>();
    let writer = thread::Builder::new()
        .name("gg-api-write".to_owned())
        .spawn(move || write_answers(writing, &answers));
    if writer.is_err() {
        return;
    }
    let in_flight = Arc::new(InFlight::default());
    let reply = Reply {
        id,
        to: to.clone(),
        flight: in_flight.take(),
    };
    reply.send(Ok(welcome.to_owned()));
    let names = grant.reads("Name");
    loop {
        let flight = in_flight.take();
        let Some(request) = next_request(&mut reader) else {
            break;
        };
        let reply = Reply {
            id: request.id,
            to: to.clone(),
            flight,
        };
        let own = match request.method.as_str() {
            "logs" => Some(Capability::Read(None)),
            "note" => Some(Capability::Note),
            "debug" => Some(Capability::Debug),
            _ => None,
        };
        if let Some(Err(failure)) = own.map(|own| granted(&grant, &request.method, &[own])) {
            reply.send(Err(failure));
            continue;
        }
        if request.method == "ping" {
            // A script stopped first, which holds the main thread even while the window's
            // minimized, between whose frames it answers otherwise.
            let stopped = shared.debugger.stopped().then_some("breakpoint");
            let waiting = || Waiting::now().map(Waiting::name);
            let pulse = Pulse {
                frames: shared.frames.load(Ordering::Relaxed),
                waiting: stopped.or_else(waiting).map(str::to_owned),
            };
            reply.result(&pulse);
            continue;
        }
        if request.method == "logs" {
            reply.send(logs(&request.params, shared));
            continue;
        }
        if request.method == "note" {
            reply.send(note(&request.params, &client));
            continue;
        }
        if request.method == "debug" {
            reply.send(debug(&request.params, shared));
            continue;
        }
        let parsed = parse(&request).and_then(|call| {
            granted(&grant, &request.method, &needs(&call))?;
            Ok(call)
        });
        match parsed {
            Ok(call) => {
                let client = client.clone();
                let incoming = Incoming {
                    call,
                    client,
                    names,
                    reply,
                };
                if let Err(mpsc::SendError(incoming)) = to_main.send(incoming) {
                    let message = "the game is ending, and took nothing of the request";
                    incoming
                        .reply
                        .send(Err(Failure::new("api.closed", message)));
                    return;
                }
                shared.passed.fetch_add(1, Ordering::Release);
                if let Some(wake) = shared
                    .waker
                    .lock()
                    .expect("no thread panics holding it")
                    .as_ref()
                {
                    wake();
                }
            }
            Err(failure) => reply.send(Err(failure)),
        }
    }
}

/// What `call` needs of its connection's grant: of the world, `read` or `write` of the type it
/// names alone, or else of the whole world; or the tool API's own capability its method needs,
/// with `files` for a path to write at.
fn needs(call: &Call) -> Vec<Capability> {
    use Capability::{Files, Input, Patch, Play, Quit, Read, Scene, Snapshot, View, Write};

    match call {
        Call::Status => Vec::new(),
        Call::Pause | Call::Resume | Call::Play | Call::Stop | Call::Step(_) => vec![Play],
        Call::Scene(_) => vec![Scene],
        Call::Capture { path, .. } => [Some(View), path.as_ref().map(|_| Files)]
            .into_iter()
            .flatten()
            .collect(),
        Call::Frames { .. } => vec![View, Files],
        Call::View(_) | Call::Look(_) | Call::Pick(..) | Call::Images => vec![View],
        Call::Input(_) => vec![Input],
        Call::Get { component, .. } => vec![Read(Some(component.clone()))],
        Call::Entities { .. }
        | Call::Entity(_)
        | Call::Types
        | Call::Hash
        | Call::Cells
        | Call::Assets { .. }
        | Call::Ui
        | Call::Edits(_) => {
            vec![Read(None)]
        }
        Call::Snapshot { path, quit } => [
            Some(Snapshot),
            path.as_ref().map(|_| Files),
            quit.then_some(Quit),
        ]
        .into_iter()
        .flatten()
        .collect(),
        Call::Patch(_) => vec![Patch],
        Call::Apply(operations, _) => operations
            .iter()
            .map(|operation| match operation {
                gg_api::Operation::Add { component, .. }
                | gg_api::Operation::Remove { component, .. }
                | gg_api::Operation::Set { component, .. }
                | gg_api::Operation::Revert { component, .. } => Write(Some(component.clone())),
                _ => Write(None),
            })
            .collect(),
        Call::SavePrefab { .. } => vec![Write(None), Scene],
        Call::Undo { .. } => vec![Write(None)],
        Call::Quit => vec![Quit],
    }
}

/// Whether `grant`, a connection's, covers each of `needs`, what a request of `method` needs.
///
/// Fails with `api.ungranted`, naming the first it doesn't.
fn granted(grant: &Grant, method: &str, needs: &[Capability]) -> Result<(), Failure> {
    match needs.iter().find(|need| !grant.has(need)) {
        None => Ok(()),
        Some(need) => Err(Failure::new(
            "api.ungranted",
            format!(
                "`{method}` needs `{need}`, which the connection isn't granted: its `hello` \
                 granted `{grant}`"
            ),
        )),
    }
}

/// Whether `given` is `token`, compared in a time that doesn't depend on where they first differ,
/// so that a client can't find the token a byte at a time by timing its refusals: only their
/// lengths tell early, and the token's, 32, is no secret. `black_box` keeps the compiler from
/// ending the loop at the first difference, as far as std's best effort goes.
fn same_token(given: &str, token: &str) -> bool {
    let (given, token) = (given.as_bytes(), token.as_bytes());
    given.len() == token.len()
        && (given.iter().zip(token))
            .fold(0, |differ, (a, b)| std::hint::black_box(differ | (a ^ b)))
            == 0
}

/// What `logs`, given `params`, answers, from the events `shared` keeps, as gg-api's docs say: read
/// on the connection's own thread, which a wait holds.
///
/// Fails with `api.bad_params` for parameters it doesn't take, a `level` that isn't one, or a wait
/// of more than [`MOST_WAIT_MS`].
fn logs(params: &Value, shared: &Shared) -> Result<String, Failure> {
    let method = "logs";
    let gg_api::Logs {
        from,
        run,
        last,
        level,
        wait_ms,
    } = take(method, params)?;
    let min = match level {
        Some(level) => {
            let at = gg_api::LEVELS.iter().position(|&name| name == level);
            let at = at.ok_or_else(|| {
                let what = format!(
                    "takes a `level` of `error`, `warn`, `info`, `debug` or `trace`, not `{level}`"
                );
                bad_params(method, what)
            })?;
            LEVELS[at]
        }
        None => Level::TRACE,
    };
    if wait_ms > MOST_WAIT_MS {
        let what = format!("waits at most {MOST_WAIT_MS} ms, not {wait_ms}");
        return Err(bad_params(method, what));
    }
    // Another run's cursor numbers another run's events, so all of this run's are new to it.
    let from = match (from, run) {
        (Some(_), Some(run)) if run != shared.run => Some(0),
        (from, _) => from,
    };
    let wait = Duration::from_millis(wait_ms);
    let read = (shared.events).read(from, last, min, wait, gg_api::MAX_LINE / 2);
    // Each line is an object of JSON already, as the log wrote it.
    let mut answer = format!(
        r#"{{"run":{},"next":{},"end":{},"missed":{},"events":["#,
        shared.run, read.next, read.end, read.missed
    );
    for (i, line) in read.lines.iter().enumerate() {
        if i > 0 {
            answer.push(',');
        }
        answer.push_str(line);
    }
    answer.push_str("]}");
    Ok(answer)
}

/// What `note`, given `params` by client `client`, answers, having logged the event it gives, as
/// gg-api's docs say: at its level, with its code, its message, `client`, and its fields as one
/// field of JSON text, `fields`, since an event's fields are flat. It's answered on the
/// connection's own thread, so that a note comes through while frames pause.
///
/// Fails with `api.bad_params` for parameters it doesn't take, as a level that isn't `info` or
/// `warn`; a code that isn't a stable code's shape, as [`is_code`] says, or is longer than
/// [`gg_api::MOST_NOTE_CODE`] bytes; an empty message; or a message or fields longer than
/// [`gg_api::MOST_NOTE`] bytes.
fn note(params: &Value, client: &str) -> Result<String, Failure> {
    let method = "note";
    let gg_api::Note {
        code,
        level,
        message,
        fields,
    } = take(method, params)?;
    let (most_code, most) = (gg_api::MOST_NOTE_CODE, gg_api::MOST_NOTE);
    if code.len() > most_code {
        let what = format!(
            "takes a code of at most {most_code} bytes, not {}",
            code.len()
        );
        return Err(bad_params(method, what));
    }
    if !is_code(&code) {
        let what = format!(
            "takes a stable code, dotted names of lowercase ASCII letters, digits and `_`, each \
             starting with a letter, such as `gg.patch_failed`, not `{code}`"
        );
        return Err(bad_params(method, what));
    }
    if message.is_empty() || message.len() > most {
        let what = format!(
            "takes a message of 1 to {most} bytes, not {}",
            message.len()
        );
        return Err(bad_params(method, what));
    }
    let fields = fields.map(|fields| Value::Object(fields).to_string());
    if let Some(given) = fields.as_ref().filter(|given| given.len() > most) {
        let what = format!(
            "takes fields of at most {most} bytes as JSON, not {}",
            given.len()
        );
        return Err(bad_params(method, what));
    }
    let (code, fields) = (code.as_str(), fields.as_deref());
    match level {
        gg_api::NoteLevel::Info => info!(code, client, fields, "{message}"),
        gg_api::NoteLevel::Warn => warn!(code, client, fields, "{message}"),
    }
    Ok("{}".to_owned())
}

/// What `debug`, given `params`, answers, as gg-api's docs say: from `shared`'s debugger, on the
/// connection's own thread, so that it answers while a script has stopped and the main thread waits
/// there, and which a wait holds. Breakpoints and a pause wake the main loop, which places them, or
/// steps to the pause, as a frame starts.
///
/// Fails with `api.bad_params` for parameters it doesn't take, a script's path that's empty, a line
/// of 0 or a wait of more than [`MOST_WAIT_MS`]; and with `script.not_stopped` for a continue or a
/// step while no script has stopped.
fn debug(params: &Value, shared: &Shared) -> Result<String, Failure> {
    let method = "debug";
    let gg_api::Debug {
        breakpoints,
        action,
        wait_ms,
    } = take(method, params)?;
    if wait_ms > MOST_WAIT_MS {
        let what = format!("waits at most {MOST_WAIT_MS} ms, not {wait_ms}");
        return Err(bad_params(method, what));
    }
    if let Some(given) = &breakpoints {
        if given.script.is_empty() {
            return Err(bad_params(
                method,
                "takes a script's path from the project's root",
            ));
        }
        if given.lines.contains(&0) {
            return Err(bad_params(method, "takes lines from 1"));
        }
    }
    let debugger = &shared.debugger;
    let wake = breakpoints.is_some() || action == Some(gg_api::DebugAction::Pause);
    if let Some(gg_api::Breakpoints { script, lines }) = breakpoints {
        debugger.set(script, lines);
    }
    if let Some(action) = action {
        debugger.act(action).map_err(|error| failure(&error))?;
    }
    if wake
        && let Some(wake) = shared
            .waker
            .lock()
            .expect("no thread panics holding it")
            .as_ref()
    {
        wake();
    }
    let answer = debugger.answer(Duration::from_millis(wait_ms));
    Ok(serde_json::to_string(&answer).expect("an answer serializes"))
}

/// Whether `code` has a stable code's shape: two or more names, joined by dots, each of lowercase
/// ASCII letters, digits and `_`, starting with a letter, as `gg.patch_failed`.
fn is_code(code: &str) -> bool {
    let name = |name: &str| {
        name.starts_with(|c: char| c.is_ascii_lowercase())
            && (name.chars()).all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    code.contains('.') && code.split('.').all(name)
}

/// The ID, the client and the grant of the `hello` that `reader`, a new connection's, reads by
/// `deadline`, as [`read_hello`] does, if it gives `token`. A first request that isn't such a
/// `hello`, or one whose grant names a capability there's none of, is answered with the failure,
/// and `None` returned.
fn hello(
    reader: &mut BufReader<TcpStream>,
    token: &str,
    deadline: Instant,
) -> Option<(u64, Arc<str>, Grant)> {
    let hello = read_hello(reader, deadline)?;
    let given = (hello.method == "hello")
        .then(|| serde_json::from_value::<Hello>(hello.params).ok())
        .flatten();
    let granted = match given.filter(|given| same_token(&given.token, token)) {
        None => {
            let message = "a connection's first request is `hello`, with the session file's token";
            Err(Failure::new("api.unauthorized", message))
        }
        Some(Hello { client, grant, .. }) => match grant.map(Grant::parse) {
            None => Ok((client, Grant::all())),
            Some(Ok(grant)) => Ok((client, grant)),
            Some(Err(error)) => Err(Failure::new(error.code(), format!("{error:#}"))),
        },
    };
    match granted {
        Ok((client, grant)) => Some((hello.id, client.as_deref().unwrap_or(UNNAMED).into(), grant)),
        Err(failure) => {
            // Here, as no thread writes the connection's answers until its `hello` is granted. It's
            // the first line, which the hello's bound keeps to tens of KiB even escaped, within
            // what the loopback's buffers take, so it's written without the client reading; and
            // within a timeout, should they not take it, so that a client that never reads can't
            // keep its place among those saying hello.
            let line = gg_api::answer_line(hello.id, Err(&failure));
            let _ = reader.get_ref().set_write_timeout(Some(HELLO_TIMEOUT));
            let _ = reader.get_ref().write_all(line.as_bytes());
            None
        }
    }
}

/// The `hello` that `reader`, a new connection's, reads: its first line, of at most
/// [`gg_api::MAX_HELLO`] bytes, its line break aside, whole by `deadline`, each read waiting only
/// for the time left, so that a client sending a byte at a time can't hold the connection longer.
/// `None` if it doesn't come so, the connection ends or breaks first, or it isn't a request; a line
/// too long is refused once it's seen to be, the rest unread.
fn read_hello(reader: &mut BufReader<TcpStream>, deadline: Instant) -> Option<Request> {
    let mut line = Vec::new();
    loop {
        // A timeout of zero is refused, as it would wait for good.
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        reader.get_ref().set_read_timeout(Some(left)).ok()?;
        let read = match reader.fill_buf() {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        if read.is_empty() {
            return None;
        }
        let (taken, whole) = match read.iter().position(|&byte| byte == b'\n') {
            Some(at) => (at + 1, true),
            None => (read.len(), false),
        };
        if line.len() + taken - usize::from(whole) > gg_api::MAX_HELLO {
            debug!(
                code = "api.bad_request",
                "A tool's hello is longer than {} bytes, so its connection closes",
                gg_api::MAX_HELLO
            );
            return None;
        }
        line.extend_from_slice(&read[..taken]);
        reader.consume(taken);
        if whole {
            return request(&line);
        }
    }
}

/// The next request `reader` reads: `None` once the connection ends or breaks, or at a line that
/// isn't a request.
fn next_request(reader: &mut BufReader<TcpStream>) -> Option<Request> {
    let line = gg_api::read_line(reader).ok()??;
    request(line.as_bytes())
}

/// The request `line` is, which may end in its line break; `None` if it isn't one.
fn request(line: &[u8]) -> Option<Request> {
    serde_json::from_slice(line)
        .inspect_err(|error| {
            debug!(
                code = "api.bad_request",
                "A tool sent what isn't a request, so its connection closes: {error}"
            );
        })
        .ok()
}

/// Writes the lines `answers` brings to `stream`, each request's place in flight going once its
/// line is written, until the connection breaks or nothing more can come.
fn write_answers(mut stream: TcpStream, answers: &Receiver<Answer>) {
    for (line, _flight) in answers {
        if stream.write_all(line.as_bytes()).is_err() {
            return;
        }
    }
}

/// Writes the captures `pngs` brings, each as a PNG, and each `frames`' frames, as PNGs numbered
/// from 1, `frame-0001.png` and on, answering each once its PNGs are written, or as the first
/// fails, until the API drops.
fn write_pngs(pngs: &Receiver<Writing>) {
    for writing in pngs {
        let png = match writing {
            Writing::Capture(png) => png,
            Writing::Frames { dir, shots, reply } => {
                let framed = (shots.into_iter().enumerate())
                    .map(|(at, back)| {
                        let path = dir.join(format!("frame-{:04}.png", at + 1));
                        write_png(&path, &back.shot, false)?;
                        let [width, height] = back.shot.size;
                        Ok(FrameShot {
                            path,
                            width,
                            height,
                            ticks: back.ticks,
                            streaming: back.streaming,
                            viewpoint: api_viewpoint(&back.viewpoint),
                        })
                    })
                    .collect::<Result<Vec<_>>>();
                match framed {
                    Ok(frames) => reply.result(&Framed { frames }),
                    Err(error) => reply.send(Err(failure(&error))),
                }
                continue;
            }
        };
        let [width, height] = png.shot.size;
        let written = (png.path.as_deref())
            .map_or(Ok(()), |path| write_png(path, &png.shot, png.named))
            .map(|()| Captured {
                path: png.path,
                width,
                height,
                pixel: png.at.and_then(|at| pixel_at(&png.shot, at)),
                ticks: png.ticks,
                streaming: png.streaming,
            });
        match written {
            Ok(captured) => png.reply.result(&captured),
            Err(error) => png.reply.send(Err(failure(&error))),
        }
    }
}

/// `shot` cropped to `rect`, `[x, y, width, height]` from its top left, where they overlap, or whole
/// without one; `None` where they don't.
fn crop(shot: &Arc<Shot>, rect: Option<[u32; 4]>) -> Option<Arc<Shot>> {
    let Some([x, y, width, height]) = rect else {
        return Some(shot.clone());
    };
    let [shot_width, shot_height] = shot.size;
    let right = x.saturating_add(width).min(shot_width);
    let bottom = y.saturating_add(height).min(shot_height);
    if x >= right || y >= bottom {
        return None;
    }
    let row = (right - x) as usize * 4;
    let mut pixels = Vec::with_capacity(row * (bottom - y) as usize);
    for line in y..bottom {
        let start = (line as usize * shot_width as usize + x as usize) * 4;
        pixels.extend_from_slice(&shot.pixels[start..start + row]);
    }
    Some(Arc::new(Shot {
        size: [right - x, bottom - y],
        pixels,
    }))
}

/// The RGBA bytes of the pixel `at` falls in, `[x, y]` from the top left of `shot`'s view, as the
/// PNG has them; `None` outside the view.
fn pixel_at(shot: &Shot, at: [f64; 2]) -> Option<[u8; 4]> {
    let [width, height] = shot.size;
    let [x, y] = at.map(f64::floor);
    // Negative and not finite positions were refused with the request; this keeps an index in range.
    if !(0.0..f64::from(width)).contains(&x) || !(0.0..f64::from(height)).contains(&y) {
        return None;
    }
    let index = y as usize * width as usize + x as usize;
    shot.pixels.as_chunks::<4>().0.get(index).copied()
}

/// Writes the snapshots `taken` brings, each a file of app `app`'s, whose layers are `layers`, as
/// [`snapshot::write`] writes one, whole or not at all, making the `snapshots` directory beside the
/// executable if it goes there; and answers each, until the API drops.
fn write_snapshots(taken: &Receiver<Taken>, app: &str, layers: &[String]) {
    let layers: Vec<&str> = layers.iter().map(String::as_str).collect();
    for Taken {
        world,
        scenes,
        cached,
        open,
        scripts,
        mods,
        run,
        paused,
        ticks,
        path,
        named,
        reply,
        written: told,
    } in taken
    {
        let of = snapshot::Of {
            world: &world,
            scenes: &scenes,
            cached: &cached,
            open: &open,
            scripts: &scripts,
            mods: &mods,
            seen: None,
        };
        let written = snapshot::write_paused(&of, &run, paused, app, &layers, ticks);
        let written = written.and_then(|text| {
            if path.parent() == Some(crate::beside_exe("snapshots").as_path()) {
                fs::create_dir_all(crate::beside_exe("snapshots")).map_err(|error| {
                    let message = format!("can't make the directory of {}", path.display());
                    Error::new("snapshot.file", message).with_source(error)
                })?;
            }
            snapshot::save(&path, &text)?;
            if named && let Some(dir) = path.parent() {
                prune(dir, "snapshot", KEPT_SNAPSHOTS);
            }
            Ok(text.len() as u64)
        });
        let whole = written.is_ok();
        match written {
            Ok(bytes) => reply.result(&Snapshotted {
                path,
                ticks,
                entities: world.entities(None).map_or(0, |entities| entities.len()),
                bytes,
            }),
            Err(error) => reply.send(Err(failure(&error))),
        }
        if let Some(told) = told {
            let _ = told.send(whole);
        }
    }
}

/// Writes `shot` as an 8-bit RGB PNG at `path`, whole or not at all, making the `captures`
/// directory beside the executable if it's there: the view's pixels are sRGB-encoded, as a PNG's
/// are taken to be without a chunk that says otherwise, and its alpha goes, since the view shows
/// opaque.
///
/// Fails with `api.capture_file` if it can't.
fn write_png(path: &Path, shot: &Shot, named: bool) -> Result<()> {
    let failed = |error: Box<dyn std::error::Error + Send + Sync>| {
        let message = format!("can't write the capture {}", path.display());
        Error::new("api.capture_file", message).with_source(error)
    };
    let [width, height] = shot.size;
    let (pixels, _) = shot.pixels.as_chunks::<4>();
    let rgb: Vec<u8> = (pixels.iter())
        .flat_map(|&[r, g, b, _]| [r, g, b])
        .collect();
    let mut data = Vec::new();
    let mut encoder = png::Encoder::new(&mut data, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let encoded = encoder.write_header().and_then(|mut writer| {
        writer.write_image_data(&rgb)?;
        writer.finish()
    });
    encoded.map_err(|error| failed(error.into()))?;
    let captures = crate::beside_exe("captures");
    if path.parent() == Some(captures.as_path()) {
        fs::create_dir_all(&captures).map_err(|error| failed(error.into()))?;
    }
    crate::dirs::write_whole(path, &data).map_err(|error| failed(error.into()))?;
    if named && let Some(dir) = path.parent() {
        prune(dir, "png", KEPT_CAPTURES);
    }
    Ok(())
}

/// Removes the files of directory `dir` that the engine named, as [`engine_named`] tells, whose
/// extension is `extension`, but the newest `keep`, by when they were last written: those a client
/// named, and the directories in it, are left alone, and a file that can't be removed, as one a
/// client reads on Windows, stays, for the next to try again.
fn prune(dir: &Path, extension: &str, keep: usize) {
    let entries = fs::read_dir(dir).into_iter().flatten().flatten();
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| engine_named(&entry.file_name().to_string_lossy(), extension))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    // The newest first, two written at once by their names.
    files.sort_by(|a, b| b.cmp(a));
    for (_, path) in files.into_iter().skip(keep) {
        let _ = fs::remove_file(path);
    }
}

/// Whether a file named `name` is one the engine named for a capture or a snapshot given no path,
/// whose extension is `extension`: a run's session file's stem, its app and its process, then the
/// capture's or snapshot's number, as `sandbox-4242-7.png`.
fn engine_named(name: &str, extension: &str) -> bool {
    let Some(stem) = name
        .strip_suffix(extension)
        .and_then(|stem| stem.strip_suffix('.'))
    else {
        return false;
    };
    let mut parts = stem.rsplitn(3, '-');
    let numbered = |part: Option<&str>| {
        part.is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    numbered(parts.next())
        && numbered(parts.next())
        && parts.next().is_some_and(|app| !app.is_empty())
}

/// The look `given`, method `method`'s, checked, its viewpoint's rotation normalized, and the
/// passes it leaves out in [`gg_api::Pass::ALL`]'s order, each once, an empty list given back.
///
/// Fails with `api.bad_params`, saying why, for a viewpoint that isn't finite, whose rotation has
/// no length, or whose projection can't project, as [`Projection`] says; and for an outline with
/// an ID that isn't an entity's, or whose color isn't finite, or whose width isn't finite and positive.
fn look(method: &str, mut given: gg_api::Look) -> Result<gg_api::Look, Failure> {
    if let Some(Some(viewpoint)) = &mut given.viewpoint {
        checked_viewpoint(method, viewpoint)?;
    }
    if let Some(Some(outline)) = &given.outline {
        for id in &outline.ids {
            if let Err(error) = id.parse::<EntityId>() {
                let what = format!("takes an outline of entities' IDs: {error}");
                return Err(bad_params(method, what));
            }
        }
        if !outline.color.iter().all(|channel| channel.is_finite()) {
            return Err(bad_params(method, "takes an outline whose color is finite"));
        }
        if !(outline.width.is_finite() && outline.width > 0.0) {
            let what = format!(
                "takes an outline whose width is finite and positive, not {}",
                outline.width
            );
            return Err(bad_params(method, what));
        }
    }
    if let Some(Some(show)) = &given.show {
        shown(method, show)?;
    }
    // A set, whatever the order, so that the same passes again aren't a change; and none left out
    // is the game's own.
    if let Some(skip) = &mut given.skip {
        if let Some(passes) = skip.as_mut() {
            passes.sort_unstable();
            passes.dedup();
        }
        if skip.as_ref().is_some_and(Vec::is_empty) {
            *skip = None;
        }
    }
    Ok(given)
}

/// Checks a viewpoint method `method` takes, normalizing its rotation.
///
/// Fails with `api.bad_params`, saying why, for one that isn't finite, whose rotation has no
/// length, or whose projection can't project, as [`Projection`] says.
fn checked_viewpoint(method: &str, viewpoint: &mut gg_api::Viewpoint) -> Result<(), Failure> {
    if !viewpoint.is_finite() {
        let what = "takes a viewpoint whose numbers are finite";
        return Err(bad_params(method, what));
    }
    let Some(rotation) = Vec4::from_array(viewpoint.rotation).try_normalize() else {
        let what = "takes a viewpoint whose rotation is a quaternion with a length, which is \
                    normalized";
        return Err(bad_params(method, what));
    };
    viewpoint.rotation = rotation.to_array();
    let projection = engine_projection(viewpoint.projection);
    if !projection.projects() {
        let what = format!(
            "takes a projection that projects: orthographic, of a positive height, between a near \
             and a far that differ; or perspective, of a field of view more than 0 and less than \
             π, from a positive near; not {projection:?}"
        );
        return Err(bad_params(method, what));
    }
    Ok(())
}

/// Checks what a `look` or a `capture` shows in place of the view's light: an image's name that's
/// neither empty nor longer than [`gg_api::MOST_IMAGE_NAME`] bytes.
fn shown(method: &str, show: &gg_api::Shown) -> Result<(), Failure> {
    match show {
        gg_api::Shown::Image(name) if name.is_empty() || name.len() > gg_api::MOST_IMAGE_NAME => {
            let what = format!(
                "takes an image's name of 1 to {} bytes, not {} bytes",
                gg_api::MOST_IMAGE_NAME,
                name.len()
            );
            Err(bad_params(method, what))
        }
        _ => Ok(()),
    }
}

/// The call `request` makes of the main loop.
///
/// Fails with `api.unknown_method` for a method there's none of, `api.bad_params` for parameters
/// it doesn't take, a `look` that can't be, as [`look`] says, a `scene` whose path is missing,
/// given where it's none, or not a `.ron` file's, or whose `discard` is given to `save`, a
/// `save_prefab` whose path isn't a `.prefab` file's, or a `frames` whose `dir` isn't an absolute
/// path to a directory, whose `count` is 0 or past [`gg_api::MOST_FRAMES`], or whose viewpoints, or
/// `rect` of no area, can't be; `api.bad_request` for a `hello` after the first, and `ecs.bad_id`
/// for an ID that isn't one.
fn parse(request: &Request) -> Result<Call, Failure> {
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
            // Before the frames are captured, rather than as their PNGs fail to be written.
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

/// The event `given` is, as the layers get it, a cursor's position over the view at `origin` in the
/// main window taken to the window's; `None` for a key named as no key is.
fn input_event(given: &InputEvent, origin: [u32; 2]) -> Option<Event<'_>> {
    let event = match *given {
        InputEvent::Key {
            ref key,
            pressed,
            repeat,
        } => Event::Key {
            key: Key::from_name(key)?,
            pressed,
            repeat,
        },
        InputEvent::Text(ref text) => Event::Text(text),
        InputEvent::Cursor([x, y]) => Event::CursorMoved {
            x: x + f64::from(origin[0]),
            y: y + f64::from(origin[1]),
        },
        InputEvent::CursorEntered => Event::CursorEntered,
        InputEvent::CursorLeft => Event::CursorLeft,
        InputEvent::Motion([x, y]) => Event::MouseMotion { x, y },
        InputEvent::Button { button, pressed } => Event::MouseButton {
            button: match button {
                Button::Left => MouseButton::Left,
                Button::Right => MouseButton::Right,
                Button::Middle => MouseButton::Middle,
                Button::Back => MouseButton::Back,
                Button::Forward => MouseButton::Forward,
                Button::Other(number) => MouseButton::Other(number),
            },
            pressed,
        },
        InputEvent::Scroll(gg_api::Scroll::Lines([x, y])) => Event::Scroll(Scroll::Lines { x, y }),
        InputEvent::Scroll(gg_api::Scroll::Pixels([x, y])) => {
            Event::Scroll(Scroll::Pixels { x, y })
        }
        InputEvent::Focused(focused) => Event::Focused(focused),
    };
    Some(event)
}

/// Method `method`'s parameters, `params`, as a `T`, whose defaults stand for what isn't given.
fn take<T: DeserializeOwned>(method: &str, params: &Value) -> Result<T, Failure> {
    let params = match params {
        Value::Null => Value::Object(serde_json::Map::new()),
        given => given.clone(),
    };
    serde_json::from_value(params).map_err(|error| bad_params(method, format!("takes {error}")))
}

/// Checks that method `method`, which takes nothing, is given nothing: no parameters, `null` or an
/// empty object.
fn none(method: &str, params: &Value) -> Result<(), Failure> {
    match params {
        Value::Null => Ok(()),
        Value::Object(map) if map.is_empty() => Ok(()),
        _ => Err(bad_params(method, "takes nothing")),
    }
}

fn bad_params(method: &str, what: impl Display) -> Failure {
    Failure::new("api.bad_params", format!("`{method}` {what}"))
}

/// The entity ID `text` gives.
///
/// Fails with `ecs.bad_id` if it gives none.
fn entity_id(text: &str) -> Result<EntityId, Failure> {
    text.parse().map_err(|error| failure(&error))
}

#[cfg(test)]
mod tests;
