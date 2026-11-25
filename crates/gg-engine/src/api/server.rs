//! The API's server. It listens on the loopback at a port the OS picks; the session file in the
//! `sessions` directory beside the executable gives the port and a token.
//!
//! A thread accepts connections. Each has a thread that reads its requests, at most
//! [`gg_api::MOST_IN_FLIGHT`] ahead of the answers written, and answers `hello`, `ping`, `logs` and
//! `note` itself; once its `hello` is granted, a second thread writes its answers. The rest go to
//! the main loop.
//!
//! The main loop answers the requests that came before a frame started as it starts, before its
//! ticks, so a `patch` applies between ticks, a `play`, `stop` or `scene` holds from the frame's
//! ticks, and a `look` reaches the frame. Later:
//!
//! - `step`: once its ticks have run.
//! - `capture`: once a frame that rendered brings its view back; a thread writes the PNG.
//! - `pick`: once the frame that read it brings its answer back.
//! - `snapshot`: once a thread has written the copy of the world the main thread took.
//!
//! A capture of the game's own view rides with a bare frame: drawn without the look, not published
//! to a view's memory, and leaving the picks to the next non-bare frame, so no pick reads a view a
//! client isn't shown. A windowed game shows bare frames in its window for a frame or two.

mod connection;
mod edits;
mod files;
mod parse;
mod view;
mod world;

pub use view::{api_viewpoint, engine_viewpoint};
pub(crate) use world::instanced;

use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use gg_api::view::{Pacer, Writer};
use gg_api::{
    Authored, By, CellCounts, Failure, Hash, History, Looked, Scene, SceneAction, Session, Status,
    Welcome,
};
use gg_core::log::Events;
use gg_core::{Error, Result};
use gg_renderer::MAX_PICKS;
use tracing::{info, warn};

use super::{Shown, json};
use crate::ecs::Operation;
use crate::edits::{Done, Edit};
use crate::layer::Stack;
use crate::luau::Debugger;
use crate::main_loop::Stepper;
use crate::net::Ticket;
use crate::scene_files;
use crate::snapshot::Run;
use crate::{Context, patch};
use connection::{Reply, accept};
use edits::{Offer, answer_done, assets_named, operations, save_prefab, words};
use files::{Taken, Writing, write_pngs, write_snapshots};
use parse::{Call, bad_params};
use view::{Capturing, Filming, Picking, input_event};
use world::{Listing, assets, entity, field, list};

/// The most of the clients' picks a frame asks: half what a frame reads back, leaving the game's
/// layers the rest.
const MOST_PICKS: usize = MAX_PICKS / 2;

/// The longest a run that ends after answering a `snapshot` with `quit` waits for the answer to be
/// written to its connection. A client that doesn't read holds it that long.
const ANSWERING: Duration = Duration::from_secs(5);

