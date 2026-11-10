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
    Applied, Apply, AssetList, Authored, Button, By, Capture, Captured, EntityList, Failure, Field,
    Hash, Hello, History, Input, InputEvent, Listed, ListedAsset, ListedOrphan, Looked, Patched,
    Picked, Pulse, Request, SavePrefab, SavedPrefab, Scene, SceneAction, Session, Snapshotted,
    Status, Step, View, Welcome,
};
use gg_core::log::Events;
use gg_core::math::{Quat, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{MAX_PICKS, Outline, Projection, Viewpoint};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use subsecond::JumpTable;
use tracing::{Level, debug, info, warn};

use super::{Waiting, json};
use crate::assets::Asset;
use crate::edits::{Done, Edit};
use crate::grant::{Capability, Grant};
use crate::grid::Plane;
use crate::layer::{Offered, Stack};
use crate::luau::Debugger;
use crate::main_loop::Stepper;
use crate::net::Ticket;
use crate::reflect::{self, Ident, Type};
use crate::render_thread::{Extracted, Publish, Shot};
use crate::scene_files::{self, Open, SceneFile};
use crate::snapshot::Run;
use crate::{
    Context, Entity, EntityId, Event, Font, Key, Model, MouseButton, Name, Operation, Pose, Prefab,
    PrefabRoot, Scroll, Texture, World, patch, snapshot,
};

/// The methods, as `api.unknown_method` lists them.
const METHODS: [&str; 32] = [
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
    "view",
    "input",
    "look",
    "pick",
    "entities",
    "entity",
    "get",
    "types",
    "hash",
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
    /// Whether the frame being made is bare, as the oldest capture waiting decided as the frame
    /// started.
    bare: bool,
    /// The look as the clients set it, each part `None` where it's the game's own, and the run's
    /// number; its `seen` is filled in as it's answered.
    look: Looked,
    /// `pick`s waiting for their answers.
    picks: Vec<Picking>,
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
    pngs: Sender<Png>,
    /// Where snapshots without a path go, and how many have, which number their files.
    snapshots: PathBuf,
    snapshotted: u64,
    /// To the thread that writes snapshots.
    taken: Sender<Taken>,
    /// Edits and patches offered to a lockstep session, waiting for the tick they apply before.
    offered: Vec<(Reply, Ticket, Offer)>,
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

/// A `pick` waiting for its answer: at a pixel of the view, with the ticket the frame that asked it
/// gave, once one has; and whether it answers its entity's name, as it does to a connection granted
/// `read` of `Name`.
struct Picking {
    at: [f64; 2],
    ticket: Option<crate::Pick>,
    named: bool,
    reply: Reply,
}

/// A `capture` waiting for its frame: where its answer goes, where its PNG goes, whether it's of
/// the game's own view, which a bare frame shows, and when it came, which it waits from, as
/// [`CAPTURE_WAIT`] says.
struct Capturing {
    reply: Reply,
    path: Option<PathBuf>,
    own: bool,
    since: Instant,
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
        own: bool,
    },
    /// The name of the memory to publish the view to, or `None` to stop.
    View(Option<String>),
    /// Events to hand the layers, each of which names a key that's a key's.
    Input(Vec<InputEvent>),
    /// The parts of the look to set, each checked, its viewpoint's rotation normalized.
    Look(gg_api::Look),
    /// The pixel of the view to pick at.
    Pick([f64; 2]),
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
    Assets {
        kind: Option<AssetKind>,
        skip: usize,
        take: usize,
    },
    Ui,
    Snapshot(Option<PathBuf>),
    Patch(JumpTable),
    /// An edit's operations, and the gesture it's part of, if it's a set of one.
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
}

/// A snapshot for the thread that writes them: the copy of the world, the ticks the simulation had
/// run as it was taken, where it goes, and where the answer goes.
struct Taken {
    world: World,
    /// The scene files the layers loaded, which the restored run watches, and its open scene.
    scenes: Vec<SceneFile>,
    open: Open,
    /// The scripts the layers loaded, each a path and its text as last applied, and the mods.
    scripts: Vec<(String, String)>,
    mods: Vec<crate::luau::ModText>,
    /// What the run was doing, with a copy of the world as edited it held, if any, which the
    /// restored run goes on doing.
    run: Run,
    ticks: u64,
    path: PathBuf,
    reply: Reply,
}

/// A capture's PNG for the thread that writes them: the view, where it goes, the ticks the frame
/// that shows it had run, whether textures or meshes were still decoding or streaming in it, and
/// where the answer goes.
struct Png {
    shot: Arc<Shot>,
    path: PathBuf,
    ticks: u64,
    streaming: bool,
    reply: Reply,
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
            bare: false,
            look: Looked {
                viewpoint: None,
                outline: None,
                grid: None,
                colliders: None,
                by: By::default(),
                revision: 0,
                run,
                seen: None,
            },
            picks: Vec::new(),
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
        })
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
    /// refused before the first tick of a run playing from its start. Then makes the frame bare if
    /// the oldest capture waiting, of those riding, whose frame may not render, then of those
    /// asked, is of the game's own view; and unless it's bare, which reads no picks, has the frame
    /// ask the picks waiting, as many as [`MOST_PICKS`], the rest waiting for the frames after.
    /// Returns whether a request asked the run to end.
    pub(crate) fn serve(
        &mut self,
        cx: &mut Context,
        stack: &mut Stack,
        stepper: &mut Stepper,
    ) -> bool {
        self.shared.frames.fetch_add(1, Ordering::Relaxed);
        let quit = self.answer(cx, stack, stepper, false);
        let oldest = self.riding.first().or(self.asked.first());
        self.bare = oldest.is_some_and(|capturing| capturing.own);
        cx.look.bare = self.bare;
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
    /// counting a frame: a `capture`, a `pick` or a `step`, which need frames, fail with
    /// `api.paused`.
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
                Call::Step(_) | Call::Capture { .. } | Call::Pick(_) if paused => {
                    let message = "frames have paused while the window is minimized, and steps, \
                                   captures and picks run in frames; restore the window first";
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
                Call::Capture { path, own } => self.asked.push(Capturing {
                    reply,
                    path,
                    own,
                    since: Instant::now(),
                }),
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
                Call::Pick(at) => self.picks.push(Picking {
                    at,
                    ticket: None,
                    named: names,
                    reply,
                }),
                Call::Entities { with, skip, take } => {
                    reply.send(list(world, with.as_deref(), skip, take));
                }
                Call::Entity(id) => reply.send(entity(world, id)),
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
                // A run stopped, or playing a copy, is snapshotted before its first tick too, so
                // that a restart keeps the world as edited; what the layers queued as they attached
                // goes, as it does at a `stop` that brings that world back.
                Call::Snapshot(_)
                    if stepper.ran() == 0 && !stepper.stopped() && cx.edited.is_none() =>
                {
                    let message = "the simulation hasn't ticked since the run started, and a \
                                   snapshot before then would miss what the layers queued for its \
                                   first tick; ask once it has";
                    reply.send(Err(Failure::new("api.not_ticked", message)));
                }
                Call::Snapshot(path) => {
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
                    let taken = Taken {
                        world: world.clone(),
                        scenes: cx.scenes.files.clone(),
                        open: cx.scenes.open.clone(),
                        scripts: cx.hosts.applied(),
                        mods: cx.mods.applied(),
                        run,
                        ticks: stepper.ticks,
                        path,
                        reply,
                    };
                    // The thread runs until the API drops.
                    let _ = self.taken.send(taken);
                }
                Call::Patch(table) => match stack.offer_patch(table, stepper.ticks) {
                    Ok(ticket) => self.offered.push((reply, ticket, Offer::Patch)),
                    Err(table) => {
                        let patched = patch::apply(table, stepper.ticks);
                        let done = patched.map(|()| Done::Applied(Vec::new()));
                        answer_done(reply, &Offer::Patch, stepper.ticks, done);
                    }
                },
                Call::Apply(given, gesture) => {
                    // What its values name that isn't loaded is loaded first, as a scene's is.
                    let named = assets_named(world, &given);
                    cx.load_named(named, false);
                    match operations(cx.world(), cx.world(), given) {
                        Ok(mut operations) => {
                            let author = client.to_string();
                            let edit = match gesture {
                                Some(gesture) => Edit::Amend {
                                    author,
                                    gesture,
                                    operation: operations.pop().expect("checked as it was read"),
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
            by,
            revision,
            ..
        } = &mut self.look;
        let changed = [
            set_part(viewpoint, &mut by.viewpoint, given.viewpoint, client),
            set_part(outline, &mut by.outline, given.outline, client),
            set_part(grid, &mut by.grid, given.grid, client),
            set_part(colliders, &mut by.colliders, given.colliders, client),
        ];
        if changed.contains(&true) {
            *revision += 1;
        }
        let look = &mut cx.look;
        look.viewpoint = viewpoint.as_ref().map(engine_viewpoint);
        look.outline = outline.as_ref().map(|outline| Outline {
            id: (outline.id.parse::<EntityId>())
                .expect("checked as it was read")
                .get(),
            color: Vec4::from_array(outline.color),
            width: outline.width,
        });
        look.grid = grid.map(|plane| match plane {
            gg_api::Plane::Xy => Plane::XY,
            gg_api::Plane::Xz => Plane::XZ,
            gg_api::Plane::Yz => Plane::YZ,
        });
        look.colliders = *colliders;
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
            publish.skip = self.bare;
            publish.ticks = ticks;
            publish.started = cx.started;
        }
        match frame.captured.take() {
            Some(Ok(shot)) => {
                let shot = Arc::new(shot);
                let streaming = frame.loading > 0 || !frame.streaming.is_empty();
                let now = Instant::now();
                let taken = (self.riding).extract_if(.., |capturing| {
                    !streaming || now.duration_since(capturing.since) >= CAPTURE_WAIT
                });
                for Capturing { reply, path, .. } in taken {
                    let path = path.unwrap_or_else(|| {
                        self.numbered += 1;
                        let name = format!("{}-{}.png", self.stem, self.numbered);
                        self.captures.join(name)
                    });
                    let png = Png {
                        shot: shot.clone(),
                        path,
                        ticks: self.riding_ticks,
                        streaming,
                        reply,
                    };
                    // The thread runs until the API drops.
                    let _ = self.pngs.send(png);
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
            ticket,
            named,
            reply,
            ..
        } in back
        {
            let shown = answered(ticket).flatten();
            // A model's node is its root's, which tools pick and change in its place.
            let entity = shown.and_then(|id| world.entity(id));
            let entity = entity.map(|entity| world.node_root(entity));
            let id = entity.map_or(shown, |entity| world.id(entity));
            let name = entity.filter(|_| named);
            let name = name.and_then(|entity| world.get::<Name>(entity));
            reply.result(&Picked {
                id: id.map(|id| id.to_string()),
                name: name.map(|name| name.0.clone()),
            });
        }
        let bare = self.bare;
        let kind = self.asked.extract_if(.., |capturing| capturing.own == bare);
        self.riding.extend(kind);
        frame.capture = !self.riding.is_empty();
        if frame.capture {
            self.riding_ticks = ticks;
        }
    }

    /// Whether the simulation is paused or stopped with nothing asked of the frames: no ticks to
    /// step, no capture or pick waiting and no request held.
    pub(crate) fn is_idle(&self, stepper: &Stepper) -> bool {
        (stepper.paused() || stepper.stopped())
            && stepper.steps() == 0
            && self.asked.is_empty()
            && self.riding.is_empty()
            && self.picks.is_empty()
            && self.held.is_none()
    }

    /// While the view is published, the number of the frame to be handed off next, as the client
    /// will take it; `None` while the frame is bare, which isn't published.
    pub(crate) fn handing(&self) -> Option<u64> {
        let pacer = self.pacer.as_ref().filter(|_| !self.bare)?;
        Some(pacer.published().saturating_add(1))
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
        name: world.get::<Name>(entity).map(|name| name.0.clone()),
        parent: parent.and_then(id),
        children: children
            .map(|child| id(child).expect("a child is alive"))
            .collect(),
        components: components.map(str::to_owned).collect(),
    }
}

/// The entity of ID `id` in `world`, as `entity` answers it.
fn entity(world: &World, id: EntityId) -> Result<String, Failure> {
    let entity = found(world, id)?;
    let about = |error: Error| failure(&error);
    let mut out = String::from("{\"id\":");
    json::string(&mut out, &id.to_string());
    out.push_str(",\"name\":");
    match world.get::<Name>(entity) {
        Some(name) => json::string(&mut out, &name.0),
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
    json::floats(&mut out, &space.matrix.to_cols_array());
    out.push_str(",\"rotation\":");
    json::floats(&mut out, &space.rotation.to_array());
    out.push('}');
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
        position: Vec3::from_array(given.position),
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
        Flight(self.clone())
    }
}

/// A request's place among those its connection has in flight, given up as it drops: once its
/// answer is written, or as it goes unanswered, as when the connection has gone.
struct Flight(Arc<InFlight>);

impl Drop for Flight {
    fn drop(&mut self) {
        *self.0.count.lock().expect("no thread panics holding it") -= 1;
        self.0.landed.notify_one();
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
            let waiting = Waiting::now().map(Waiting::name);
            let stopped = || shared.debugger.stopped().then_some("breakpoint");
            let pulse = Pulse {
                frames: shared.frames.load(Ordering::Relaxed),
                waiting: waiting.or_else(stopped).map(str::to_owned),
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
                    let message = "the game is ending";
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
        Call::View(_) | Call::Look(_) | Call::Pick(_) => vec![View],
        Call::Input(_) => vec![Input],
        Call::Get { component, .. } => vec![Read(Some(component.clone()))],
        Call::Entities { .. }
        | Call::Entity(_)
        | Call::Types
        | Call::Hash
        | Call::Assets { .. }
        | Call::Ui
        | Call::Edits(_) => {
            vec![Read(None)]
        }
        Call::Snapshot(path) => [Some(Snapshot), path.as_ref().map(|_| Files)]
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

/// Writes the captures `pngs` brings, each as a PNG, answering each, until the API drops.
fn write_pngs(pngs: &Receiver<Png>) {
    for png in pngs {
        let [width, height] = png.shot.size;
        let written = write_png(&png.path, &png.shot).map(|()| Captured {
            path: png.path,
            width,
            height,
            ticks: png.ticks,
            streaming: png.streaming,
        });
        match written {
            Ok(captured) => png.reply.result(&captured),
            Err(error) => png.reply.send(Err(failure(&error))),
        }
    }
}

/// Writes the snapshots `taken` brings, each a file of app `app`'s, whose layers are `layers`, as
/// [`snapshot::write`] writes one, whole or not at all, making the `snapshots` directory beside the
/// executable if it goes there; and answers each, until the API drops.
fn write_snapshots(taken: &Receiver<Taken>, app: &str, layers: &[String]) {
    let layers: Vec<&str> = layers.iter().map(String::as_str).collect();
    for Taken {
        world,
        scenes,
        open,
        scripts,
        mods,
        run,
        ticks,
        path,
        reply,
    } in taken
    {
        let of = snapshot::Of {
            world: &world,
            scenes: &scenes,
            open: &open,
            scripts: &scripts,
            mods: &mods,
        };
        let written = snapshot::write(&of, &run, app, &layers, ticks);
        let written = written.and_then(|text| {
            if path.parent() == Some(crate::beside_exe("snapshots").as_path()) {
                fs::create_dir_all(crate::beside_exe("snapshots")).map_err(|error| {
                    let message = format!("can't make the directory of {}", path.display());
                    Error::new("snapshot.file", message).with_source(error)
                })?;
            }
            snapshot::save(&path, &text)?;
            Ok(text.len() as u64)
        });
        match written {
            Ok(bytes) => reply.result(&Snapshotted {
                path,
                ticks,
                entities: world.entities(None).map_or(0, |entities| entities.len()),
                bytes,
            }),
            Err(error) => reply.send(Err(failure(&error))),
        }
    }
}

/// Writes `shot` as an 8-bit RGB PNG at `path`, whole or not at all, making the `captures`
/// directory beside the executable if it's there: the view's pixels are sRGB-encoded, as a PNG's
/// are taken to be without a chunk that says otherwise, and its alpha goes, since the view shows
/// opaque.
///
/// Fails with `api.capture_file` if it can't.
fn write_png(path: &Path, shot: &Shot) -> Result<()> {
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
    if path.parent() == Some(crate::beside_exe("captures").as_path()) {
        fs::create_dir_all(crate::beside_exe("captures")).map_err(|error| failed(error.into()))?;
    }
    crate::dirs::write_whole(path, &data).map_err(|error| failed(error.into()))
}

/// The look `given`, method `method`'s, checked, its viewpoint's rotation normalized.
///
/// Fails with `api.bad_params`, saying why, for a viewpoint that isn't finite, whose rotation has
/// no length, or whose projection can't project, as [`Projection`] says; and for an outline whose
/// ID isn't an entity's, or whose color isn't finite, or whose width isn't finite and positive.
fn look(method: &str, mut given: gg_api::Look) -> Result<gg_api::Look, Failure> {
    if let Some(Some(viewpoint)) = &mut given.viewpoint {
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
                "takes a projection that projects: orthographic, of a positive height, between a \
                 near and a far that differ; or perspective, of a field of view more than 0 and \
                 less than π, from a positive near; not {projection:?}"
            );
            return Err(bad_params(method, what));
        }
    }
    if let Some(Some(outline)) = &given.outline {
        if let Err(error) = outline.id.parse::<EntityId>() {
            let what = format!("takes an outline of an entity's ID: {error}");
            return Err(bad_params(method, what));
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
    Ok(given)
}

/// The call `request` makes of the main loop.
///
/// Fails with `api.unknown_method` for a method there's none of, `api.bad_params` for parameters
/// it doesn't take, a `look` that can't be, as [`look`] says, a `scene` whose path is missing,
/// given where it's none, or not a `.ron` file's, or whose `discard` is given to `save`, or a
/// `save_prefab` whose path isn't a `.prefab` file's,
/// `api.bad_request` for a `hello` after the first, and `ecs.bad_id` for an ID that isn't one.
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
        "ui" => none(method, params).map(|()| Call::Ui)?,
        "snapshot" => {
            let gg_api::Snapshot { path } = take(method, params)?;
            if path.as_deref().is_some_and(|path| !path.is_absolute()) {
                let what = "takes an absolute path, since the game's working directory may not be \
                            the tool's";
                return Err(bad_params(method, what));
            }
            Call::Snapshot(path)
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
            if gesture.is_some() && !matches!(operations[..], [gg_api::Operation::Set { .. }]) {
                return Err(bad_params(method, "takes one set as a gesture's"));
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
            let Capture { path, own } = take(method, params)?;
            if path.as_deref().is_some_and(|path| !path.is_absolute()) {
                let what = "takes an absolute path, since the game's working directory may not be \
                            the tool's";
                return Err(bad_params(method, what));
            }
            Call::Capture { path, own }
        }
        "look" => Call::Look(look(method, take(method, params)?)?),
        "pick" => {
            let gg_api::Pick { at } = take(method, params)?;
            Call::Pick(at)
        }
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
mod tests {
    use std::cell::Cell;
    use std::io::Read as _;
    use std::rc::Rc;
    use std::time::Instant;

    use gg_api::{Client, LogEvents, Session};
    use gg_core::math::Mat4;
    use gg_renderer::Streaming;
    use serde_json::json;

    use super::*;
    use crate::args::Net;
    use crate::dirs::tests::Scratch;
    use crate::net::Lockstep;
    use crate::recording::Recorder;
    use crate::scene_files::tests as scenes;
    use crate::{BoxCollider2D, Sprite, Tick, Transform, Viewport};

    /// How long a test waits for what should come at once, past which it fails rather than hangs.
    const DEADLINE: Duration = Duration::from_secs(20);

    /// The manifest of the project whose game [`started`] starts the API for.
    const PROJECT: &str = "games/tested/project.ron";

    /// Starts the API for app `app`, a name of the test's own, so that tests running at once write
    /// session files of their own, a game of the project [`PROJECT`] names, which keeps no events;
    /// and returns it, with the session its file gives.
    fn started(app: &str) -> (Api, Session) {
        started_keeping(app, Box::leak(Box::default()))
    }

    /// As [`started`], the run's events kept in `events`.
    fn started_keeping(app: &str, events: &'static Events) -> (Api, Session) {
        let project = Some(Path::new(PROJECT));
        let api = Api::start(app, &[], None, events, None, project).expect("the API starts");
        let pid = std::process::id();
        let file = crate::beside_exe("sessions").join(Session::file_name(app, pid));
        let text = fs::read(file).expect("the session file");
        (api, serde_json::from_slice(&text).expect("a session"))
    }

    /// Where the answer to request `id` goes: `to`, whose other end the test reads, with a place in
    /// flight of a connection of its own.
    fn reply(id: u64, to: &Sender<Answer>) -> Reply {
        Reply {
            id,
            to: to.clone(),
            flight: Arc::new(InFlight::default()).take(),
        }
    }

    /// A connection to `session`'s API, written and read as raw lines, once it has said hello.
    fn said_hello(session: &Session) -> (TcpStream, BufReader<TcpStream>) {
        let stream = TcpStream::connect(session.address).expect("it connects");
        stream.set_read_timeout(Some(DEADLINE)).expect("a timeout");
        let hello = json!({"id": 0, "method": "hello", "params": {"token": session.token}});
        (&stream)
            .write_all(format!("{hello}\n").as_bytes())
            .expect("written");
        let mut reader = BufReader::new(stream.try_clone().expect("a clone"));
        let welcome = gg_api::read_line(&mut reader).expect("an answer");
        assert!(welcome.is_some_and(|line| line.contains("\"result\"")));
        (stream, reader)
    }

    /// Runs `client` on a thread of its own, connected to `session`'s API, while this thread serves
    /// `api` from `cx`, whose layers are `stack`'s, as frames would, each a tick's time, looking at
    /// the scene files first, as a frame does, then stepping `stepper` and doing `frame`; returns
    /// what `client` returns.
    fn serving<T: Send + 'static>(
        api: &mut Api,
        cx: &mut Context,
        stack: &mut Stack,
        stepper: &mut Stepper,
        session: Session,
        client: impl FnOnce(Client) -> T + Send + 'static,
        mut frame: impl FnMut(&mut Api, &mut Context, &Stepper),
    ) -> T {
        let client = thread::spawn(move || {
            client(Client::connect(&session, "test", DEADLINE).expect("it connects"))
        });
        let start = Instant::now();
        while !client.is_finished() {
            assert!(start.elapsed() < DEADLINE, "the client never finished");
            crate::scene_files::look(cx, stack);
            api.serve(cx, stack, stepper);
            stepper.step(Tick::DT, |_| true, || Duration::ZERO);
            frame(api, cx, stepper);
            thread::sleep(Duration::from_millis(1));
        }
        client.join().expect("the client didn't panic")
    }

    /// What [`a_client_pauses_and_steps_the_simulation_and_reads_the_world`]'s client gets.
    struct Read {
        paused: Status,
        stepped: Status,
        status: Status,
        page: EntityList,
        rest: EntityList,
        all: EntityList,
        entity: Value,
        kid: Value,
        field: f32,
        types: gg_api::Types,
        hash: Hash,
        refused: Vec<String>,
    }

    /// A client pauses the simulation, steps it, and reads the world: its status, pages of its
    /// entities, an entity whole, the space a root's and a child's transforms are in among it, a
    /// field, its types, whose description describes them again as they are, and the hash; and is
    /// refused, with a code, what's wrong.
    #[test]
    fn a_client_pauses_and_steps_the_simulation_and_reads_the_world() {
        let (mut api, session) = started("Api Reads");
        let mut cx = Context::seeded(1);
        let world = cx.world_mut();
        let at = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            ..Transform::IDENTITY
        };
        let hero = world.spawn((at, Name("Hero".to_owned())));
        let kid = world.spawn(Transform::IDENTITY);
        world.set_parent(kid, Some(hero)).expect("alive");
        for _ in 0..3 {
            world.spawn((Transform::IDENTITY, Sprite::default()));
        }
        let [hero, kid] = [hero, kid].map(|entity| world.id(entity).expect("alive").to_string());
        let mut stepper = Stepper::default();
        let asked = [hero.clone(), kid.clone()];
        let client = move |mut client: Client| {
            let [hero, kid] = asked;
            let field = json!({"id": hero, "component": "Transform", "path": "translation.y"});
            let refused = [
                ("entity", json!({"id": "0000000000000001"})),
                ("get", json!({"id": hero, "component": "Nope"})),
                ("step", json!({"ticks": 0})),
                ("bogus", Value::Null),
            ];
            Read {
                paused: client.call_as("pause", ()).expect("pauses"),
                stepped: client.call_as("step", json!({"ticks": 3})).expect("steps"),
                status: client.call_as("status", ()).expect("a status"),
                page: (client.call_as("entities", json!({"with": "Sprite", "take": 2})))
                    .expect("a page"),
                rest: client
                    .call_as("entities", json!({"skip": 4}))
                    .expect("the rest"),
                all: client.call_as("entities", ()).expect("them all"),
                entity: client
                    .call_as("entity", json!({"id": hero}))
                    .expect("the hero"),
                kid: client
                    .call_as("entity", json!({"id": kid}))
                    .expect("the kid"),
                field: client.call_as("get", field).expect("its y"),
                types: client.call_as("types", ()).expect("the types"),
                hash: client.call_as("hash", ()).expect("a hash"),
                refused: (refused.into_iter())
                    .map(|(method, params)| client.call(method, params).expect_err(method).code)
                    .collect(),
            }
        };
        let read = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        );
        assert!(read.paused.paused);
        assert_eq!(
            read.stepped.ticks,
            read.paused.ticks + 3,
            "answered once its ticks ran"
        );
        assert_eq!(
            (read.status.ticks, read.status.paused),
            (read.stepped.ticks, true)
        );
        assert_eq!(read.status.entities, 5);
        assert_eq!((read.page.total, read.page.entities.len()), (3, 2));
        assert!(
            read.page.entities[0]
                .components
                .contains(&"Sprite".to_owned())
        );
        assert_eq!((read.rest.total, read.rest.entities.len()), (5, 1));
        let listed = |id: &str| read.all.entities.iter().find(|listed| listed.id == id);
        let listed = |id: &str| listed(id).expect("listed");
        assert_eq!(listed(&hero).children, std::slice::from_ref(&kid));
        assert_eq!(listed(&kid).parent.as_ref(), Some(&hero));
        assert_eq!(read.entity["name"], "Hero");
        assert_eq!(read.entity["parent"], Value::Null);
        assert_eq!(read.entity["children"], json!([kid]));
        let translation = &read.entity["components"]["Transform"]["translation"];
        assert_eq!(*translation, json!([1.0, 2.0, 3.0]));
        // A root's transform is in the world's space; the kid's in the hero's, as it's placed.
        let space = |translation: [f32; 3]| {
            let matrix = Mat4::from_translation(Vec3::from_array(translation));
            json!({"matrix": matrix.to_cols_array(), "rotation": [0.0, 0.0, 0.0, 1.0]})
        };
        assert_eq!(read.entity["space"], space([0.0; 3]));
        assert_eq!(read.kid["space"], space([1.0, 2.0, 3.0]));
        assert_eq!(read.field, 2.0);
        let types = cx.world().types();
        let names: Vec<&str> = read
            .types
            .components
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, cx.world().component_types().collect::<Vec<_>>());
        let parent = read.types.components.iter().find(|c| c.name == "Parent");
        let refused = parent.expect("a parent's type").refused.as_deref();
        assert!(
            refused.is_some_and(|why| why.contains("entity")),
            "{refused:?}"
        );
        let described = json::described(&read.types).expect("described");
        assert_eq!(described.types.iter().len(), types.iter().len());
        for ((_, ours), (_, theirs)) in types.iter().zip(described.types.iter()) {
            assert_eq!(ours, theirs, "described as it is");
        }
        let transform = described
            .components
            .iter()
            .find(|(name, _)| name == "Transform");
        let transform = transform.map(|(_, default)| default.as_ref().ok());
        let identity = crate::Reflect::to_value(&Transform::IDENTITY);
        assert_eq!(transform, Some(Some(&identity)));
        assert_eq!(
            read.hash.content,
            format!("{:016x}", cx.world().hash().content)
        );
        let codes = [
            "ecs.no_entity",
            "reflect.unknown_type",
            "api.bad_params",
            "api.unknown_method",
        ];
        assert_eq!(read.refused, codes);
    }

    /// A connection's first request is `hello` with the token, and `ping` is answered by the
    /// connection's own thread while no frame serves, with what frames wait for.
    #[test]
    fn a_connection_says_hello_with_the_token_and_pings_without_frames() {
        let (_api, session) = started("Api Hello");
        assert_eq!(session.project.as_deref(), Some(Path::new(PROJECT)));
        let wrong = Session {
            token: "0".repeat(32),
            ..session.clone()
        };
        let refused = Client::connect(&wrong, "test", DEADLINE).err();
        let refused = refused.map(|failure| failure.code);
        assert_eq!(refused.as_deref(), Some("api.unauthorized"));
        let mut client = Client::connect(&session, "test", DEADLINE).expect("it connects");
        assert_eq!(client.welcome().app, "Api Hello");
        let pulse: Pulse = client.call_as("ping", ()).expect("a pulse");
        let still = Pulse {
            frames: 0,
            waiting: None,
        };
        assert_eq!(pulse, still);
        let waiting = super::super::waiting(Waiting::Menu);
        let pulse: Pulse = client.call_as("ping", ()).expect("a pulse");
        assert_eq!(pulse.waiting.as_deref(), Some("menu"));
        drop(waiting);
    }

    /// A connection is kept to what its `hello` grants: a request beyond it is refused, with
    /// `api.ungranted`, by the connection's own thread, and a capability there's none of refuses
    /// the `hello`, with `grant.unknown`.
    #[test]
    fn a_connection_is_kept_to_what_its_hello_grants() {
        let (_api, session) = started("Api Grants");
        let grant = |names: &[&str]| Some(names.iter().map(|&name| name.to_owned()).collect());
        let unknown = Client::connect_granted(&session, "test", grant(&["reed"]), DEADLINE);
        assert_eq!(
            unknown.err().map(|failure| failure.code).as_deref(),
            Some("grant.unknown")
        );
        let granted = Client::connect_granted(&session, "test", grant(&["read", "view"]), DEADLINE);
        let mut client = granted.expect("it connects");
        let set = json!({"operations": [{"set": {
            "id": "00000000000000ff", "component": "Transform", "path": "", "value": {},
        }}]});
        let note = json!({"code": "test.noted", "message": "noted"});
        for (method, params) in [("apply", set), ("note", note), ("quit", Value::Null)] {
            let refused = client.call(method, params).expect_err(method);
            assert_eq!(
                refused.code, "api.ungranted",
                "{method}: {}",
                refused.message
            );
        }
        let pulse: Pulse = client.call_as("ping", ()).expect("a pulse");
        assert_eq!(pulse.frames, 0);
    }

    /// Whether `stream`'s connection has been closed by the game, rather than waiting for more:
    /// its read ends, or is reset, well before the game would drop a connection that never says
    /// hello.
    fn closed(stream: &mut TcpStream) -> bool {
        stream
            .set_read_timeout(Some(HELLO_TIMEOUT / 2))
            .expect("a timeout");
        match stream.read(&mut [0]) {
            Ok(read) => read == 0,
            Err(error) => matches!(
                error.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
            ),
        }
    }

    /// A `hello` as long as [`gg_api::MAX_HELLO`] is welcomed, and one longer is refused once it's
    /// past it, without the game waiting for the rest.
    #[test]
    fn a_hello_longer_than_its_bound_is_refused_unread() {
        let (_api, session) = started("Api Long Hello");
        let hello = |client: &str| {
            let params = json!({"token": session.token, "client": client});
            json!({"id": 7, "method": "hello", "params": params}).to_string()
        };
        let longest = hello(&"a".repeat(gg_api::MAX_HELLO - hello("").len()));
        assert_eq!(longest.len(), gg_api::MAX_HELLO);
        let mut stream = TcpStream::connect(session.address).expect("it connects");
        stream.set_read_timeout(Some(DEADLINE)).expect("a timeout");
        stream
            .write_all(format!("{longest}\n").as_bytes())
            .expect("written");
        let answer = gg_api::read_line(&mut BufReader::new(&stream)).expect("an answer");
        let answer: gg_api::Answer = serde_json::from_str(&answer.expect("a line")).expect("JSON");
        assert_eq!((answer.id, answer.error), (7, None));
        let mut stream = TcpStream::connect(session.address).expect("it connects");
        stream
            .write_all(&[b' '; gg_api::MAX_HELLO + 1])
            .expect("written");
        assert!(closed(&mut stream), "the game waited for the rest");
    }

    /// A `hello` that comes a byte at a time is cut off at its deadline, though each byte comes well
    /// within the time left, rather than read for as long as bytes keep coming.
    #[test]
    fn a_hello_dripped_a_byte_at_a_time_is_cut_off_at_its_deadline() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("it listens");
        let address = listener.local_addr().expect("its address");
        let mut client = TcpStream::connect(address).expect("it connects");
        client.set_nodelay(true).expect("no delay");
        let (served, _) = listener.accept().expect("accepted");
        let token = "0".repeat(32);
        let hello =
            format!("{{\"id\":0,\"method\":\"hello\",\"params\":{{\"token\":\"{token}\"}}}}\n");
        // 80 bytes, which take 1.6 s to come whole.
        let dripping = thread::spawn(move || {
            for byte in hello.bytes() {
                if client.write_all(&[byte]).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let mut reader = BufReader::new(served);
        let deadline = Duration::from_millis(250);
        let start = Instant::now();
        let read = read_hello(&mut reader, start + deadline);
        let took = start.elapsed();
        // Which ends the client's writes.
        drop(reader);
        dripping.join().expect("the client didn't panic");
        assert_eq!(read, None);
        assert!(
            took >= deadline && took < Duration::from_secs(1),
            "{took:?}"
        );
    }

    /// While [`MOST_SAYING_HELLO`] connections are saying hello, the game closes the next unread, a
    /// client's too, until one of them has gone.
    #[test]
    fn connections_past_the_most_saying_hello_are_refused() {
        let (_api, session) = started("Api Hellos");
        let connect = || TcpStream::connect(session.address).expect("it connects");
        let mut saying: Vec<TcpStream> = (0..MOST_SAYING_HELLO).map(|_| connect()).collect();
        assert!(
            closed(&mut connect()),
            "a connection past the most was served"
        );
        let refused = Client::connect(&session, "test", DEADLINE).err();
        assert!(refused.is_some(), "a client past the most was welcomed");
        drop(saying.pop());
        // Once the game has seen that one go.
        let start = Instant::now();
        let client = loop {
            if let Ok(client) = Client::connect(&session, "test", DEADLINE) {
                break client;
            }
            assert!(start.elapsed() < DEADLINE, "no client was welcomed");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(client.welcome().app, "Api Hellos");
    }

    /// A client that sends requests faster than they're answered, here with no frame to answer
    /// them, is held back by its connection once it has [`gg_api::MOST_IN_FLIGHT`] in flight: the
    /// game reads no more, and the client's writes stop once the loopback's buffers are full,
    /// rather than the game reading on, holding what it reads.
    #[test]
    fn a_client_that_sends_faster_than_its_answers_go_is_held_back() {
        let (_api, session) = started("Api In Flight");
        let (mut stream, _) = said_hello(&session);
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("a timeout");
        // 64 MiB of them, far more than the loopback's buffers hold, a chunk at a time, so that each
        // write waits only for the game to read on.
        let status = "{\"id\":1,\"method\":\"status\"}\n";
        let chunk = status.repeat((64 << 10) / status.len());
        let mut sent = 0;
        let held = loop {
            if sent >= 64 << 20 {
                break None;
            }
            if let Err(error) = stream.write_all(chunk.as_bytes()) {
                break Some(error.kind());
            }
            sent += chunk.len();
        };
        let held = held.is_some_and(|kind| {
            matches!(kind, io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
        });
        assert!(
            held,
            "the game read {sent} bytes of requests it didn't answer"
        );
    }

    /// Hands each event it gets to `events`, and until it has had `most`, has its client send
    /// another `input`, waiting for the game to pass it on to the main loop, as `wakes` says.
    struct Sending {
        stream: TcpStream,
        wakes: Receiver<()>,
        events: Rc<Cell<usize>>,
        most: usize,
    }

    impl crate::Layer for Sending {
        fn event(&mut self, _: &mut Context, _: &Event<'_>) -> std::ops::ControlFlow<()> {
            self.events.set(self.events.get() + 1);
            if self.events.get() < self.most {
                let input =
                    json!({"id": 1, "method": "input", "params": {"events": ["cursor_left"]}});
                (self.stream)
                    .write_all(format!("{input}\n").as_bytes())
                    .expect("written");
                self.wakes.recv_timeout(DEADLINE).expect("passed on");
            }
            std::ops::ControlFlow::Continue(())
        }
    }

    /// A frame answers the requests that came before it started, and those that come as it does
    /// wait for the next, so that a client that keeps sending can't keep the frame from going on.
    #[test]
    fn a_frame_answers_the_requests_that_came_before_it_started() {
        let (mut api, session) = started("Api Frames Go On");
        let (passed, wakes) = mpsc::channel();
        api.set_waker(Box::new(move || {
            let _ = passed.send(());
        }));
        let (mut stream, mut answers) = said_hello(&session);
        let input = json!({"id": 1, "method": "input", "params": {"events": ["cursor_left"]}});
        stream
            .write_all(format!("{input}\n").as_bytes())
            .expect("written");
        wakes.recv_timeout(DEADLINE).expect("passed on");
        let events = Rc::new(Cell::new(0));
        let mut stack = Stack::default();
        stack.push_layer(Sending {
            stream: stream.try_clone().expect("a clone"),
            wakes,
            events: events.clone(),
            most: 3,
        });
        let mut cx = Context::seeded(13);
        let mut stepper = Stepper::default();
        for frame in 1..=3 {
            api.serve(&mut cx, &mut stack, &mut stepper);
            assert_eq!(
                events.get(),
                frame,
                "the frame answered those that came after it started"
            );
            let answer = gg_api::read_line(&mut answers).expect("an answer");
            assert_eq!(answer.as_deref(), Some("{\"id\":1,\"result\":{}}"));
        }
    }

    /// Each request needs its own capability, or `read` or `write` of the world, of the one type it
    /// names alone, as gg-api's docs say.
    #[test]
    fn a_request_needs_what_its_method_and_the_types_it_names_need() {
        let grant = Grant::parse(["read:Transform", "view", "write:Sprite"]).expect("a grant");
        let path = std::env::temp_dir().join("granted.png");
        let set = |component: &str| {
            json!({"operations": [{"set": {
                "id": "00000000000000ff", "component": component, "path": "", "value": {},
            }}]})
        };
        let get = |component: &str| json!({"id": "00000000000000ff", "component": component});
        let delete = json!({"operations": [{"delete": {"id": "00000000000000ff"}}]});
        let asked = [
            ("status", Value::Null, None),
            ("get", get("Transform"), None),
            ("get", get("Sprite"), Some("read:Sprite")),
            ("entities", Value::Null, Some("read")),
            ("capture", Value::Null, None),
            ("capture", json!({ "path": path }), Some("files")),
            ("look", Value::Null, None),
            ("apply", set("Sprite"), None),
            ("apply", set("Transform"), Some("write:Transform")),
            ("apply", delete, Some("write")),
            (
                "save_prefab",
                json!({"id": "00000000000000ff"}),
                Some("write"),
            ),
            ("undo", Value::Null, Some("write")),
            ("step", json!({"ticks": 1}), Some("play")),
            ("snapshot", Value::Null, Some("snapshot")),
            ("quit", Value::Null, Some("quit")),
        ];
        for (method, params, needed) in asked {
            let request = Request {
                id: 1,
                method: method.to_owned(),
                params,
            };
            let call = parse(&request).expect("a call");
            let refused = granted(&grant, method, &needs(&call)).err();
            let named = needed.map(|needed| format!("`{method}` needs `{needed}`,"));
            let message = refused.map(|failure| (failure.code, failure.message));
            match (named, message) {
                (None, None) => {}
                (Some(named), Some((code, message))) => {
                    assert_eq!(code, "api.ungranted");
                    assert!(message.starts_with(&named), "{message}");
                }
                (named, message) => panic!("{method}: {named:?}, {message:?}"),
            }
        }
    }

    /// What [`a_client_debugs_a_script_while_the_game_waits_at_its_stop`]'s client gets.
    struct Debugged {
        before: gg_api::Debugging,
        stopped: gg_api::Debugging,
        pulse: Pulse,
        stepped: gg_api::Debugging,
        gone_on: gg_api::Debugging,
        refused: Vec<String>,
    }

    /// A client sets a breakpoint in a script and waits for the game to stop there, which `ping`
    /// says it waits at, answered on the connection's own thread while the main thread waits in the
    /// tick; steps over a line; clears it and has the game go on; and is refused, with a code,
    /// what's wrong.
    #[test]
    fn a_client_debugs_a_script_while_the_game_waits_at_its_stop() {
        let (mut api, session) = started("Api Debug");
        let dir = Scratch::new("api-debug");
        fs::create_dir_all(dir.0.join("scripts")).expect("a directory");
        let path = dir.0.join("scripts/count.luau");
        let script = "local Count = gg.declare(\"Count\", { n = 0 })\n\
            function Count.tick(entity, dt)\n\
                local n = entity[Count.n]\n\
                n = n + 1\n\
                entity[Count.n] = n\n\
            end\n\
            return Count\n";
        fs::write(&path, script).expect("written");
        let mut cx = Context::seeded(1).root(&dir.0);
        cx.hosts.debug(api.debugger());
        let mut host = crate::Scripts::new().expect("Luau starts");
        host.load(&mut cx, &path).expect("the script loads");
        let world = cx.world_mut();
        let counted = world.spawn(());
        world.add_default(counted, "Count").expect("added");
        let pinging = session.clone();
        let client = move |mut client: Client| {
            let mut debug = |params: Value| -> gg_api::Debugging {
                client.call_as("debug", params).expect("an answer")
            };
            let before = debug(json!({}));
            let lines = json!({"script": "scripts/count.luau", "lines": [4]});
            let stopped = debug(json!({"breakpoints": lines, "wait_ms": 10_000}));
            let mut pinged = Client::connect(&pinging, "pinger", DEADLINE).expect("it connects");
            let pulse = pinged.call_as("ping", ()).expect("a pulse");
            let stepped = debug(json!({"action": "step_over", "wait_ms": 10_000}));
            let none = json!({"script": "scripts/count.luau", "lines": []});
            let gone_on = debug(json!({"breakpoints": none, "action": "continue"}));
            let refused = [
                json!({"action": "continue"}),
                json!({"breakpoints": {"script": "", "lines": [1]}}),
                json!({"breakpoints": {"script": "scripts/count.luau", "lines": [0]}}),
                json!({"wait_ms": 10_001}),
                json!({"action": "nope"}),
            ];
            let refused = (refused.into_iter())
                .map(|params| client.call("debug", params).expect_err("refused").code)
                .collect();
            Debugged {
                before,
                stopped,
                pulse,
                stepped,
                gone_on,
                refused,
            }
        };
        let got = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut Stepper::default(),
            session,
            client,
            |_, cx, _| {
                cx.hosts.place();
                host.run(cx.world_mut());
            },
        );
        assert_eq!(got.before.scripts, ["scripts/count.luau"]);
        assert_eq!((got.before.stopped, got.before.stops), (None, 0));
        let stop = got.stopped.stopped.expect("stopped");
        let at = |stop: &gg_api::Stop| (stop.why, stop.frames[0].line);
        assert_eq!(at(&stop), (gg_api::StopReason::Breakpoint, 4));
        let placed = &got.stopped.breakpoints;
        assert_eq!((placed[0].line, placed[0].placed), (4, Some(4)));
        assert_eq!(got.pulse.waiting.as_deref(), Some("breakpoint"));
        let stepped = got.stepped.stopped.expect("stopped");
        assert_eq!(at(&stepped), (gg_api::StopReason::Step, 5));
        let n = |stop: &gg_api::Stop| {
            let locals = &stop.frames[0].locals;
            let n = locals
                .iter()
                .find(|local| local.name == "n")
                .expect("a local");
            n.value.as_f64().expect("a number")
        };
        assert_eq!(n(&stepped), n(&stop) + 1.0, "its line ran");
        assert_eq!((got.gone_on.stopped, got.gone_on.stops), (None, 2));
        assert!(got.gone_on.breakpoints.is_empty());
        assert_eq!(
            got.refused,
            [
                "script.not_stopped",
                "api.bad_params",
                "api.bad_params",
                "api.bad_params",
                "api.bad_params",
            ]
        );
    }

    /// A token given is the token only if every byte is: not if its first or last differs, nor if
    /// it's the token cut short, the token with more after, or nothing.
    #[test]
    fn a_token_given_is_the_token_only_if_every_byte_is() {
        let token = "0123456789abcdef0123456789abcdef";
        assert!(same_token(token, token));
        let longer = format!("{token}0");
        for given in [
            "f123456789abcdef0123456789abcdef",
            "0123456789abcdef0123456789abcdee",
            "0123456789abcdef",
            &longer,
            "",
        ] {
            assert!(!same_token(given, token), "{given:?}");
        }
    }

    /// The numbers of the events `read` read, each event's `n`.
    fn numbers(read: &LogEvents) -> Vec<u64> {
        let events = read.events.iter().map(|event| {
            let event: Value = serde_json::from_str(event.get()).expect("an object");
            event["n"].as_u64().expect("its number")
        });
        events.collect()
    }

    /// A client follows the run's logs while no frame serves: the events kept, at a level and more
    /// severe, from the oldest or the last of them, and those that come as it waits; a cursor of
    /// another run's reads from the oldest kept; and what's wrong is refused, with a code.
    #[test]
    fn a_client_follows_the_logs_without_frames() {
        let events: &'static Events = Box::leak(Box::default());
        let (_api, session) = started_keeping("Api Logs", events);
        let line = |n: u64, level: Level| format!(r#"{{"level":"{level}","n":{n}}}"#);
        for (n, level) in [Level::INFO, Level::WARN, Level::DEBUG]
            .into_iter()
            .enumerate()
        {
            events.push(level, &line(n as u64, level));
        }
        let mut client = Client::connect(&session, "test", DEADLINE).expect("it connects");
        let all: LogEvents = client.call_as("logs", ()).expect("the events");
        assert_eq!(numbers(&all), [0, 1, 2]);
        assert_eq!((all.next, all.end, all.missed), (3, 3, 0));
        let warned: LogEvents =
            (client.call_as("logs", json!({"level": "warn"}))).expect("the warnings");
        assert_eq!((numbers(&warned), warned.next), (vec![1], 3));
        let last: LogEvents = client
            .call_as("logs", json!({"last": 1}))
            .expect("the last");
        assert_eq!(numbers(&last), [2]);
        assert_eq!(last.run, all.run, "one run's");

        let pusher = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            events.push(Level::ERROR, &line(3, Level::ERROR));
        });
        let next = json!({"from": all.next, "run": all.run, "level": "error", "wait_ms": 10_000});
        let start = Instant::now();
        let waited: LogEvents = client.call_as("logs", next).expect("the next");
        pusher.join().expect("the pusher didn't panic");
        assert!(start.elapsed() < DEADLINE, "answered as it came");
        assert_eq!((numbers(&waited), waited.next), (vec![3], 4));

        let other = json!({"from": waited.next, "run": all.run ^ 1});
        let other: LogEvents = client.call_as("logs", other).expect("the events");
        assert_eq!(numbers(&other), [0, 1, 2, 3]);
        for refused in [json!({"level": "loud"}), json!({"wait_ms": 10_001})] {
            let code = client
                .call("logs", refused.clone())
                .err()
                .map(|failure| failure.code);
            assert_eq!(code.as_deref(), Some("api.bad_params"), "{refused}");
        }
    }

    /// A note is answered by the connection's own thread while no frame serves; and one that isn't
    /// a note, as one at an error's level, of a code of another shape, without a message, or too
    /// long, is refused, with a code.
    #[test]
    fn a_client_notes_an_event_without_frames_or_is_refused() {
        let (_api, session) = started("Api Notes");
        let mut client = Client::connect(&session, "test", DEADLINE).expect("it connects");
        let note = |code: &str, level: &str, message: &str| {
            json!({
                "code": code,
                "level": level,
                "message": message,
            })
        };
        let mut noted = note("gg.patch_failed", "warn", "A patch didn't build");
        noted["fields"] = json!({"files": ["src/main.rs"]});
        let answer = client.call("note", noted).expect("noted");
        assert_eq!(answer.get(), "{}");
        let long = "x".repeat(gg_api::MOST_NOTE + 1);
        let with = |key: &str, value: Value| {
            let mut note = note("gg.patch_failed", "info", "m");
            note[key] = value;
            note
        };
        for refused in [
            note("gg.patch_failed", "error", "m"),
            note("gg", "info", "m"),
            note("Gg.patch_failed", "info", "m"),
            note("gg..patch_failed", "info", "m"),
            note("gg.patch-failed", "info", "m"),
            note("gg.1patch", "info", "m"),
            note(
                &format!("gg.{}", "x".repeat(gg_api::MOST_NOTE_CODE)),
                "info",
                "m",
            ),
            note("gg.patch_failed", "info", ""),
            note("gg.patch_failed", "info", &long),
            with("fields", json!(["src/main.rs"])),
            with("fields", json!({"files": long})),
            with("by", json!("me")),
            json!({"code": "gg.patch_failed", "level": "info"}),
            Value::Null,
        ] {
            let code = client.call("note", refused.clone()).err();
            let code = code.map(|failure| failure.code);
            assert_eq!(code.as_deref(), Some("api.bad_params"), "{refused}");
        }
    }

    /// What a JSON layer writes, its events' lines, as the run's log has them.
    #[derive(Clone, Default)]
    struct Written(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Written {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut written = self.0.lock().expect("no thread panics holding it");
            written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A note is logged as the run's JSON log writes events: at its level, with its code and its
    /// message, the client that gave it, and its fields, if any, as one field of JSON text.
    #[test]
    fn a_note_is_logged_at_its_level_as_its_clients() {
        use tracing_subscriber::layer::SubscriberExt;

        let written = Written::default();
        let writer = written.clone();
        // Two dispatchers, as `testing::logged` keeps for the reason it gives.
        let _other = tracing::subscriber::set_default(tracing_subscriber::registry());
        let json = tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_writer(move || writer.clone());
        let _logging = tracing::subscriber::set_default(tracing_subscriber::registry().with(json));
        let warned = json!({"code": "gg.attach_changed", "level": "warn",
            "message": "A layer's attach changed", "fields": {"files": ["src/main.rs"]}});
        assert_eq!(note(&warned, "gg").as_deref(), Ok("{}"));
        let told = json!({"code": "agent.checked", "level": "info", "message": "Checked"});
        assert_eq!(note(&told, "claude-code").as_deref(), Ok("{}"));
        let text = written
            .0
            .lock()
            .expect("no thread panics holding it")
            .clone();
        let text = String::from_utf8(text).expect("UTF-8");
        let lines: Vec<Value> = (text.lines())
            .map(|line| serde_json::from_str(line).expect("an event"))
            .collect();
        assert_eq!(lines.len(), 2, "{text}");
        let field = |line: &Value, name: &str| line.get(name).cloned().unwrap_or(Value::Null);
        let logged =
            |line: &Value| ["level", "code", "message", "client"].map(|name| field(line, name));
        let expected = [
            "WARN",
            "gg.attach_changed",
            "A layer's attach changed",
            "gg",
        ];
        assert_eq!(logged(&lines[0]), expected.map(Value::from));
        let fields = field(&lines[0], "fields");
        let fields = fields.as_str().expect("JSON text");
        let fields: Value = serde_json::from_str(fields).expect("JSON");
        assert_eq!(fields, json!({"files": ["src/main.rs"]}));
        let expected = ["INFO", "agent.checked", "Checked", "claude-code"];
        assert_eq!(logged(&lines[1]), expected.map(Value::from));
        assert_eq!(field(&lines[1], "fields"), Value::Null, "none given");
    }

    /// A client's sets of one field in one gesture undo as one edit, and a gesture's apply is of
    /// one set.
    #[test]
    fn a_gestures_sets_undo_as_one_edit() {
        let (mut api, session) = started("Api Gestures");
        let mut cx = Context::seeded(3);
        let hero = cx.world_mut().spawn(Transform::IDENTITY);
        let hero_id = cx.world().id(hero).expect("alive").to_string();
        let mut stepper = Stepper::default();
        stepper.pause();
        let client = move |mut client: Client| {
            let set = |x: i32| {
                let path = "translation.x";
                json!({"set": {"id": hero_id, "component": "Transform", "path": path, "value": x}})
            };
            for x in [1, 2, 3] {
                let apply = json!({"operations": [set(x)], "gesture": 9});
                client.call_as::<Applied>("apply", apply).expect("set");
            }
            let two = json!({"operations": [set(4), set(5)], "gesture": 9});
            let refused = client.call("apply", two).expect_err("two sets").code;
            let history: History = client.call_as("edits", ()).expect("the edits");
            let undone: Option<gg_api::Edit> = client.call_as("undo", ()).expect("undone");
            (refused, history, undone)
        };
        let (refused, history, undone) = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        );
        assert_eq!(refused, "api.bad_params");
        assert_eq!(history.authors[0].done.len(), 1, "one edit: {history:?}");
        assert!(undone.is_some());
        let x = cx
            .world()
            .get::<Transform>(hero)
            .expect("a transform")
            .translation
            .x;
        assert_eq!(x, 0.0, "the gesture undone whole");
    }

    /// A client's edit that names a texture none loaded, by its ID, has it loaded from its file
    /// under the root before the edit's read; one naming a texture that can't be loaded is
    /// refused, saying why.
    #[test]
    fn an_edit_loads_the_textures_it_names() {
        let (mut api, session) = started("Api Textures");
        let dir = Scratch::new("api-textures");
        fs::create_dir_all(dir.0.join("assets")).expect("a directory");
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        fs::write(dir.0.join("assets/stone.png"), png).expect("a PNG");
        let id = "5f3c9a1e07b2d846";
        let meta = gg_api::asset::Meta {
            id: id.parse().expect("an ID"),
        };
        fs::write(dir.0.join("assets/stone.png.meta"), meta.text()).expect("its ID file");
        let mut cx = Context::seeded(3).root(dir.0.clone());
        let hero = cx
            .world_mut()
            .spawn((Transform::IDENTITY, Sprite::default()));
        let hero_id = cx.world().id(hero).expect("alive").to_string();
        let mut stepper = Stepper::default();
        stepper.pause();
        let client = move |mut client: Client| {
            let set = |texture: &str| {
                let set = json!({"id": hero_id, "component": "Sprite", "path": "texture", "value": texture});
                json!({"operations": [{"set": set}]})
            };
            let applied = client.call_as::<Applied>("apply", set(id));
            let refused = client.call("apply", set("assets/none.png"));
            (applied, refused.expect_err("no file"))
        };
        let (applied, refused) = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        );
        applied.expect("set");
        assert_eq!(refused.code, "api.no_texture", "{}", refused.message);
        assert!(
            refused.message.contains("engine.bad_texture"),
            "{}",
            refused.message
        );
        let sprite = cx.world().get::<Sprite>(hero).expect("a sprite");
        let texture = sprite.texture.expect("a texture");
        assert_eq!(cx.world().assets().path(texture), Some("assets/stone.png"));
    }

    /// What [`clients_apply_edits_and_each_undoes_its_own`]'s clients get.
    struct Edited {
        set: Applied,
        created: Applied,
        refused: [Failure; 3],
        others: Option<gg_api::Edit>,
        history: History,
        undone: Option<gg_api::Edit>,
        redone: Option<gg_api::Edit>,
        undone_for_them: Option<gg_api::Edit>,
        nothing: Option<gg_api::Edit>,
    }

    /// A client applies edits, while the simulation's paused, read by the types of the fields they
    /// set, all of each or none; another undoes only its own, but for an author it names; and undo
    /// and redo step through an edit's operations together.
    #[test]
    fn clients_apply_edits_and_each_undoes_its_own() {
        let (mut api, session) = started("Api Edits");
        let mut cx = Context::seeded(2);
        let hero = cx
            .world_mut()
            .spawn((Transform::IDENTITY, Name("Hero".to_owned())));
        let hero_id = cx.world().id(hero).expect("alive").to_string();
        let mut stepper = Stepper::default();
        stepper.pause();
        let other = session.clone();
        let asked = hero_id.clone();
        let client = move |mut client: Client| {
            let hero = asked;
            let apply = |client: &mut Client, operations: Value| {
                client.call_as::<Applied>("apply", json!({ "operations": operations }))
            };
            let set = json!([
                {"set": {"id": hero, "component": "Transform", "path": "translation.x", "value": 2}},
                {"set": {"id": hero, "component": "Name", "path": "0", "value": "Ace"}},
            ]);
            let set = apply(&mut client, set).expect("set");
            // Missing fields take their defaults; the parent is the hero.
            let create = json!([{"create": {"parent": hero, "components": {
                "Transform": {"scale": [2, 2, 2]},
                "Name": {"0": "Kid"},
            }}}]);
            let created = apply(&mut client, create).expect("created");
            let refused = [
                // The first is fine, so the second's refusal undoes it.
                json!([
                    {"set": {"id": hero, "component": "Transform", "path": "translation.y", "value": 5}},
                    {"delete": {"id": "0000000000000001"}},
                ]),
                json!([{"set": {"id": hero, "component": "Transform", "path": "translation", "value": 1}}]),
                json!([{"add": {"id": hero, "component": "Nope"}}]),
            ]
            .map(|operations| apply(&mut client, operations).expect_err("refused"));
            let mut them = Client::connect(&other, "other", DEADLINE).expect("it connects");
            let others = them.call_as("undo", ()).expect("nothing of theirs");
            let history = client.call_as("edits", ()).expect("the edits");
            let undone = client.call_as("undo", ()).expect("the create undone");
            let redone = client.call_as("redo", ()).expect("redone");
            let undone_for_them = them
                .call_as("undo", json!({"author": "test"}))
                .expect("the create undone for test");
            client
                .call_as::<Option<gg_api::Edit>>("undo", ())
                .expect("the set undone");
            let nothing = client.call_as("undo", ()).expect("nothing left");
            Edited {
                set,
                created,
                refused,
                others,
                history,
                undone,
                redone,
                undone_for_them,
                nothing,
            }
        };
        let edited = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        );
        assert_eq!(edited.set.entities, Vec::<String>::new());
        assert_eq!(edited.created.entities.len(), 1);
        let kid = &edited.created.entities[0];
        let codes = edited.refused.map(|failure| failure.code);
        assert_eq!(
            codes,
            [
                "ecs.no_entity",
                "reflect.wrong_kind",
                "reflect.unknown_type"
            ]
        );
        assert_eq!(edited.others, None);
        let words = |words: &[&str]| {
            words
                .iter()
                .map(|&word| word.to_owned())
                .collect::<Vec<_>>()
        };
        let expected = Authored {
            author: "test".to_owned(),
            done: vec![
                words(&[
                    &format!("set `Transform.translation.x` of {hero_id}"),
                    &format!("set `Name.0` of {hero_id}"),
                ]),
                words(&[&format!("create {kid}")]),
            ],
            undone: Vec::new(),
        };
        assert_eq!(edited.history.authors, [expected]);
        let create = gg_api::Edit {
            author: "test".to_owned(),
            operations: words(&[&format!("create {kid}")]),
        };
        assert_eq!(edited.undone.as_ref(), Some(&create));
        assert_eq!(edited.redone.as_ref(), Some(&create));
        assert_eq!(edited.undone_for_them.as_ref(), Some(&create));
        assert_eq!(edited.nothing, None);
        // Every edit undone, and the refused ones never applied.
        let world = cx.world();
        assert_eq!(world.entities(None).expect("all").len(), 1);
        assert_eq!(world.get::<Transform>(hero), Some(&Transform::IDENTITY));
        assert_eq!(
            world.get::<Name>(hero).map(|name| name.0.as_str()),
            Some("Hero")
        );
        assert_eq!(stepper.ticks, 0, "paused, edits apply all the same");
    }

    /// `quit` is answered, and ends the run once the frame's served.
    #[test]
    fn quit_is_answered_and_ends_the_run() {
        let (mut api, session) = started("Api Quit");
        let mut cx = Context::seeded(3);
        let mut stepper = Stepper::default();
        let client = thread::spawn(move || {
            let mut client = Client::connect(&session, "test", DEADLINE).expect("it connects");
            client
                .call("quit", Value::Null)
                .map(|result| result.get().to_owned())
        });
        let start = Instant::now();
        while !api.serve(&mut cx, &mut Stack::default(), &mut stepper) {
            assert!(start.elapsed() < DEADLINE, "no quit came");
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(client.join().expect("no panic").as_deref(), Ok("{}"));
    }

    /// `snapshot` writes the world whole where it's asked, once a thread of its own has, and answers
    /// where, at how many ticks, of how many entities and bytes: a file a run restores the world
    /// from, at those ticks. A path that isn't absolute is refused, and so is a snapshot before the
    /// run's first tick.
    #[test]
    fn a_snapshot_is_written_where_its_asked() {
        let (mut api, session) = started("Api Snapshots");
        let mut cx = Context::seeded(5);
        for _ in 0..3 {
            cx.world_mut()
                .spawn((Transform::IDENTITY, Sprite::default()));
        }
        // Paused, so that no tick runs before the client asks.
        let mut stepper = Stepper::default();
        stepper.pause();
        let dir = Scratch::new("api-snapshot");
        fs::create_dir_all(&dir.0).expect("a directory");
        let path = dir.0.join("world.snapshot");
        let asked = path.clone();
        let client = move |mut client: Client| {
            let mut refused = Vec::new();
            for path in [json!("world.snapshot"), json!(asked)] {
                let call = client.call("snapshot", json!({ "path": path }));
                refused.push(call.expect_err("refused").code);
            }
            client.call("step", json!({"ticks": 1})).expect("stepped");
            let taken: Snapshotted =
                (client.call_as("snapshot", json!({"path": asked}))).expect("a snapshot");
            (refused, taken)
        };
        let (refused, taken) = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        );
        assert_eq!(refused, ["api.bad_params", "api.not_ticked"]);
        assert_eq!((&taken.path, taken.ticks, taken.entities), (&path, 1, 3));
        assert_eq!(taken.bytes, fs::metadata(&path).expect("written").len());
        let snapshot = crate::snapshot::Snapshot::read(&path, "Api Snapshots").expect("read");
        let mut restored = Context::seeded(6);
        let ticks = snapshot
            .restore(restored.world_mut(), &[])
            .expect("restored");
        assert_eq!(ticks, taken.ticks);
        assert_eq!(restored.world().hash(), cx.world().hash());
    }

    /// What [`stop_brings_back_the_world_as_edited_that_play_copied`]'s client gets.
    struct Stopped {
        stopped: Status,
        still: Status,
        snapshot: Snapshotted,
        refused: String,
        edited: Hash,
        played: Status,
        ticking: Status,
        failed: String,
        back: Status,
        hash: Hash,
        history: History,
        undone: Option<gg_api::Edit>,
    }

    /// `stop` stops the game, whose frames then run no ticks, paused or not, which a snapshot may
    /// be taken of before any, and which refuses `step`; `play` has a copy of the world as edited
    /// play in its place, whose edits go at the next `stop`, which fails the `step`s waiting,
    /// leaving the game paused as they did, and brings back the world as edited at its ticks, as it
    /// hashed, with its edits, which `undo` takes back; and every answer says whether the game is
    /// stopped.
    #[test]
    fn stop_brings_back_the_world_as_edited_that_play_copied() {
        let (mut api, session) = started("Api Play And Stop");
        let mut cx = Context::seeded(8);
        let hero = cx.world_mut().spawn(Transform::IDENTITY);
        let hero_id = cx.world().id(hero).expect("alive").to_string();
        // Paused, so that no tick runs before the client asks.
        let mut stepper = Stepper::default();
        stepper.pause();
        let dir = Scratch::new("api-stop");
        fs::create_dir_all(&dir.0).expect("a directory");
        let path = dir.0.join("stopped.snapshot");
        let (asked, other) = (path.clone(), session.clone());
        let client = move |mut client: Client| {
            let set = |x: i32| {
                let path = "translation.x";
                let set =
                    json!({"id": hero_id, "component": "Transform", "path": path, "value": x});
                json!({"operations": [{ "set": set }]})
            };
            let status = |client: &mut Client| client.call_as::<Status>("status", ());
            let stopped: Status = client.call_as("stop", ()).expect("stopped");
            // Stopped, it runs no ticks, paused or not.
            client.call_as::<Status>("resume", ()).expect("resumed");
            let still = loop {
                let status = status(&mut client).expect("a status");
                if status.frames > stopped.frames + 2 {
                    break status;
                }
            };
            let snapshot = (client.call_as("snapshot", json!({ "path": asked })))
                .expect("a snapshot before any tick");
            let step = client.call("step", json!({"ticks": 1}));
            let refused = step.expect_err("refused").code;
            client.call_as::<Applied>("apply", set(1)).expect("set");
            let edited = client.call_as("hash", ()).expect("a hash");
            let played: Status = client.call_as("play", ()).expect("played");
            let ticking = loop {
                let status = status(&mut client).expect("a status");
                if status.ticks > played.ticks {
                    break status;
                }
            };
            client
                .call_as::<Applied>("apply", set(2))
                .expect("set in the copy");
            // Another client's step, waiting for its ticks, which pauses the game.
            let stepping = thread::spawn(move || {
                let mut them = Client::connect(&other, "other", DEADLINE).expect("it connects");
                let step = them.call("step", json!({"ticks": MOST_STEPS}));
                step.expect_err("failed").code
            });
            while !status(&mut client).expect("a status").paused {}
            let back: Status = client.call_as("stop", ()).expect("stopped");
            Stopped {
                stopped,
                still,
                snapshot,
                refused,
                edited,
                played,
                ticking,
                failed: stepping.join().expect("the step didn't panic"),
                back,
                hash: client.call_as("hash", ()).expect("a hash"),
                history: client.call_as("edits", ()).expect("the edits"),
                undone: client.call_as("undo", ()).expect("undone"),
            }
        };
        let got = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        );
        assert!(got.stopped.stopped && got.still.stopped && !got.still.paused);
        assert_eq!((got.stopped.ticks, got.still.ticks), (0, 0));
        assert_eq!(
            (got.snapshot.ticks, got.refused.as_str()),
            (0, "api.stopped")
        );
        let snapshot = crate::snapshot::Snapshot::read(&path, "Api Play And Stop");
        assert!(snapshot.expect("read").stopped, "it says it's stopped");
        assert!(!got.played.stopped && !got.ticking.stopped);
        assert_eq!(got.failed, "api.stopped");
        assert!(got.back.stopped && got.back.paused);
        assert_eq!(got.back.ticks, got.played.ticks, "the world as edited's");
        assert_eq!(got.hash, got.edited);
        let [authored] = &got.history.authors[..] else {
            panic!("one author: {:?}", got.history);
        };
        assert_eq!(authored.done.len(), 1, "the edit before `play` alone");
        assert!(got.undone.is_some());
        assert_eq!(
            cx.world().get::<Transform>(hero),
            Some(&Transform::IDENTITY)
        );
        assert!(cx.edited.is_none());
    }

    /// A client saves the open scene, which the watch's next look applies nothing of, then saves it
    /// as another file, opens one in its place, unsaved edits discarded once refused, the textures
    /// it names loaded, and empties it, each answered with the status, which names the open scene
    /// and says whether it's unsaved; and is refused, with a code, what can't be.
    #[test]
    fn a_client_saves_opens_and_empties_the_open_scene() {
        let logged = crate::testing::logged();
        let (mut api, session) = started("Api Scenes");
        let dir = Scratch::new("api-scenes");
        let (_, ids) = scenes::project(&dir.0, 1);
        let scenes_dir = dir.0.join("scenes");
        fs::create_dir_all(dir.0.join("assets")).expect("a directory");
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        fs::write(dir.0.join("assets/stone.png"), png).expect("a PNG");
        // Of a sprite of a texture no layer adds, which opening it loads.
        let level2 =
            "(entities: {\"00000000000a5001\": (Sprite: (texture: Some(\"assets/stone.png\")))})\n";
        scenes::save(&scenes_dir.join("level2.ron"), level2, 0);
        fs::write(dir.0.join("outside.ron"), "(entities: {})\n").expect("written");
        let mut cx = scenes::started(&dir.0);
        let mut stack = Stack::default();
        let mut stepper = Stepper::default();
        stepper.pause();
        let scene = |client: &mut Client, params: Value| client.call_as::<Status>("scene", params);
        let create = json!({"operations": [{"create": {"components": {"Transform": {}}}}]});
        let client = move |mut client: Client| {
            let loaded = client.call_as("status", ()).expect("a status");
            client.call_as::<Applied>("apply", create).expect("created");
            let edited = client.call_as("status", ()).expect("a status");
            let saved = scene(&mut client, json!({"action": "save"})).expect("saved");
            (loaded, edited, saved)
        };
        let (loaded, edited, saved): (Status, Status, Status) = serving(
            &mut api,
            &mut cx,
            &mut stack,
            &mut stepper,
            session.clone(),
            client,
            |_, _, _| {},
        );
        scenes::look_now(&mut cx, &mut stack);
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert!(
            !codes.iter().any(|code| code == "scene.applied"),
            "{codes:?}"
        );
        let logs = cx
            .authors
            .logs(cx.world())
            .map(|(author, _)| author.to_owned());
        assert_eq!(logs.collect::<Vec<_>>(), ["test"], "no edit of the file's");
        let id = ids[0].to_string();
        let set = json!({"operations": [{"set": {"id": id, "component": "Transform",
            "path": "translation.x", "value": 2}}]});
        let client = move |mut client: Client| {
            let saved_as = (scene(
                &mut client,
                json!({"action": "save", "path": "scenes/copy.ron"}),
            ))
            .expect("saved as");
            let open = json!({"action": "open", "path": "scenes/level2.ron"});
            client.call_as::<Applied>("apply", set).expect("set");
            let unsaved = scene(&mut client, open.clone()).expect_err("unsaved").code;
            let mut discarded = open;
            discarded["discard"] = json!(true);
            let opened = scene(&mut client, discarded).expect("opened");
            let emptied = scene(&mut client, json!({"action": "new"})).expect("emptied");
            let refused = [
                json!({"action": "save"}),
                json!({"action": "open", "path": "outside.ron"}),
                json!({"action": "open", "path": "scenes/none.ron"}),
            ];
            let refused =
                refused.map(|params| scene(&mut client, params).expect_err("refused").code);
            (saved_as, unsaved, opened, emptied, refused.to_vec())
        };
        let (saved_as, unsaved, opened, emptied, refused) = serving(
            &mut api,
            &mut cx,
            &mut stack,
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        );
        let named = |status: &Status| (status.scene.clone(), status.unsaved);
        let title = |name: &str| Some(name.to_owned());
        assert_eq!(named(&loaded), (title("scenes/main.ron"), false));
        assert_eq!(named(&edited), (title("scenes/main.ron"), true));
        assert_eq!(named(&saved), (title("scenes/main.ron"), false));
        assert_eq!(named(&saved_as), (title("scenes/copy.ron"), false));
        assert_eq!(unsaved, "scene.unsaved");
        assert_eq!(named(&opened), (title("scenes/level2.ron"), false));
        assert!(cx.world().assets().texture("assets/stone.png").is_some());
        assert_eq!((named(&emptied), emptied.entities), ((None, false), 0));
        assert_eq!(refused, ["scene.untitled", "scene.bad_path", "scene.file"]);
        assert!(scenes_dir.join("copy.ron").is_file());
    }

    /// `scene` is refused with `api.no_scene` where it can't be: `open` and `new` in a recorded
    /// run, which saves, and all of it in a lockstep peer; and with `engine.no_project` in a game
    /// that isn't a project's.
    #[test]
    fn scenes_are_refused_where_the_run_cant_have_them() {
        let (mut api, session) = started("Api No Scenes");
        let dir = Scratch::new("api-no-scenes");
        scenes::project(&dir.0, 1);
        let mut stepper = Stepper::default();
        stepper.pause();
        let asked = [
            json!({"action": "new", "discard": true}),
            json!({"action": "open", "path": "scenes/main.ron", "discard": true}),
            json!({"action": "save"}),
        ];
        let ask = move |mut client: Client| {
            let asked = asked.map(|params| client.call("scene", params).err().map(|f| f.code));
            asked.to_vec()
        };
        let mut recorded = Stack::default();
        let recorder = Recorder::create(&dir.0.join("run.recording"), "Tested");
        recorded.record(recorder.expect("created"));
        let mut peer = Stack::default();
        let net = Net::Host("127.0.0.1:0".to_owned());
        peer.lockstep(Lockstep::start(&net, "Tested", &[]).expect("hosting"));
        let no_scene = Some("api.no_scene".to_owned());
        let no_project = Some("engine.no_project".to_owned());
        for (stack, project, codes) in [
            (
                &mut recorded,
                true,
                [no_scene.clone(), no_scene.clone(), None],
            ),
            (
                &mut peer,
                true,
                [no_scene.clone(), no_scene.clone(), no_scene],
            ),
            (
                &mut Stack::default(),
                false,
                [no_project.clone(), no_project.clone(), no_project],
            ),
        ] {
            let mut cx = if project {
                scenes::started(&dir.0)
            } else {
                Context::seeded(4)
            };
            let got = serving(
                &mut api,
                &mut cx,
                stack,
                &mut stepper,
                session.clone(),
                ask.clone(),
                |_, _, _| {},
            );
            assert_eq!(got, codes, "project: {project}");
        }
    }

    /// What [`a_client_saves_an_entity_as_a_prefab_linked_in_its_place`]'s client gets.
    struct Prefabbed {
        saved: SavedPrefab,
        root: Value,
        bulb: Value,
        renamed: Value,
        reverted: Value,
        refused: Vec<String>,
        again: SavedPrefab,
    }

    /// A client saves an entity and its descendants as a prefab, which the game writes with its
    /// ID file in its `prefabs` folder, named after the entity, and links the entity to, under the
    /// IDs its subtree has, as an edit `undo` unlinks; `entity` answers what of an instance's
    /// entities differs from its prefab, which `revert` sets back; and a save is refused, with a
    /// code, for an instance's entity or root, a file that's there, a path outside the root or not a
    /// prefab's, while one of the same name is numbered.
    #[test]
    fn a_client_saves_an_entity_as_a_prefab_linked_in_its_place() {
        let (mut api, session) = started("Api Prefabs");
        let dir = Scratch::new("api-prefabs");
        scenes::project(&dir.0, 0);
        let transform = "Transform: (translation: (2.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, \
                         1.0), scale: (1.0, 1.0, 1.0))";
        let main = format!(
            "(entities: {{\"0000000000000a01\": (Name: (\"Big Lamp\"), {transform}), \
             \"0000000000000a02\": (Name: (\"bulb\"), Parent: (\"0000000000000a01\"))}})\n"
        );
        scenes::save(&dir.0.join("scenes").join("main.ron"), &main, 0);
        let mut cx = scenes::started(&dir.0);
        let mut stack = Stack::default();
        let mut stepper = Stepper::default();
        stepper.pause();
        let (root, bulb) = ("0000000000000a01", "0000000000000a02");
        let client = move |mut client: Client| {
            let entity = |client: &mut Client, id: &str| -> Value {
                let answer = client.call("entity", json!({"id": id})).expect("an entity");
                serde_json::from_str(answer.get()).expect("JSON")
            };
            let save = |client: &mut Client, params: Value| {
                client.call_as::<SavedPrefab>("save_prefab", params)
            };
            let saved = save(&mut client, json!({"id": root})).expect("saved");
            let (root_read, bulb_read) = (entity(&mut client, root), entity(&mut client, bulb));
            let set = json!({"set": {"id": bulb, "component": "Name", "path": "0",
                "value": "globe"}});
            let apply = |client: &mut Client, operation: Value| {
                let operations = json!({"operations": [operation]});
                client
                    .call_as::<Applied>("apply", operations)
                    .expect("applied");
            };
            apply(&mut client, set);
            let renamed = entity(&mut client, bulb);
            apply(
                &mut client,
                json!({"revert": {"id": bulb, "component": "Name"}}),
            );
            let reverted = entity(&mut client, bulb);
            let mut refused = vec![
                save(&mut client, json!({"id": root})),
                save(&mut client, json!({"id": bulb})),
            ];
            for _ in 0..3 {
                client.call("undo", Value::Null).expect("undone");
            }
            for path in [
                "prefabs/big_lamp.prefab",
                "../outside.prefab",
                "prefabs/x.ron",
            ] {
                refused.push(save(&mut client, json!({"id": root, "path": path})));
            }
            let refused = refused
                .into_iter()
                .map(|refused| refused.expect_err("refused").code);
            let again = save(&mut client, json!({"id": root})).expect("saved again");
            Prefabbed {
                saved,
                root: root_read,
                bulb: bulb_read,
                renamed,
                reverted,
                refused: refused.collect(),
                again,
            }
        };
        let got = serving(
            &mut api,
            &mut cx,
            &mut stack,
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        );
        assert_eq!(got.saved.path, "prefabs/big_lamp.prefab");
        let file = dir.0.join("prefabs").join("big_lamp.prefab");
        let meta = gg_api::asset::Meta::read(&gg_api::asset::Meta::path(&file)).expect("its ID");
        assert_eq!(got.saved.name, meta.id.to_string(), "named by its ID");
        let instance = |read: &Value| -> gg_api::Instanced {
            serde_json::from_value(read["instance"].clone()).expect("an instance's")
        };
        let linked = instance(&got.root);
        assert_eq!(
            (&*linked.root, linked.link.as_deref()),
            (root, Some("prefabs/big_lamp.prefab"))
        );
        assert!(
            linked.overrides.is_empty() && linked.added.is_empty(),
            "{linked:?}"
        );
        let given = instance(&got.bulb);
        assert_eq!(
            (&*given.root, &*given.prefab),
            (root, "prefabs/big_lamp.prefab")
        );
        assert_eq!(given.link, None);
        let renamed = instance(&got.renamed).overrides;
        assert_eq!(
            renamed.get("Name").map(Vec::as_slice),
            Some(&[String::new()][..])
        );
        assert!(instance(&got.reverted).overrides.is_empty());
        let expected = [
            "scene.prefab_variant",
            "ecs.prefab_entity",
            "asset.exists",
            "asset.outside_root",
            "api.bad_params",
        ];
        assert_eq!(got.refused, expected);
        assert_eq!(got.again.path, "prefabs/big_lamp-2.prefab");
        // Linked again, under the IDs the subtree has, the prefab's root at its origin.
        let world = cx.world();
        let [root, bulb] = [root, bulb].map(|id| world.entity(id.parse().unwrap()).unwrap());
        assert!(world.get::<PrefabRoot>(root).is_some());
        assert_eq!(world.parent(bulb).expect("alive"), Some(root));
        let text = fs::read_to_string(&file).expect("written");
        assert!(text.contains("translation: (0.0, 0.0, 0.0)"), "{text}");
        assert!(text.contains("Name: (\"bulb\")"), "{text}");
    }

    /// A prefab isn't saved while a copy of the world as edited plays, whose entities `stop` takes
    /// away with their link: nothing is written.
    #[test]
    fn a_prefab_isnt_saved_while_a_copy_plays() {
        let dir = Scratch::new("api-prefab-playing");
        scenes::project(&dir.0, 0);
        let main = "(entities: {\"0000000000000a01\": (Name: (\"Lamp\"))})\n";
        scenes::save(&dir.0.join("scenes").join("main.ron"), main, 0);
        let mut cx = scenes::started(&dir.0);
        let stack = Stack::default();
        stack.play(&mut cx, 0);
        let id = "0000000000000a01".parse().expect("an ID");
        let refused = save_prefab(&mut cx, &stack, id, None).err();
        assert_eq!(refused.expect("refused").code(), "api.no_scene");
        assert!(!dir.0.join("prefabs").exists(), "nothing written");
    }

    /// A client lists the assets under the game's root, a page of them, of a kind or all, each with
    /// its ID, which the game writes where its ID file is missing, as it scans first, and whether a
    /// layer added it; and the ID files whose assets aren't there. It's refused, with a code, in a
    /// game without a root, or whose root isn't there.
    #[test]
    fn a_client_lists_the_assets_under_the_root_with_their_ids() {
        use gg_api::asset::{AssetId, Meta};

        let (mut api, session) = started("Api Assets");
        let dir = Scratch::new("api-assets");
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.0.join(name);
            fs::create_dir_all(path.parent().expect("a folder")).expect("a directory");
            fs::write(path, bytes).expect("written");
        };
        let meta = |id: &str| Meta {
            id: id.parse().expect("an ID"),
        };
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        write("assets/a.png", &png);
        write("assets/b.png", &png);
        write(
            "assets/b.png.meta",
            meta("5f3c9a1e07b2d846").text().as_bytes(),
        );
        write("models/m.glb", b"");
        write("scenes/x.ron", b"(entities: {})\n");
        write(
            "assets/gone.png.meta",
            meta("0a1b2c3d4e5f6071").text().as_bytes(),
        );
        let mut cx = Context::seeded(2).root(dir.0.clone());
        cx.add_texture("b", dir.0.join("assets/b.png"))
            .expect("a texture");
        let mut stepper = Stepper::default();
        let ask = |mut client: Client| {
            let mut list = |params: Value| {
                let list = client.call_as::<AssetList>("assets", params);
                list.expect("a list")
            };
            [
                list(Value::Null),
                list(json!({"kind": "model"})),
                list(json!({"skip": 1, "take": 1})),
                list(json!({})),
            ]
        };
        let [all, models, page, again] = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session.clone(),
            ask,
            |_, _, _| {},
        );
        let paths = |list: &AssetList| -> Vec<String> {
            list.assets.iter().map(|asset| asset.path.clone()).collect()
        };
        assert_eq!(all.total, 3);
        assert_eq!(
            paths(&all),
            ["assets/a.png", "assets/b.png", "models/m.glb"]
        );
        let a = all.assets[0].id.as_deref().expect("written");
        let a: AssetId = a.parse().expect("16 hex digits");
        let written = Meta::read(&dir.0.join("assets/a.png.meta")).expect("its ID file");
        assert_eq!(written.id, a);
        assert_eq!(all.assets[1].id.as_deref(), Some("5f3c9a1e07b2d846"));
        let loaded: Vec<bool> = all.assets.iter().map(|asset| asset.loaded).collect();
        assert_eq!(loaded, [false, true, false]);
        assert!(
            all.assets.iter().all(|asset| asset.problem.is_none()),
            "{all:?}"
        );
        let gone = ListedOrphan {
            path: "assets/gone.png.meta".to_owned(),
            kind: AssetKind::Texture,
            id: Some("0a1b2c3d4e5f6071".to_owned()),
        };
        assert_eq!(all.orphans, [gone]);
        assert_eq!(
            (models.total, paths(&models)),
            (1, vec!["models/m.glb".to_owned()])
        );
        assert_eq!(models.assets[0].kind, AssetKind::Model);
        assert!(models.orphans.is_empty(), "the orphan's asset is a texture");
        assert_eq!(
            (page.total, paths(&page)),
            (3, vec!["assets/b.png".to_owned()])
        );
        assert_eq!(again, all, "the same IDs");

        // With a thread watching the root, each answers once it has looked since it came: an
        // asset renamed without its ID file is listed at its new path with its ID, still loaded,
        // and one that came with the ID written it.
        cx.start_assets(true, true);
        assert!(cx.watches_assets());
        let [b, c, d] = ["b", "c", "d"].map(|name| dir.0.join(format!("assets/{name}.png")));
        let other =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[7; 4], None);
        let ask = move |mut client: Client| {
            let mut list = || client.call_as::<AssetList>("assets", json!({}));
            let first = list().expect("a list");
            fs::rename(&b, &c).expect("renamed");
            fs::write(&d, other).expect("written");
            (first, list().expect("a list"))
        };
        let (first, after) = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session.clone(),
            ask,
            |_, cx, _| cx.take_assets(),
        );
        assert_eq!(first.assets, all.assets);
        let expected = [
            "assets/a.png",
            "assets/c.png",
            "assets/d.png",
            "models/m.glb",
        ];
        assert_eq!(paths(&after), expected);
        let c = &after.assets[1];
        assert_eq!(
            (c.id.as_deref(), c.loaded),
            (Some("5f3c9a1e07b2d846"), true)
        );
        let written = Meta::read(&dir.0.join("assets/d.png.meta")).expect("its ID file");
        assert_eq!(after.assets[2].id, Some(written.id.to_string()));
        assert_eq!(after.orphans, all.orphans, "b.png's ID file moved");

        let refusals = [
            (Context::seeded(3), "engine.no_root"),
            (
                Context::seeded(3).root(dir.0.join("none")),
                "asset.unlisted",
            ),
        ];
        for (mut cx, code) in refusals {
            let refused = serving(
                &mut api,
                &mut cx,
                &mut Stack::default(),
                &mut stepper,
                session.clone(),
                |mut client: Client| client.call("assets", Value::Null).expect_err("refused"),
                |_, _, _| {},
            );
            assert_eq!(refused.code, code, "{}", refused.message);
        }
    }

    /// An `assets` waiting for a look at the asset root is answered as frames pause, from the
    /// database as it is, rather than waiting for a frame or the next request.
    #[test]
    fn an_assets_waiting_for_a_look_is_answered_as_frames_pause() {
        let (mut api, _) = started("Api Paused Assets");
        let dir = Scratch::new("api-paused-assets");
        fs::create_dir_all(&dir.0).expect("a directory");
        fs::write(dir.0.join("a.png"), "").expect("an asset");
        let cx = Context::seeded(2).root(dir.0.clone());
        let (to, answers) = mpsc::channel();
        api.listing.push(Listing {
            reply: reply(7, &to),
            look: u64::MAX,
            kind: None,
            skip: 0,
            take: 10,
        });
        api.frames_paused(&cx);
        let (answer, _) = answers.try_recv().expect("answered");
        assert!(answer.contains("\"a.png\""), "{answer}");
        assert!(api.listing.is_empty());
    }

    /// A patch that can't apply is answered with `api.patch`: here, one whose table's reference is
    /// past where any process's `main` is, which a test's build refuses whether its executable
    /// exports `main` or not, as `patch`'s own tests say.
    #[test]
    fn a_patch_that_cant_apply_is_refused() {
        let (mut api, session) = started("Api Patches");
        let mut cx = Context::seeded(7);
        let mut stepper = Stepper::default();
        let client = |mut client: Client| {
            let table = json!({
                "lib": std::env::temp_dir().join("patch.dll"),
                "map": {},
                "aslr_reference": u64::MAX,
                "new_base_address": 0,
                "ifunc_count": 0,
            });
            let call = client.call("patch", json!({ "table": table }));
            call.expect_err("refused")
        };
        let refused = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        );
        assert_eq!(refused.code, "api.patch", "{}", refused.message);
    }

    /// A capture rides with the frames handed off until one that rendered brings the view back: an
    /// error, which it answers, or the view, whose PNG it writes where it's asked and answers.
    #[test]
    fn a_capture_rides_with_frames_until_one_brings_the_view_back() {
        let (mut api, session) = started("Api Captures");
        let mut cx = Context::seeded(4);
        let mut stepper = Stepper::default();
        let dir = Scratch::new("api-capture");
        fs::create_dir_all(&dir.0).expect("a directory");
        let path = dir.0.join("view.png");
        let asked = path.clone();
        let client = move |mut client: Client| {
            let refused = client.call("capture", Value::Null).expect_err("no view");
            let captured: Captured =
                (client.call_as("capture", json!({"path": asked}))).expect("a capture");
            (refused.code, captured)
        };
        let mut frame = Extracted::default();
        let mut rides = 0;
        let (refused, captured) = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            |api, cx, stepper| {
                // The frame that rode with a capture comes back: the first two that rode with each
                // had nothing to render into.
                if frame.capture {
                    rides += 1;
                    frame.captured = match rides {
                        3 => Some(Err(Error::new("api.no_view", "the view has no area"))),
                        6 => Some(Ok(Shot {
                            size: [2, 1],
                            pixels: vec![255, 0, 0, 255, 0, 128, 255, 9],
                        })),
                        _ => None,
                    };
                }
                api.frame_back(&mut frame, cx, stepper.ticks);
            },
        );
        assert_eq!(refused, "api.no_view");
        assert_eq!(captured.path, path);
        assert_eq!((captured.width, captured.height), (2, 1));
        let file = fs::File::open(&path).expect("the PNG");
        let mut reader =
            (png::Decoder::new(std::io::BufReader::new(file)).read_info()).expect("a PNG");
        let mut pixels = vec![0; reader.output_buffer_size().expect("a size")];
        let info = reader.next_frame(&mut pixels).expect("its pixels");
        assert_eq!(info.color_type, png::ColorType::Rgb);
        let rgb = &pixels[..info.buffer_size()];
        assert_eq!(rgb, [255, 0, 0, 0, 128, 255], "alpha goes");
        assert!(!frame.capture, "nothing rides once both are answered");
    }

    /// A client's input reaches the layers as the window's would, the cursor over the view taken
    /// to the window's pixels, in order.
    #[test]
    fn input_reaches_the_layers_as_the_windows_would() {
        let (mut api, session) = started("Api Input");
        let mut cx = Context::seeded(5);
        let view = Viewport {
            origin: [10, 20],
            size: [100, 50],
        };
        cx.extract(&mut Extracted::default(), view);
        let mut stepper = Stepper::default();
        let client = |mut client: Client| {
            let events = json!({"events": [
                {"key": {"key": "KeyW", "pressed": true}},
                {"key": {"key": "KeyA", "pressed": true}},
                {"key": {"key": "KeyA", "pressed": false}},
                {"key": {"key": "MetaLeft", "pressed": true}},
                {"button": {"button": "left", "pressed": true}},
                {"cursor": [3, 4.5]},
            ]});
            client.call("input", events).expect("the input");
        };
        serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        );
        let input = cx.input();
        assert!(input.key_down(Key::KeyW));
        assert!(!input.key_down(Key::KeyA), "released after its press");
        assert!(input.key_down(Key::SuperLeft), "by its W3C name");
        assert!(input.button_down(MouseButton::Left));
        assert_eq!(input.cursor(), Some([13.0, 24.5]));
    }

    /// A client has the view published to the memory it made, which the next frame back takes to
    /// the render thread, with what paces frames by it; a name that opens no memory is refused, and
    /// leaves the view published as it was; and `null` stops publishing it.
    #[test]
    fn the_view_is_published_to_the_memory_a_client_names() {
        let (mut api, session) = started("Api View");
        let mut cx = Context::seeded(6);
        let mut stepper = Stepper::default();
        let mut frame = Extracted::default();
        let client = |mut client: Client| {
            let reader = gg_api::view::Reader::create(8, 4).expect("memory for the view");
            let memory = json!({"memory": reader.name()});
            client
                .call("view", memory.clone())
                .expect("the view published");
            // Again, which leaves it as it is, rather than opening a second writer of the memory.
            client
                .call("view", memory)
                .expect("the view published still");
            let missing = json!({"memory": "gg-view-0-4294967295"});
            let refused = client.call("view", missing).expect_err("no such memory");
            (reader, refused.code)
        };
        let back = |api: &mut Api, cx: &mut Context, stepper: &Stepper| {
            api.frame_back(&mut frame, cx, stepper.ticks);
        };
        let (reader, refused) = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session.clone(),
            client,
            back,
        );
        assert_eq!(refused, "api.view");
        let publish = frame.publish.as_ref().expect("the view published");
        assert_eq!(publish.writer.asked(), Some([8, 4]));
        assert_eq!(
            api.handing(),
            Some(1),
            "the frame handed off next publishes first"
        );
        let client = |mut client: Client| {
            client
                .call("view", Value::Null)
                .expect("publishing stopped");
        };
        let back = |api: &mut Api, cx: &mut Context, stepper: &Stepper| {
            api.frame_back(&mut frame, cx, stepper.ticks);
        };
        serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            back,
        );
        assert!(frame.publish.is_none());
        assert_eq!(api.handing(), None);
        drop(reader);
    }

    /// What [`clients_set_the_look_part_by_part_the_last_setter_winning`]'s clients get.
    struct Looks {
        read: Looked,
        set: Looked,
        again: Looked,
        theirs: Looked,
        given_back: Looked,
        run: u64,
    }

    /// Clients set the look part by part, each kept until a client sets it again, the last setter
    /// winning, and `null` gives a part back to the game; a `look` that changes no part's value or
    /// setter counts no revision; and the frames get the look as it's answered.
    #[test]
    fn clients_set_the_look_part_by_part_the_last_setter_winning() {
        let (mut api, session) = started("Api Looks");
        let mut cx = Context::seeded(8);
        let mut stepper = Stepper::default();
        let other = session.clone();
        let client = move |mut client: Client| {
            let mut them = Client::connect(&other, "other", DEADLINE).expect("it connects");
            let viewpoint = json!({"position": [0, 0, 5], "rotation": [0, 0, 0, 2],
                "projection": {"Orthographic": {"height": 4, "near": -1, "far": 1}}});
            let look = |client: &mut Client, params: Value| {
                client.call_as::<Looked>("look", params).expect("the look")
            };
            Looks {
                read: look(&mut client, Value::Null),
                set: look(&mut client, json!({"viewpoint": viewpoint, "grid": "xz"})),
                again: look(&mut client, json!({"grid": "xz"})),
                theirs: look(&mut them, json!({"grid": "xy", "colliders": true})),
                given_back: look(&mut client, json!({"viewpoint": null, "colliders": null})),
                run: (client.call_as::<LogEvents>("logs", json!({"last": 0})))
                    .expect("the logs")
                    .run,
            }
        };
        let looks = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        );
        let by = |name: &str| Some(name.to_owned());
        assert_eq!((looks.read.by, looks.read.revision), (By::default(), 0));
        assert_eq!(
            looks.read.run, looks.run,
            "the run's number, as `logs` gives it"
        );
        assert_eq!(looks.read.seen, None, "before a frame drew");
        let viewpoint = gg_api::Viewpoint {
            position: [0.0, 0.0, 5.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            projection: gg_api::Projection::Orthographic {
                height: 4.0,
                near: -1.0,
                far: 1.0,
            },
        };
        let set = &looks.set;
        assert_eq!(set.viewpoint, Some(viewpoint), "its rotation normalized");
        assert_eq!((set.grid, set.colliders), (Some(gg_api::Plane::Xz), None));
        let mine = By {
            viewpoint: by("test"),
            grid: by("test"),
            ..By::default()
        };
        assert_eq!((&set.by, set.revision), (&mine, 1));
        assert_eq!(looks.again, looks.set, "nothing changed, nor counted");
        let theirs = &looks.theirs;
        assert_eq!(
            (theirs.viewpoint, theirs.grid),
            (Some(viewpoint), Some(gg_api::Plane::Xy))
        );
        let both = By {
            viewpoint: by("test"),
            grid: by("other"),
            colliders: by("other"),
            ..By::default()
        };
        assert_eq!(
            (&theirs.by, theirs.colliders, theirs.revision),
            (&both, Some(true), 2)
        );
        let back = &looks.given_back;
        assert_eq!(
            (back.viewpoint, back.grid, back.colliders),
            (None, Some(gg_api::Plane::Xy), None)
        );
        let left = By {
            grid: by("other"),
            ..By::default()
        };
        assert_eq!((&back.by, back.revision), (&left, 3));
        let look = &cx.look;
        assert_eq!(
            (look.viewpoint, look.grid, look.colliders),
            (None, Some(Plane::XY), None)
        );
        assert_eq!(look.revision, 3);
    }

    /// The look draws as a layer that set it would: its viewpoint and outline in place of a
    /// layer's, its colliders' outlines in place of the game's, and its grid before the world,
    /// which the layers' own calls go on saying they set; and a bare frame draws as the layers set.
    #[test]
    fn the_look_draws_over_what_the_layers_set_but_in_a_bare_frame() {
        let (mut api, session) = started("Api Look Draws");
        // A sprite with a collider, to draw over the grid, and the colliders' outlines over it.
        let world = |seed| {
            let mut cx = Context::seeded(seed);
            let shown = (
                Transform::IDENTITY,
                Sprite::default(),
                BoxCollider2D::default(),
            );
            let hero = cx.world_mut().spawn(shown);
            let id = cx.world().id(hero).expect("alive");
            (cx, id)
        };
        let (mut cx, hero) = world(9);
        let view = Viewport {
            origin: [0, 0],
            size: [64, 32],
        };
        cx.extract(&mut Extracted::default(), view);
        let mut stepper = Stepper::default();
        let looked = gg_api::Viewpoint {
            position: [1.0, 2.0, 10.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            projection: gg_api::Projection::Perspective {
                fov: 1.0,
                near: 0.1,
            },
        };
        let look = json!({
            "viewpoint": looked,
            "outline": {"id": hero.to_string(), "color": [1, 0.5, 0, 1], "width": 3},
            "grid": "xy",
            "colliders": true,
        });
        let client = move |mut client: Client| {
            client.call_as::<Looked>("look", look).expect("the look");
        };
        serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        );
        // What the layers set this frame.
        let theirs = Viewpoint::default();
        let set = |cx: &mut Context| {
            cx.set_viewpoint(theirs);
            cx.outline(hero, Vec4::ONE, 1.0);
        };
        let drawn = |cx: &mut Context| {
            cx.draw_world(1.0, 0.0, 0);
            let mut frame = Extracted::default();
            cx.extract(&mut frame, view);
            frame
        };
        set(&mut cx);
        let frame = drawn(&mut cx);
        // As a layer that set the look's parts itself.
        let viewpoint = engine_viewpoint(&looked);
        let (mut twin, _) = world(9);
        twin.extract(&mut Extracted::default(), view);
        twin.set_viewpoint(viewpoint);
        twin.outline(hero, Vec4::new(1.0, 0.5, 0.0, 1.0), 3.0);
        twin.draw_grid(&viewpoint, Plane::XY);
        twin.set_colliders_shown(true);
        let expected = drawn(&mut twin);
        assert_eq!(frame.draws, expected.draws);
        assert_eq!(
            (frame.viewpoint, frame.outline),
            (viewpoint, expected.outline)
        );
        assert_eq!((frame.look, cx.look.seen), (1, Some(viewpoint)));
        assert!(!cx.colliders_shown(), "as the layers set");
        // Seen from the look's viewpoint, whose exposure adapts apart, as the layer's own isn't.
        assert!(frame.looked && !expected.looked);
        // Bare, as the layers set alone.
        cx.look.bare = true;
        set(&mut cx);
        let frame = drawn(&mut cx);
        let (mut twin, _) = world(9);
        twin.extract(&mut Extracted::default(), view);
        set(&mut twin);
        let expected = drawn(&mut twin);
        assert_eq!(frame.draws, expected.draws);
        assert_eq!((frame.viewpoint, frame.outline), (theirs, expected.outline));
        assert!(!frame.looked, "a bare frame is the game's own view");
    }

    /// A capture of the game's own view rides with a bare frame, which isn't published nor paced
    /// for, nor asks a client's pick; a capture of the view as it's shown with one that isn't; and
    /// of a pair of each asked together, the older goes first, the other with a frame of its kind
    /// after.
    #[test]
    fn captures_ride_with_frames_of_their_kind_and_bare_ones_go_unpublished() {
        let (mut api, session) = started("Api Bare Captures");
        let mut cx = Context::seeded(10);
        let mut stepper = Stepper::default();
        let mut frame = Extracted::default();
        let client = |mut client: Client| {
            let reader = gg_api::view::Reader::create(8, 4).expect("memory for the view");
            let memory = json!({"memory": reader.name()});
            client.call("view", memory).expect("the view published");
            reader
        };
        let back = |api: &mut Api, cx: &mut Context, stepper: &Stepper| {
            api.frame_back(&mut frame, cx, stepper.ticks);
        };
        let reader = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            back,
        );
        let dir = Scratch::new("api-bare-capture");
        fs::create_dir_all(&dir.0).expect("a directory");
        let (to, answers) = mpsc::channel();
        for (id, own) in [(1, true), (2, false)] {
            api.asked.push(Capturing {
                reply: reply(id, &to),
                path: Some(dir.0.join(format!("{id}.png"))),
                own,
                since: Instant::now(),
            });
        }
        api.picks.push(Picking {
            at: [0.0, 0.0],
            ticket: None,
            named: true,
            reply: reply(3, &to),
        });
        // Each frame: whether it's bare, rides with captures, is published, is paced for and has
        // asked the pick.
        let mut frames = Vec::new();
        for _ in 0..4 {
            api.serve(&mut cx, &mut Stack::default(), &mut stepper);
            let bare = cx.look.bare;
            let asked = api.picks[0].ticket.is_some();
            api.frame_back(&mut frame, &cx, stepper.ticks);
            let skip = frame.publish.as_ref().expect("published").skip;
            frames.push((bare, frame.capture, !skip, api.handing().is_some(), asked));
            // As the renderer reads back the view of a frame that rides with captures, a bare one
            // 1 pixel wide, another 2.
            frame.captured = frame.capture.then(|| {
                let width = if bare { 1 } else { 2 };
                let pixels = vec![0; width * 4];
                Ok(Shot {
                    size: [width as u32, 1],
                    pixels,
                })
            });
        }
        let expected = [
            (true, true, false, false, false),
            // Bare too, as the capture riding decides, its frame not known to have rendered.
            (true, false, false, false, false),
            (false, true, true, true, true),
            (false, false, true, true, true),
        ];
        assert_eq!(frames, expected);
        let mut widths = [0; 2];
        for _ in 0..2 {
            let (line, _) = answers.recv_timeout(DEADLINE).expect("an answer");
            let answer: Value = serde_json::from_str(&line).expect("an answer's JSON");
            let id = answer["id"].as_u64().expect("its ID") as usize;
            widths[id - 1] = answer["result"]["width"]
                .as_u64()
                .expect("a capture's width");
        }
        assert_eq!(
            widths,
            [1, 2],
            "the game's own view bare, the other as it's shown"
        );
        drop(reader);
    }

    /// A capture rides on with frame after frame that shows a texture still decoding, or meshes
    /// and textures still streaming, until one shows everything, which it takes, saying nothing
    /// was on its way; one that has waited past its bound takes the next frame that brings the view
    /// back all the same, saying something was.
    #[test]
    fn a_capture_waits_within_its_bound_for_assets_to_stream() {
        let (mut api, _) = started("Api Streaming Captures");
        let mut cx = Context::seeded(12);
        let mut stepper = Stepper::default();
        let mut frame = Extracted::default();
        let dir = Scratch::new("api-streaming-capture");
        fs::create_dir_all(&dir.0).expect("a directory");
        let (to, answers) = mpsc::channel();
        let past = CAPTURE_WAIT + Duration::from_secs(1);
        let long_ago = Instant::now()
            .checked_sub(past)
            .expect("a time that long ago");
        for (id, since) in [(1, long_ago), (2, Instant::now())] {
            api.asked.push(Capturing {
                reply: reply(id, &to),
                path: Some(dir.0.join(format!("{id}.png"))),
                own: false,
                since,
            });
        }
        // The answer that comes next: its capture's ID, and whether it says what it shows was
        // still on its way.
        let answer = |timeout| {
            let (line, _) = answers.recv_timeout(timeout).ok()?;
            let answer: Value = serde_json::from_str(&line).expect("an answer's JSON");
            let id = answer["id"].as_u64().expect("its ID");
            Some((id, answer["result"]["streaming"].as_bool()))
        };
        // As the renderer reads back the view of each frame that rides with captures: three with a
        // texture decoding or meshes streaming, then one with nothing on its way.
        for (loading, streaming) in [(1, 0), (0, 2), (1, 2), (0, 0)] {
            api.serve(&mut cx, &mut Stack::default(), &mut stepper);
            api.frame_back(&mut frame, &cx, stepper.ticks);
            assert!(frame.capture, "a capture rides with the frame");
            frame.loading = loading;
            frame.streaming = Streaming {
                assets: streaming,
                bytes: 64 * streaming as u64,
            };
            frame.captured = Some(Ok(Shot {
                size: [1, 1],
                pixels: vec![0; 4],
            }));
        }
        assert_eq!(answer(DEADLINE), Some((1, Some(true))), "past its bound");
        assert_eq!(answer(Duration::from_millis(200)), None, "waiting still");
        api.frame_back(&mut frame, &cx, stepper.ticks);
        assert_eq!(
            answer(DEADLINE),
            Some((2, Some(false))),
            "once nothing is on its way"
        );
        assert!(!frame.capture, "nothing rides once both are answered");
    }

    /// A client's pick is answered with the entity drawn at its pixel of the view, about as the
    /// frames that read it come back, named as it's answered; none where nothing is, or outside
    /// the view; and an entity gone by the answer by its ID alone, as is any to a client granted
    /// `view` without `read` of `Name`.
    #[test]
    fn a_pick_is_answered_with_the_entity_drawn_there() {
        let (mut api, session) = started("Api Picks");
        let mut cx = Context::seeded(11);
        let world = cx.world_mut();
        let hero = world.spawn((Transform::IDENTITY, Name("Hero".to_owned())));
        let ghost = world.spawn((Transform::IDENTITY, Name("Ghost".to_owned())));
        let [hero, ghost] = [hero, ghost].map(|entity| world.id(entity).expect("alive"));
        world
            .despawn(world.entity(ghost).expect("alive"))
            .expect("despawned");
        let view = Viewport {
            origin: [10, 20],
            size: [100, 50],
        };
        let mut frame = Extracted::default();
        cx.extract(&mut frame, view);
        let mut stepper = Stepper::default();
        let granted = session.clone();
        let client = move |mut client: Client| {
            let pick = |client: &mut Client, at: [f64; 2]| {
                (client.call_as::<Picked>("pick", json!({ "at": at }))).expect("an answer")
            };
            let ats = [[3.5, 4.0], [60.0, 4.0], [80.0, 4.0], [-1.0, 4.0]];
            let picked = ats.map(|at| pick(&mut client, at));
            let names = [&["view"][..], &["view", "read:Name"]].map(|grant| {
                let grant = Some(grant.iter().map(|&name| name.to_owned()).collect());
                let connected = Client::connect_granted(&granted, "test", grant, DEADLINE);
                let mut client = connected.expect("it connects");
                pick(&mut client, [3.5, 4.0]).name
            });
            (picked, names)
        };
        // As the renderer answers: the hero is drawn at the view's columns up to 50, the ghost's
        // pixels from 70, and nothing between.
        let mut answers = Vec::new();
        let back = |api: &mut Api, cx: &mut Context, stepper: &Stepper| {
            cx.take_answers(&answers);
            api.frame_back(&mut frame, cx, stepper.ticks);
            cx.extract(&mut frame, view);
            answers.clear();
            answers.extend(frame.picks.iter().map(|pick| {
                let id = match pick.pixel[0] {
                    0..50 => hero.get(),
                    70.. => ghost.get(),
                    _ => 0,
                };
                (pick.tag, id)
            }));
        };
        let (picked, names) = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            back,
        );
        let picked = picked.map(|picked| (picked.id, picked.name));
        let expected = [
            (Some(hero.to_string()), Some("Hero".to_owned())),
            (None, None),
            (Some(ghost.to_string()), None),
            (None, None),
        ];
        assert_eq!(picked, expected);
        assert_eq!(
            names,
            [None, Some("Hero".to_owned())],
            "`view`, then `read:Name` too"
        );
        assert!(api.picks.is_empty());
    }

    #[test]
    fn requests_parse_into_calls_or_are_refused_with_a_code() {
        let request = |method: &str, params: Value| Request {
            id: 1,
            method: method.to_owned(),
            params,
        };
        assert_eq!(parse(&request("step", Value::Null)), Ok(Call::Step(1)));
        assert_eq!(parse(&request("play", Value::Null)), Ok(Call::Play));
        assert_eq!(parse(&request("stop", json!({}))), Ok(Call::Stop));
        let entities = Call::Entities {
            with: Some("Sprite".to_owned()),
            skip: 0,
            take: 100,
        };
        let given = json!({"with": "Sprite"});
        assert_eq!(parse(&request("entities", given)), Ok(entities));
        let assets = Call::Assets {
            kind: Some(AssetKind::Model),
            skip: 2,
            take: 100,
        };
        let given = json!({"kind": "model", "skip": 2});
        assert_eq!(parse(&request("assets", given)), Ok(assets));
        assert_eq!(parse(&request("ui", Value::Null)), Ok(Call::Ui));
        // As `dx tools hotpatch` prints it, the map's keys as strings, its library a path from the
        // root.
        let lib = std::env::temp_dir().join("patch.dll");
        let table = json!({
            "lib": lib,
            "map": {"4096": 8192},
            "aslr_reference": 1,
            "new_base_address": 2,
            "ifunc_count": 0,
        });
        let jump = JumpTable {
            lib,
            map: [(4096, 8192)].into_iter().collect(),
            aslr_reference: 1,
            new_base_address: 2,
            ifunc_count: 0,
        };
        let given = json!({ "table": table });
        assert_eq!(parse(&request("patch", given)), Ok(Call::Patch(jump)));
        let own = Call::Capture {
            path: None,
            own: true,
        };
        assert_eq!(parse(&request("capture", json!({"own": true}))), Ok(own));
        let pick = parse(&request("pick", json!({"at": [3, 4.5]})));
        assert_eq!(pick, Ok(Call::Pick([3.0, 4.5])));
        let saved = |path: Option<&str>| {
            Ok(Call::SavePrefab {
                id: "00000000000000ff".parse().expect("an ID"),
                path: path.map(PathBuf::from),
            })
        };
        let given = json!({"id": "00000000000000ff"});
        assert_eq!(parse(&request("save_prefab", given)), saved(None));
        let given = json!({"id": "00000000000000ff", "path": "prefabs/Lamp.PREFAB"});
        assert_eq!(
            parse(&request("save_prefab", given)),
            saved(Some("prefabs/Lamp.PREFAB"))
        );
        for (params, code) in [
            (
                json!({"id": "00000000000000ff", "path": "lamp.ron"}),
                "api.bad_params",
            ),
            (json!({"path": "lamp.prefab"}), "api.bad_params"),
            (json!({"id": "ff"}), "ecs.bad_id"),
        ] {
            let refused = parse(&request("save_prefab", params.clone())).err();
            assert_eq!(
                refused.map(|failure| failure.code),
                Some(code.to_owned()),
                "{params}"
            );
        }
        let scene = |action, path: Option<&str>, discard| {
            let path = path.map(PathBuf::from);
            Ok(Call::Scene(Scene {
                action,
                path,
                discard,
            }))
        };
        for (params, call) in [
            (
                json!({"action": "new"}),
                scene(SceneAction::New, None, false),
            ),
            (
                json!({"action": "new", "discard": true}),
                scene(SceneAction::New, None, true),
            ),
            (
                json!({"action": "open", "path": "scenes/Level2.RON", "discard": true}),
                scene(SceneAction::Open, Some("scenes/Level2.RON"), true),
            ),
            (
                json!({"action": "save"}),
                scene(SceneAction::Save, None, false),
            ),
            (
                json!({"action": "save", "path": "scenes/level2.ron"}),
                scene(SceneAction::Save, Some("scenes/level2.ron"), false),
            ),
        ] {
            assert_eq!(parse(&request("scene", params.clone())), call, "{params}");
        }
        assert_eq!(
            parse(&request("look", Value::Null)),
            Ok(Call::Look(gg_api::Look::default())),
            "reads"
        );
        let viewpoint = |rotation: [f32; 4], projection: Value| json!({"position": [0, 0, 0], "rotation": rotation, "projection": projection});
        let orthographic = json!({"Orthographic": {"height": 2, "near": -1, "far": 1}});
        let turned = viewpoint([0.0, 0.0, 3.0, 4.0], orthographic.clone());
        let Ok(Call::Look(look)) = parse(&request("look", json!({ "viewpoint": turned }))) else {
            panic!("a look");
        };
        let rotation = look.viewpoint.flatten().map(|viewpoint| viewpoint.rotation);
        assert_eq!(rotation, Some([0.0, 0.0, 0.6, 0.8]), "normalized");
        let outline = |id: &str, color: [f32; 4], width: f32| json!({"outline": {"id": id, "color": color, "width": width}});
        let id = "5f3a9c0e12b47d68";
        let half = json!({"table": {"lib": "patch.dll"}});
        let refused = parse(&request("patch", half.clone())).err();
        let message = refused.map(|failure| failure.message).unwrap_or_default();
        assert!(message.contains("`map`"), "says what's missing: {message}");
        let extra = json!({"table": table, "lib": "x"});
        let mut relative = table.clone();
        relative["lib"] = json!("patch.dll");
        for (method, params, code) in [
            ("status", json!({"x": 1}), "api.bad_params"),
            ("stop", json!({"x": 1}), "api.bad_params"),
            ("step", json!({"ticks": 0}), "api.bad_params"),
            ("step", json!({"ticks": 10_001}), "api.bad_params"),
            ("step", json!({"tick": 2}), "api.bad_params"),
            ("capture", json!({"path": "relative.png"}), "api.bad_params"),
            ("capture", json!({"bare": true}), "api.bad_params"),
            ("scene", Value::Null, "api.bad_params"),
            ("scene", json!({"action": "close"}), "api.bad_params"),
            ("scene", json!({"action": "open"}), "api.bad_params"),
            (
                "scene",
                json!({"action": "new", "path": "scenes/a.ron"}),
                "api.bad_params",
            ),
            (
                "scene",
                json!({"action": "save", "discard": true}),
                "api.bad_params",
            ),
            (
                "scene",
                json!({"action": "open", "path": "scenes/a.txt"}),
                "api.bad_params",
            ),
            (
                "scene",
                json!({"action": "save", "path": "scenes/ron"}),
                "api.bad_params",
            ),
            ("view", json!({"memory": 3}), "api.bad_params"),
            // A viewpoint whose rotation has no length, too far for an `f32`, or that can't
            // project; an outline of no entity's ID, of a color that isn't finite, or of no width.
            (
                "look",
                json!({"viewpoint": viewpoint([0.0; 4], orthographic.clone())}),
                "api.bad_params",
            ),
            (
                "look",
                json!({"viewpoint": {"position": [1e39, 0, 0], "rotation": [0, 0, 0, 1],
                    "projection": orthographic.clone()}}),
                "api.bad_params",
            ),
            (
                "look",
                json!({"viewpoint": viewpoint([0.0, 0.0, 0.0, 1.0],
                    json!({"Perspective": {"fov": 4, "near": 0.1}}))}),
                "api.bad_params",
            ),
            (
                "look",
                outline("5F3A9C0E12B47D68", [1.0; 4], 2.0),
                "api.bad_params",
            ),
            (
                "look",
                json!({"outline": {"id": id, "color": [1e39, 0, 0, 1], "width": 2}}),
                "api.bad_params",
            ),
            ("look", outline(id, [1.0; 4], 0.0), "api.bad_params"),
            ("look", json!({"grid": "xw"}), "api.bad_params"),
            ("pick", Value::Null, "api.bad_params"),
            ("pick", json!({"at": [1]}), "api.bad_params"),
            ("input", Value::Null, "api.bad_params"),
            (
                "input",
                json!({"events": [{"key": {"key": "W", "pressed": true}}]}),
                "api.bad_params",
            ),
            ("entities", json!({"take": 10_001}), "api.bad_params"),
            ("assets", json!({"take": 10_001}), "api.bad_params"),
            ("assets", json!({"kind": "sound"}), "api.bad_params"),
            ("ui", json!({"name": "menu"}), "api.bad_params"),
            ("entity", json!({"id": "nope"}), "ecs.bad_id"),
            ("patch", Value::Null, "api.bad_params"),
            ("patch", json!({ "table": relative }), "api.bad_params"),
            ("patch", extra, "api.bad_params"),
            ("patch", half, "api.bad_params"),
            ("hello", json!({"token": "x"}), "api.bad_request"),
            ("bogus", Value::Null, "api.unknown_method"),
        ] {
            let refused = parse(&request(method, params)).err();
            let refused = refused.map(|failure| failure.code);
            assert_eq!(refused.as_deref(), Some(code), "{method}");
        }
    }
}
