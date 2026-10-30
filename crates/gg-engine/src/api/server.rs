//! The API's server: it listens on the machine's loopback, at a port the OS picks, which the session
//! file in the `sessions` directory beside the executable gives with a token. A thread accepts
//! connections, and each has a thread that reads its requests, answering `hello`, `ping` and `logs`
//! itself, `logs` from the events the run's log keeps, and passing the rest on to the main loop,
//! and one that writes its answers. The main loop answers them as a frame starts, before its ticks,
//! so that a `patch` applies between ticks; a `step` once its ticks have run; a `capture` once a
//! frame that rendered brings its view back, whose PNG a thread of its own writes; and a `snapshot`
//! once a thread of its own has written the copy of the world the main thread took.

use std::fmt::Display;
use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io::{BufReader, Write as _};
use std::mem;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use gg_api::view::{Pacer, Writer};
use gg_api::{
    Applied, Apply, Authored, Button, Capture, Captured, EntityList, Failure, Field, Hash, Hello,
    History, Input, InputEvent, Listed, Patched, Pulse, Request, Session, Snapshotted, Status,
    Step, View, Welcome,
};
use gg_core::log::Events;
use gg_core::{Error, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use subsecond::JumpTable;
use tracing::{Level, debug, info, warn};

use super::{Waiting, json};
use crate::edits::{Done, Edit};
use crate::layer::{Offered, Stack};
use crate::main_loop::Stepper;
use crate::net::Ticket;
use crate::reflect::{self, Ident, Type};
use crate::render_thread::{Extracted, Publish, Shot};
use crate::scene_files::SceneFile;
use crate::{
    Context, Entity, EntityId, Event, Key, MouseButton, Name, Operation, Scroll, World, patch,
    snapshot,
};

/// The methods, as `api.unknown_method` lists them.
const METHODS: [&str; 22] = [
    "hello", "ping", "status", "pause", "resume", "step", "capture", "view", "input", "entities",
    "entity", "get", "types", "hash", "snapshot", "patch", "apply", "undo", "redo", "edits",
    "logs", "quit",
];

/// The client a connection whose `hello` names none is, the author of its edits.
const UNNAMED: &str = "api";

/// The most ticks a `step` runs.
const MOST_STEPS: u64 = 10_000;

/// The most entities an `entities` page lists.
const MOST_LISTED: usize = 10_000;

/// How long a connection has to say hello before it's dropped.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest a `logs` waits for an event to come, in milliseconds.
const MOST_WAIT_MS: u64 = 10_000;

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
    /// The requests the connections' threads pass on, for the main loop to answer.
    incoming: Receiver<Incoming>,
    /// A request taken while waiting for one, answered first at the next turn.
    held: Option<Incoming>,
    shared: Arc<Shared>,
    /// `step`s waiting for the simulation to have run as many ticks as each says.
    steps: Vec<(Reply, u64)>,
    /// `capture`s waiting for the next frame handed off, each with where it goes.
    asked: Vec<(Reply, Option<PathBuf>)>,
    /// `capture`s riding with the frame handed off last, which shows the world after
    /// `riding_ticks` ticks, until a frame that rendered brings its view back.
    riding: Vec<(Reply, Option<PathBuf>)>,
    riding_ticks: u64,
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

/// What an edit or a patch offered to a lockstep session was asked as, which its answer reads as.
enum Offer {
    Apply,
    Step { author: String },
    Patch,
}

/// What the main loop shares with the connections' threads.
struct Shared {
    /// The frames that have started, which `ping` answers.
    frames: AtomicU64,
    /// What wakes the main loop as a request comes, so that it answers while frames pause.
    waker: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// The events of the run's log that `logs` reads, and the number drawn at random that tells
    /// its cursors from another run's.
    events: &'static Events,
    run: u64,
}

/// A request for the main loop to answer, the client of its connection, and where its answer goes.
struct Incoming {
    call: Call,
    client: Arc<str>,
    reply: Reply,
}

/// The requests the main loop answers, as read.
#[derive(Debug, PartialEq)]
enum Call {
    Status,
    Pause,
    Resume,
    Step(u64),
    Capture(Option<PathBuf>),
    /// The name of the memory to publish the view to, or `None` to stop.
    View(Option<String>),
    /// Events to hand the layers, each of which names a key that's a key's.
    Input(Vec<InputEvent>),
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
    Snapshot(Option<PathBuf>),
    Patch(JumpTable),
    /// An edit's operations, and the gesture it's part of, if it's a set of one.
    Apply(Vec<gg_api::Operation>, Option<u64>),
    /// An undo, or a redo, of the edits of the author named, or else of the client's own.
    Undo {
        author: Option<String>,
        redo: bool,
    },
    Edits(Option<String>),
    Quit,
}

/// Where a request's answer goes: its connection's writer, which drops it once the connection has
/// gone.
struct Reply {
    id: u64,
    to: Sender<String>,
}

impl Reply {
    fn send(self, answer: Result<String, Failure>) {
        let _ = self
            .to
            .send(gg_api::answer_line(self.id, answer.as_deref()));
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
    /// The scene files the layers loaded, which the restored run watches.
    scenes: Vec<SceneFile>,
    ticks: u64,
    path: PathBuf,
    reply: Reply,
}

/// A capture's PNG for the thread that writes them: the view, where it goes, the ticks the frame
/// that shows it had run, and where the answer goes.
struct Png {
    shot: Arc<Shot>,
    path: PathBuf,
    ticks: u64,
    reply: Reply,
}

impl Api {
    /// Starts serving app `app`'s run, whose layers are `layers`, from the bottom up, whose JSON
    /// log is at `log`, if it has one, and keeps its last events in `events`, which plays `player`
    /// in a lockstep session, if it's a peer of one, and whose project's manifest is at `project`,
    /// if it's a project's game: listens on the loopback, starts its threads and writes the session
    /// file.
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
            waker: Mutex::new(None),
            events,
            run,
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
        fs::create_dir_all(&beside)
            .and_then(|()| crate::dirs::write_whole(&path, text.as_bytes()))
            .map_err(failed(format!(
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
            held: None,
            shared,
            steps: Vec::new(),
            asked: Vec::new(),
            riding: Vec::new(),
            riding_ticks: 0,
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

    /// As a frame starts, before its ticks, answers the requests that have come, from `cx`, whose
    /// layers are `stack`'s and whose simulation `stepper` steps: first the `step`s whose ticks have
    /// run, then the rest in the order they came, `capture`s once a frame has rendered the view, as
    /// [`frame_back`](Self::frame_back) says, and `snapshot`s once their files are written, but
    /// refused before the run's first tick. Returns whether one asked the run to end.
    pub(crate) fn serve(
        &mut self,
        cx: &mut Context,
        stack: &mut Stack,
        stepper: &mut Stepper,
    ) -> bool {
        self.shared.frames.fetch_add(1, Ordering::Relaxed);
        self.answer(cx, stack, stepper, false)
    }

    /// As [`serve`](Self::serve), between frames while they pause for a minimized window, without
    /// counting a frame: a `capture` or a `step`, which need frames, fail with `api.paused`.
    pub(crate) fn serve_paused(
        &mut self,
        cx: &mut Context,
        stack: &mut Stack,
        stepper: &mut Stepper,
    ) -> bool {
        self.answer(cx, stack, stepper, true)
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
        let applied = self
            .offered
            .extract_if(.., |(_, ticket, _)| stack.offered_is_done(*ticket));
        for (reply, ticket, offer) in applied.collect::<Vec<_>>() {
            if let Some((ticks, done)) = stack.offered(ticket) {
                answer_done(reply, &offer, ticks, done);
            }
        }
        let mut quit = false;
        while let Some(Incoming {
            call,
            client,
            reply,
        }) = (self.held.take()).or_else(|| self.incoming.try_recv().ok())
        {
            let world = cx.world();
            match call {
                Call::Step(_) | Call::Capture(_) if paused => {
                    let message = "frames have paused while the window is minimized, and steps and \
                                   captures run in frames; restore the window first";
                    reply.send(Err(Failure::new("api.paused", message)));
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
                Call::Step(ticks) => {
                    stepper.step_by(ticks);
                    let until = stepper.ticks + stepper.steps();
                    self.steps.push((reply, until));
                }
                Call::Capture(path) => self.asked.push((reply, path)),
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
                Call::Snapshot(_) if stepper.ran() == 0 => {
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
                    let taken = Taken {
                        world: stack.saved_world(cx).clone(),
                        scenes: cx.scenes.files.clone(),
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
                Call::Apply(given, gesture) => match operations(world, given) {
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

    /// Answers `reply`, asked as `offer`, with what an edit did, if it applied now, the next tick
    /// to run being `ticks`; or keeps it until the tick it applies before in a lockstep session.
    fn answer_offered(&mut self, reply: Reply, offered: Offered, offer: Offer, ticks: u64) {
        match offered {
            Offered::Now(done) => answer_done(reply, &offer, ticks, done),
            Offered::Later(ticket) => self.offered.push((reply, ticket, offer)),
        }
    }

    /// The game as the frame that answers starts.
    fn status(&self, cx: &Context, stepper: &Stepper) -> Status {
        let entities = cx.world().entities(None);
        Status {
            ticks: stepper.ticks,
            paused: stepper.paused(),
            frames: self.shared.frames.load(Ordering::Relaxed),
            entities: entities.map_or(0, |entities| entities.len()),
            view: cx.view().map(|view| view.size),
        }
    }

    /// With the renderer back from a frame, `frame`, takes the view it read back, if it rode with
    /// captures and rendered, for a thread of its own to write each capture's PNG and answer it;
    /// and has the next frame handed off, which shows the world after `ticks` ticks and started at
    /// `started`, read its view back for the captures asked since, and for those whose frame had
    /// nothing to render into, and publish it to the memory a `view` asked for, warning once of a
    /// frame that wasn't, since the view was asked for.
    pub(crate) fn frame_back(&mut self, frame: &mut Extracted, ticks: u64, started: SystemTime) {
        if let Some(viewing) = self.viewing.take() {
            frame.publish = viewing.map(|writer| Publish {
                writer,
                ticks,
                started,
                failed: None,
            });
        }
        if let Some(publish) = &mut frame.publish {
            if let Some(error) = publish.failed.take()
                && !mem::replace(&mut self.view_warned, true)
            {
                let consequence = "frames go unpublished while that lasts";
                warn!(code = error.code(), "{error:#}; {consequence}");
            }
            publish.ticks = ticks;
            publish.started = started;
        }
        match frame.captured.take() {
            Some(Ok(shot)) => {
                let shot = Arc::new(shot);
                for (reply, path) in self.riding.drain(..) {
                    let path = path.unwrap_or_else(|| {
                        self.numbered += 1;
                        let name = format!("{}-{}.png", self.stem, self.numbered);
                        self.captures.join(name)
                    });
                    let png = Png {
                        shot: shot.clone(),
                        path,
                        ticks: self.riding_ticks,
                        reply,
                    };
                    // The thread runs until the API drops.
                    let _ = self.pngs.send(png);
                }
            }
            Some(Err(error)) => {
                let failure = failure(&error);
                for (reply, _) in self.riding.drain(..) {
                    reply.send(Err(failure.clone()));
                }
            }
            None => {}
        }
        self.riding.append(&mut self.asked);
        frame.capture = !self.riding.is_empty();
        if frame.capture {
            self.riding_ticks = ticks;
        }
    }

    /// Whether the simulation is paused with nothing asked of the frames: no ticks to step, no
    /// capture waiting and no request held.
    pub(crate) fn is_idle(&self, stepper: &Stepper) -> bool {
        stepper.paused()
            && stepper.steps() == 0
            && self.asked.is_empty()
            && self.riding.is_empty()
            && self.held.is_none()
    }

    /// While the view is published, the number of the frame to be handed off next, as the client
    /// will take it.
    pub(crate) fn handing(&self) -> Option<u64> {
        self.pacer
            .as_ref()
            .map(|pacer| pacer.published().saturating_add(1))
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
        }
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
        (Offer::Apply, Ok(Done::Stepped(_))) => unreachable!("an apply applies"),
        (Offer::Step { .. }, Ok(Done::Applied(_))) => unreachable!("an undo or a redo steps"),
    }
}

/// The page of `world`'s entities `entities` asks for.
fn list(world: &World, with: Option<&str>, skip: usize, take: usize) -> Result<String, Failure> {
    let mut listing = world.entities(with).map_err(|error| failure(&error))?;
    let total = listing.len();
    let page = listing.by_ref().skip(skip).take(take);
    let entities = page.map(|entity| listed(world, entity)).collect();
    let list = EntityList { total, entities };
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
    out.push_str("}}");
    Ok(out)
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

/// The engine's operations `given` asks for, their values read by the types of `world`'s components
/// and the fields they go in, as [`json::read`] reads them.
///
/// Fails, naming the operation, with `ecs.bad_id` for an ID that isn't one, `reflect.unknown_type`
/// for a component type that isn't registered, as a path to a field it hasn't fails, and as
/// `json::read` fails for a value.
fn operations(world: &World, given: Vec<gg_api::Operation>) -> Result<Vec<Operation>, Failure> {
    let count = given.len();
    let operations = given.into_iter().enumerate().map(|(i, operation)| {
        operation_of(world, operation).map_err(|error| {
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
    operations.collect()
}

/// The engine's operation `given` asks for, as [`operations`] reads it.
fn operation_of(world: &World, given: gg_api::Operation) -> Result<Operation> {
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
        json::read(json, &ty, world)
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
        Given::Reparent { id: of, parent } => Operation::Reparent {
            entity: id(&of)?,
            parent: maybe(parent)?,
        },
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

/// Accepts connections on `listener`, serving each on threads of its own, as [`connection`] does,
/// until the process ends.
fn accept(
    listener: &TcpListener,
    token: &str,
    welcome: &str,
    to_main: &Sender<Incoming>,
    shared: &Arc<Shared>,
) {
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
        let (token, welcome) = (token.to_owned(), welcome.to_owned());
        let (to_main, shared) = (to_main.clone(), shared.clone());
        let serving = thread::Builder::new()
            .name("gg-api-read".to_owned())
            .spawn(move || connection(stream, &token, &welcome, &to_main, &shared));
        if let Err(error) = serving {
            debug!(code = "api.accept", "Can't serve a connection: {error}");
        }
    }
}

/// Serves connection `stream`: its first request must be `hello` with `token`, which it answers
/// with `welcome`; then it answers `ping` and `logs` itself, and passes the rest on to the main
/// loop through `to_main`, waking it, each answer going back through a thread that writes them as
/// they come. A line that isn't a request ends it.
fn connection(
    stream: TcpStream,
    token: &str,
    welcome: &str,
    to_main: &Sender<Incoming>,
    shared: &Shared,
) {
    let _ = stream.set_nodelay(true);
    let Ok(writing) = stream.try_clone() else {
        return;
    };
    let (to, answers) = mpsc::channel::<String>();
    let writer = thread::Builder::new()
        .name("gg-api-write".to_owned())
        .spawn(move || write_answers(writing, &answers));
    if writer.is_err() {
        return;
    }
    let mut reader = BufReader::new(stream);
    let _ = reader.get_ref().set_read_timeout(Some(HELLO_TIMEOUT));
    let Some(hello) = next_request(&mut reader) else {
        return;
    };
    let reply = Reply {
        id: hello.id,
        to: to.clone(),
    };
    let given = (hello.method == "hello")
        .then(|| serde_json::from_value::<Hello>(hello.params).ok())
        .flatten();
    let Some(Hello { client, .. }) = given.filter(|hello| hello.token == token) else {
        let message = "a connection's first request is `hello`, with the session file's token";
        reply.send(Err(Failure::new("api.unauthorized", message)));
        return;
    };
    let client: Arc<str> = client.as_deref().unwrap_or(UNNAMED).into();
    reply.send(Ok(welcome.to_owned()));
    let _ = reader.get_ref().set_read_timeout(None);
    while let Some(request) = next_request(&mut reader) {
        let reply = Reply {
            id: request.id,
            to: to.clone(),
        };
        if request.method == "ping" {
            let pulse = Pulse {
                frames: shared.frames.load(Ordering::Relaxed),
                waiting: Waiting::now().map(|waiting| waiting.name().to_owned()),
            };
            reply.result(&pulse);
            continue;
        }
        if request.method == "logs" {
            reply.send(logs(&request.params, shared));
            continue;
        }
        match parse(&request) {
            Ok(call) => {
                let client = client.clone();
                let incoming = Incoming {
                    call,
                    client,
                    reply,
                };
                if let Err(mpsc::SendError(incoming)) = to_main.send(incoming) {
                    let message = "the game is ending";
                    incoming
                        .reply
                        .send(Err(Failure::new("api.closed", message)));
                    return;
                }
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

/// The next request `reader` reads: `None` once the connection ends or breaks, or at a line that
/// isn't a request.
fn next_request(reader: &mut BufReader<TcpStream>) -> Option<Request> {
    let line = gg_api::read_line(reader).ok()??;
    serde_json::from_str(&line)
        .inspect_err(|error| {
            debug!(
                code = "api.bad_request",
                "A tool sent what isn't a request, so its connection closes: {error}"
            );
        })
        .ok()
}

/// Writes the lines `answers` brings to `stream`, until the connection breaks or nothing more can
/// come.
fn write_answers(mut stream: TcpStream, answers: &Receiver<String>) {
    for line in answers {
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
        ticks,
        path,
        reply,
    } in taken
    {
        let written = snapshot::write(&world, &scenes, app, &layers, ticks).and_then(|text| {
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

/// The call `request` makes of the main loop.
///
/// Fails with `api.unknown_method` for a method there's none of, `api.bad_params` for parameters
/// it doesn't take, `api.bad_request` for a `hello` after the first, and `ecs.bad_id` for an ID
/// that isn't one.
fn parse(request: &Request) -> Result<Call, Failure> {
    let method = request.method.as_str();
    let params = &request.params;
    let call = match method {
        "status" => none(method, params).map(|()| Call::Status)?,
        "pause" => none(method, params).map(|()| Call::Pause)?,
        "resume" => none(method, params).map(|()| Call::Resume)?,
        "types" => none(method, params).map(|()| Call::Types)?,
        "hash" => none(method, params).map(|()| Call::Hash)?,
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
        "undo" | "redo" => Call::Undo {
            author: take::<gg_api::Undo>(method, params)?.author,
            redo: method == "redo",
        },
        "edits" => Call::Edits(take::<gg_api::Edits>(method, params)?.author),
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
            let Capture { path } = take(method, params)?;
            if path.as_deref().is_some_and(|path| !path.is_absolute()) {
                let what = "takes an absolute path, since the game's working directory may not be \
                            the tool's";
                return Err(bad_params(method, what));
            }
            Call::Capture(path)
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
    use std::time::Instant;

    use gg_api::{Client, LogEvents, Session};
    use gg_core::math::Vec3;
    use serde_json::json;

    use super::*;
    use crate::dirs::tests::Scratch;
    use crate::{Sprite, Tick, Transform, Viewport};

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

    /// Runs `client` on a thread of its own, connected to `session`'s API, while this thread serves
    /// `api` from `cx` as frames would, each a tick's time, stepping `stepper` and then doing
    /// `frame`; returns what `client` returns.
    fn serving<T: Send + 'static>(
        api: &mut Api,
        cx: &mut Context,
        stepper: &mut Stepper,
        session: Session,
        client: impl FnOnce(Client) -> T + Send + 'static,
        mut frame: impl FnMut(&mut Api, &Stepper),
    ) -> T {
        let client = thread::spawn(move || {
            client(Client::connect(&session, "test", DEADLINE).expect("it connects"))
        });
        let start = Instant::now();
        let mut stack = Stack::default();
        while !client.is_finished() {
            assert!(start.elapsed() < DEADLINE, "the client never finished");
            api.serve(cx, &mut stack, stepper);
            stepper.step(Tick::DT, |_| true, || Duration::ZERO);
            frame(api, stepper);
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
        field: f32,
        types: gg_api::Types,
        hash: Hash,
        refused: Vec<String>,
    }

    /// A client pauses the simulation, steps it, and reads the world: its status, pages of its
    /// entities, an entity whole, a field, its types, whose description describes them again as
    /// they are, and the hash; and is refused, with a code, what's wrong.
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
        let asked = hero.clone();
        let client = move |mut client: Client| {
            let hero = asked;
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
                field: client.call_as("get", field).expect("its y"),
                types: client.call_as("types", ()).expect("the types"),
                hash: client.call_as("hash", ()).expect("a hash"),
                refused: (refused.into_iter())
                    .map(|(method, params)| client.call(method, params).expect_err(method).code)
                    .collect(),
            }
        };
        let read = serving(&mut api, &mut cx, &mut stepper, session, client, |_, _| {});
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
        let (refused, history, undone) =
            serving(&mut api, &mut cx, &mut stepper, session, client, |_, _| {});
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
        let edited = serving(&mut api, &mut cx, &mut stepper, session, client, |_, _| {});
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
        let (refused, taken) = serving(&mut api, &mut cx, &mut stepper, session, client, |_, _| {});
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
        let refused = serving(&mut api, &mut cx, &mut stepper, session, client, |_, _| {});
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
            &mut stepper,
            session,
            client,
            |api, stepper| {
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
                api.frame_back(&mut frame, stepper.ticks, SystemTime::UNIX_EPOCH);
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
        serving(&mut api, &mut cx, &mut stepper, session, client, |_, _| {});
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
        let back = |api: &mut Api, stepper: &Stepper| {
            api.frame_back(&mut frame, stepper.ticks, SystemTime::UNIX_EPOCH);
        };
        let (reader, refused) = serving(
            &mut api,
            &mut cx,
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
        let back = |api: &mut Api, stepper: &Stepper| {
            api.frame_back(&mut frame, stepper.ticks, SystemTime::UNIX_EPOCH);
        };
        serving(&mut api, &mut cx, &mut stepper, session, client, back);
        assert!(frame.publish.is_none());
        assert_eq!(api.handing(), None);
        drop(reader);
    }

    #[test]
    fn requests_parse_into_calls_or_are_refused_with_a_code() {
        let request = |method: &str, params: Value| Request {
            id: 1,
            method: method.to_owned(),
            params,
        };
        assert_eq!(parse(&request("step", Value::Null)), Ok(Call::Step(1)));
        let entities = Call::Entities {
            with: Some("Sprite".to_owned()),
            skip: 0,
            take: 100,
        };
        let given = json!({"with": "Sprite"});
        assert_eq!(parse(&request("entities", given)), Ok(entities));
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
        let half = json!({"table": {"lib": "patch.dll"}});
        let refused = parse(&request("patch", half.clone())).err();
        let message = refused.map(|failure| failure.message).unwrap_or_default();
        assert!(message.contains("`map`"), "says what's missing: {message}");
        let extra = json!({"table": table, "lib": "x"});
        let mut relative = table.clone();
        relative["lib"] = json!("patch.dll");
        for (method, params, code) in [
            ("status", json!({"x": 1}), "api.bad_params"),
            ("step", json!({"ticks": 0}), "api.bad_params"),
            ("step", json!({"ticks": 10_001}), "api.bad_params"),
            ("step", json!({"tick": 2}), "api.bad_params"),
            ("capture", json!({"path": "relative.png"}), "api.bad_params"),
            ("view", json!({"memory": 3}), "api.bad_params"),
            ("input", Value::Null, "api.bad_params"),
            (
                "input",
                json!({"events": [{"key": {"key": "W", "pressed": true}}]}),
                "api.bad_params",
            ),
            ("entities", json!({"take": 10_001}), "api.bad_params"),
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