/// The tool API, from its start until the run ends; dropping it removes the session file.
pub(crate) struct Api {
    session: PathBuf,
    /// Where captures without a path go, and how their names start.
    captures: PathBuf,
    stem: String,
    /// The requests the connections' threads pass on, and how many the main loop has received.
    incoming: Receiver<Incoming>,
    received: u64,
    /// A request taken while waiting, answered first at the next turn.
    held: Option<Incoming>,
    shared: Arc<Shared>,
    /// `step`s waiting for the simulation to have run their ticks.
    steps: Vec<(Reply, u64)>,
    /// `assets` waiting for the asset root's watcher thread to have looked since each came.
    listing: Vec<Listing>,
    /// `capture`s waiting for a frame handed off of their kind, bare or not, in order of arrival.
    asked: Vec<Capturing>,
    /// `capture`s riding with the frame handed off last, all of its kind, which shows the world
    /// after `riding_ticks` ticks, until a frame that rendered brings its view back.
    riding: Vec<Capturing>,
    riding_ticks: u64,
    /// `frames` in order of arrival: the first captures, the rest wait. `film_frame`: the frame
    /// being made is the first's, drawn from its viewpoint if it gives them. `film_viewpoint`: the
    /// last frame drew from a film's viewpoint, not the look's.
    films: Vec<Filming>,
    film_frame: bool,
    film_viewpoint: bool,
    /// Whether the frame being made is bare, and what it shows in place of the view's light, from
    /// the film capturing, else the oldest capture waiting, decided as the frame started. Such a
    /// frame is apart, not published.
    bare: bool,
    shown: Option<Shown>,
    /// The look as clients set it, each part `None` where it's the game's own; `seen` is filled in
    /// as it's answered.
    look: Looked,
    picks: Vec<Picking>,
    /// `images` waiting for a frame handed off, and those riding with frames handed off since,
    /// until one that rendered brings its images back.
    images_asked: Vec<Reply>,
    images_riding: Vec<Reply>,
    /// The captures written without a path, which number their files.
    numbered: u64,
    /// The memory a `view` asked to publish to, or `None` to stop, until the next frame back takes
    /// it to the render thread.
    viewing: Option<Option<Writer>>,
    /// The memory published to or asked for; a `view` naming it again changes nothing.
    memory: Option<String>,
    /// Paces frames by the client's taking them, while the view is published.
    pacer: Option<Pacer>,
    /// Whether an unpublished frame has been warned of since the view was asked for.
    view_warned: bool,
    pngs: Sender<Writing>,
    /// Where snapshots without a path go, and how many have, which number their files.
    snapshots: PathBuf,
    snapshotted: u64,
    taken: Sender<Taken>,
    /// Edits and patches offered to a lockstep session, waiting for the tick they apply before.
    offered: Vec<(Reply, Ticket, Offer)>,
    /// The session file as written; rewritten as the run's player changes.
    written: Session,
}

/// What the main loop shares with the connections' threads.
struct Shared {
    /// The frames that have started, which `ping` answers.
    frames: AtomicU64,
    /// The requests passed on to the main loop, each counted once sent. A frame answers those
    /// that came before it started.
    passed: AtomicU64,
    /// Wakes the main loop as a request comes, so it answers while frames pause.
    waker: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// The log's events that `logs` reads, and a random number telling its cursors from another
    /// run's.
    events: &'static Events,
    run: u64,
    debugger: Arc<Debugger>,
}

/// A request for the main loop: its client, whether that client may `read` `Name` (which a `pick`
/// answers), and where the answer goes.
struct Incoming {
    call: Call,
    client: Arc<str>,
    names: bool,
    reply: Reply,
}

impl Api {
    /// Starts serving `app`'s run: listens on the loopback, starts its threads and writes the
    /// session file.
    ///
    /// `layers` are bottom up; `log` is the JSON log, if any; `player` is the run's player in a
    /// lockstep session, if a peer; `project` is the project's manifest, if a project's game.
    ///
    /// The file is readable only by the game's user, from before it holds the token (see
    /// `dirs::write_private`). The token alone keeps another process from `patch`, which loads a
    /// library into the game, and from the writes of `capture` and `snapshot`.
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
        // std seeds its hashers' keys from the OS's randomness; SipHash's output reveals nothing of
        // them.
        let random = RandomState::new();
        let token = format!("{:016x}{:016x}", random.hash_one(1), random.hash_one(2));
        // Under 2^53, so JSON readers that take every number for a double (JavaScript's, which MCP
        // clients may use) read it whole.
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
                max_fps: None,
                render_scale: None,
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

    /// Writes the session file again if `player` differs from the one it names, as a session the
    /// game hosts or joins starts or ends, so tools reach its peers as they do flag-started ones.
    /// Warns where it can't (`api.session`).
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

    pub(crate) fn set_waker(&mut self, wake: Box<dyn Fn() + Send>) {
        *self
            .shared
            .waker
            .lock()
            .expect("no thread panics holding it") = Some(wake);
    }

    pub(crate) fn debugger(&self) -> Arc<Debugger> {
        self.shared.debugger.clone()
    }

    /// As a frame starts, before its ticks, answers the requests that had come by then, so clients
    /// that keep sending can't hold the frame back: first the `step`s whose ticks have run, then
    /// the rest in order of arrival. `look`s set the look the frame draws with; `capture`s and
    /// `pick`s wait for a frame that rendered the view ([`frame_back`](Self::frame_back));
    /// `snapshot`s wait for their files, and are refused before the first tick of a run playing
    /// from its start.
    ///
    /// Then makes the frame of the kind of the oldest capture riding (its frame may not render),
    /// else of the film with frames to capture, else of the oldest capture asked. A frame of the
    /// game's own view is bare. A film's frame draws from its viewpoint, if it gives them
    /// ([`Filming::viewpoint`]); the next frame not a film's gets the look back. Unless the frame
    /// is bare, which reads no picks, it asks the waiting picks, at most [`MOST_PICKS`]; the rest
    /// wait.
    ///
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
        cx.dev.look.bare = self.bare;
        cx.dev.look.captured = self.shown.clone();
        let filming = film.filter(|film| !self.bare && self.shown == film.show);
        let filmed = filming.and_then(Filming::viewpoint);
        if let Some(viewpoint) = filmed {
            cx.dev.look.viewpoint = Some(viewpoint);
        } else if self.film_viewpoint {
            cx.dev.look.viewpoint = self.look.viewpoint.as_ref().map(engine_viewpoint);
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

    /// Like [`serve`](Self::serve), between frames while a minimized window pauses them, counting
    /// no frame. `capture`, `frames`, `pick`, `images` and `step` need frames and fail with
    /// `api.paused`.
    pub(crate) fn serve_paused(
        &mut self,
        cx: &mut Context,
        stack: &mut Stack,
        stepper: &mut Stepper,
    ) -> bool {
        self.answer(cx, stack, stepper, true)
    }

    /// As frames pause, answers each `assets` waiting for a look from `cx`'s database as it is.
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
            match call {
                // What a quit leaves would go with the run, and a snapshot that quits is what a
                // restarted run continues from: nothing past either applies.
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
                Call::Status
                | Call::Pause
                | Call::Resume
                | Call::Play
                | Call::Stop
                | Call::Scene(_)
                | Call::Step(_)
                | Call::Quit => quit = self.answer_control(cx, stack, stepper, call, reply),
                Call::Capture { .. }
                | Call::Frames { .. }
                | Call::View(_)
                | Call::Input(_)
                | Call::Look(_)
                | Call::Pick(..)
                | Call::Images => self.answer_view(cx, stack, call, &client, names, reply),
                Call::Entities { .. }
                | Call::Entity(_)
                | Call::Get { .. }
                | Call::Types
                | Call::Hash
                | Call::Assets { .. }
                | Call::Ui
                | Call::Cells
                | Call::Edits(_) => self.answer_read(cx, stack, stepper, call, paused, reply),
                Call::Snapshot { .. } => quit = self.answer_snapshot(cx, stepper, call, reply),
                Call::Patch(_) | Call::Apply(..) | Call::SavePrefab { .. } | Call::Undo { .. } => {
                    self.answer_edit(cx, stack, stepper, call, &client, reply);
                }
            }
        }
        quit
    }

    /// Answers `call`, a control of the run; returns whether it asked the run to end.
    fn answer_control(
        &mut self,
        cx: &mut Context,
        stack: &mut Stack,
        stepper: &mut Stepper,
        call: Call,
        reply: Reply,
    ) -> bool {
        let mut quit = false;
        match call {
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
            Call::Quit => {
                reply.send(Ok("{}".to_owned()));
                quit = true;
            }
            _ => unreachable!("`answer` passes the run's controls alone"),
        }
        quit
    }

    /// A pick answers names if `names`.
    fn answer_view(
        &mut self,
        cx: &mut Context,
        stack: &mut Stack,
        call: Call,
        client: &str,
        names: bool,
        reply: Reply,
    ) {
        match call {
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
                // A view of no area, or none, fails when the frames come back.
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
                self.set_look(cx, given, client);
                let seen = cx.dev.look.seen.as_ref().map(api_viewpoint);
                // The game's own camera may use non-finite numbers, which JSON can't carry.
                self.look.seen = seen.filter(gg_api::Viewpoint::is_finite);
                reply.result(&self.look);
            }
            Call::Pick(at, within) => {
                let within = within.map(|within| within.resolve(cx.world()));
                match within.transpose() {
                    Ok(within) => self.picks.push(Picking {
                        at,
                        within,
                        ticket: None,
                        named: names,
                        reply,
                    }),
                    Err(error) => reply.send(Err(failure(&error))),
                }
            }
            Call::Images => self.images_asked.push(reply),
            _ => unreachable!("`answer` passes the view's calls alone"),
        }
    }

    fn answer_read(
        &mut self,
        cx: &mut Context,
        stack: &Stack,
        stepper: &Stepper,
        call: Call,
        paused: bool,
        reply: Reply,
    ) {
        let world = cx.world();
        match call {
            Call::Entities { with, skip, take } => {
                reply.send(list(world, with.as_deref(), skip, take));
            }
            Call::Entity(id) => reply.send(match id.resolve(world) {
                Ok(id) => entity(world, cx.meshes(), id),
                Err(error) => Err(failure(&error)),
            }),
            Call::Get {
                id,
                component,
                path,
            } => reply.send(match id.resolve(world) {
                Ok(id) => field(world, id, &component, &path),
                Err(error) => Err(failure(&error)),
            }),
            Call::Types => reply.result(&json::describe(world)),
            Call::Hash => {
                let hash = world.hash();
                reply.result(&Hash {
                    content: format!("{:016x}", hash.content),
                    layout: format!("{:016x}", hash.layout),
                });
            }
            Call::Assets { kind, skip, take } => {
                // While frames pause, a look the thread takes would wait for the next request,
                // so the database as the thread last sent it answers. A run with no thread
                // watching the root scans here.
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
            Call::Edits(author) => {
                let logs = cx.authors.logs(world);
                let logs = logs.filter(|(name, _)| author.as_deref().is_none_or(|a| a == *name));
                let authors = logs.map(|(name, log)| Authored {
                    author: name.to_owned(),
                    done: log.edits().map(words).collect(),
                    undone: log.undone().map(words).collect(),
                });
                reply.result(&History {
                    authors: authors.collect(),
                });
            }
            _ => unreachable!("`answer` passes the reads alone"),
        }
    }

    /// Answers `call`, a `snapshot`; returns whether the run ends, as one that quits does once
    /// written.
    fn answer_snapshot(
        &mut self,
        cx: &Context,
        stepper: &Stepper,
        call: Call,
        reply: Reply,
    ) -> bool {
        let mut quit = false;
        match call {
            // A run stopped, or playing a copy, is snapshotted before its first tick too, so a
            // restart keeps the world as edited; what the layers queued as they attached goes,
            // as at a `stop` that brings that world back. So is one restored from a snapshot
            // (its layers attached nothing), which may not tick for a while if restored paused.
            Call::Snapshot { .. }
                if stepper.fresh() && !stepper.stopped() && cx.dev.edited.is_none() =>
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
                let run = match &cx.dev.edited {
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
                    world: cx.world().clone(),
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
                    // Written before the run ends, which ends the thread too: the main thread
                    // waits, and the snapshots asked earlier are written first.
                    let (written, done) = mpsc::channel();
                    let taken = Taken {
                        written: Some(written),
                        ..taken
                    };
                    // A failed one leaves the run going, as an untaken one does.
                    quit = self.taken.send(taken).is_ok() && done.recv().unwrap_or(false);
                    // The answer goes out before the run ends the thread that writes it, or a
                    // client would take the snapshot for a failure.
                    if quit && let Some(landing) = landing {
                        let _ = landing.recv_timeout(ANSWERING);
                    }
                } else {
                    // The thread runs until the API drops.
                    let _ = self.taken.send(taken);
                }
            }
            _ => unreachable!("`answer` passes a `snapshot` alone"),
        }
        quit
    }

    fn answer_edit(
        &mut self,
        cx: &mut Context,
        stack: &mut Stack,
        stepper: &Stepper,
        call: Call,
        client: &str,
        reply: Reply,
    ) {
        match call {
            Call::Patch(table) => match stack.offer_patch(table) {
                Ok(ticket) => self.offered.push((reply, ticket, Offer::Patch)),
                Err(table) => {
                    let patched = stack.patch(table, stepper.ticks);
                    let done = patched.map(|()| Done::Applied(Vec::new()));
                    answer_done(reply, &Offer::Patch, stepper.ticks, done);
                }
            },
            Call::Apply(given, gesture) => {
                // Assets its values name are loaded first, as a scene's are.
                let named = assets_named(cx.world(), &given);
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
            Call::SavePrefab { id, path } => match id
                .resolve(cx.world())
                .and_then(|id| Ok((id, save_prefab(cx, stack, id, path)?)))
            {
                Ok((id, saved)) => {
                    let name = cx.world().assets().prefab_name(saved.prefab).to_owned();
                    // Checked as the save wrote the file, against this world: it applies.
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
            _ => unreachable!("`answer` passes the edits alone"),
        }
    }

    /// Stops the playing game, as `stop` asks of a run that can stop: brings back the world as
    /// edited, if a copy plays, at its ticks ([`Stack::stop`]); runs no ticks from then on; fails
    /// the waiting `step`s with `api.stopped`.
    pub(super) fn stop(&mut self, cx: &mut Context, stack: &mut Stack, stepper: &mut Stepper) {
        if let Some(ticks) = stack.stop(cx) {
            stepper.jump_to(ticks);
        }
        stepper.stop();
        let message = "the game stopped before the ticks ran: its world is the world as edited";
        for (waiting, _) in self.steps.drain(..) {
            waiting.send(Err(Failure::new("api.stopped", message)));
        }
    }

    /// Does to the open scene what `scene` asks, as gg-api's docs say, in a project's game, with
    /// its path resolved ([`scene_files::resolve`]): saves the world as edited
    /// ([`scene_files::save`]); or, once [`scene_files::check_open`] or
    /// [`scene_files::check_unsaved`] passes, stops a copy that plays, as `stop` does, then opens a
    /// scene file ([`scene_files::open`]) or empties the scene ([`scene_files::new`]).
    ///
    /// # Errors
    ///
    /// - `engine.no_project`: the game isn't a project's.
    /// - `api.no_scene`: the game runs from its package, which has no scene files; a save in a
    ///   lockstep peer, whose scene files must match the others'; an open or new in a run that
    ///   can't stop ([`Stack::refuses_stop`]), since neither the history they restart nor a copy's
    ///   stopping is recorded or sent to a peer.
    /// - Those of the calls above.
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
        if cx.dev.edited.is_some() {
            self.stop(cx, stack, stepper);
        }
        match opening {
            Some(opening) => scene_files::open(cx, stack, opening),
            None => scene_files::new(cx, stack),
        }
    }

    /// The game as the answering frame starts, with the open scene of the world as edited and
    /// whether it's unsaved ([`scene_files::unsaved`]).
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
            // The world as edited's, like `unsaved`.
            edited: cx
                .dev
                .edited
                .as_ref()
                .map_or(cx.scenes.open.edited, |edited| edited.open.edited),
            cells: (cx.world().cell_counts()).map(|(cells_in, kept)| CellCounts { cells_in, kept }),
        }
    }

    /// Whether the simulation is paused or stopped and nothing asks for frames: no ticks to step,
    /// no capture, `frames`, pick or `images` waiting, no request held.
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

    /// Waits for a request for at most `timeout`, holding it for the next turn to answer first.
    pub(crate) fn wait(&mut self, timeout: Duration) {
        if self.held.is_none() {
            self.held = self.incoming.recv_timeout(timeout).ok();
            self.received += u64::from(self.held.is_some());
        }
    }

    /// The next request to answer: the one held, then those of the first `came` passed on that
    /// aren't yet received; `None` once all are.
    fn next(&mut self, came: u64) -> Option<Incoming> {
        if let Some(held) = self.held.take() {
            return Some(held);
        }
        // Each is sent before it's counted, so it's there to receive.
        if self.received >= came {
            return None;
        }
        let incoming = self.incoming.try_recv().ok()?;
        self.received += 1;
        Some(incoming)
    }
}

impl Drop for Api {
    /// Removes the session file, so tools don't find an ended run. A crash or kill leaves it,
    /// naming an address that refuses connections.
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.session);
    }
}

/// `error` as the API answers it.
fn failure(error: &Error) -> Failure {
    Failure::new(error.code(), format!("{error:#}"))
}

#[cfg(test)]
mod tests;
