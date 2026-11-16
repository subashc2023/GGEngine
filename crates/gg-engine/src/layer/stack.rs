//! The layer stack, which drives the layers through a run: their ticks, updates, UI and events.

use std::any::type_name;
use std::mem;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use gg_core::math::Vec2;
use gg_core::{Error, Result};
#[cfg(feature = "dev")]
use subsecond::HotFn;
use tracing::{error, info, warn};

#[cfg(feature = "dev")]
use super::Edited;
use super::{Asked, Commands, Context, Layer, Tick};
#[cfg(feature = "dev")]
use crate::Ui;
use crate::edits::Edit;
#[cfg(feature = "dev")]
use crate::edits::{Authors, Done};
#[cfg(doc)]
use crate::game_ui::GameUi;
#[cfg(feature = "dev")]
use crate::net::Ticket;
use crate::net::{Outside, Session, SessionStatus, Turn};
use crate::recording::Recorder;
use crate::reflect::Value;
use crate::scene_files;
use crate::snapshot::{Run, Snapshot};
use crate::sync_test::SyncTest;
use crate::{Event, World};

/// Layers from the bottom up, with the overlays above the layers.
#[derive(Default)]
pub(crate) struct Stack {
    layers: Vec<Box<dyn Layer>>,
    /// The name of each layer's type, in the same order, which a recording names it by.
    names: Vec<&'static str>,
    /// The full name of each layer's type, in the same order, which tells a layer's own default
    /// `ui` from another layer's that its `ui` calls.
    #[cfg(feature = "dev")]
    types: Vec<&'static str>,
    /// Where the overlays start.
    overlays: usize,
    /// What records the run, if it's recorded.
    recorder: Option<Recorder>,
    /// The session the run takes part in, if it's networked: a lockstep session's peer, a server,
    /// or a server's client.
    net: Option<Session>,
    /// What runs each tick again, if the run's ticks are sync-tested, until one goes another way.
    sync_test: Option<SyncTest>,
    /// Whether a layer ended the run with an error, as [`Context::fail`] says, which the context
    /// tells as the layers detach.
    failed: bool,
}

/// What became of an edit from outside the layers, as [`Stack::offer`] says.
#[cfg(feature = "dev")]
pub(crate) enum Offered {
    /// It applied, and did this.
    Now(Result<Done>),
    /// It applies before a tick to come, and [`Stack::offered`] gives what it did with this ticket.
    Later(Ticket),
}

impl Stack {
    /// Adds a layer on top of the other layers, under the overlays.
    pub(crate) fn push_layer<L: Layer + 'static>(&mut self, layer: L) {
        #[cfg(feature = "dev")]
        let layer = Hot(layer);
        self.layers.insert(self.overlays, Box::new(layer));
        self.names.insert(self.overlays, short_name::<L>());
        #[cfg(feature = "dev")]
        self.types.insert(self.overlays, type_name::<L>());
        self.overlays += 1;
    }

    /// Adds an overlay on top of everything.
    pub(crate) fn push_overlay<L: Layer + 'static>(&mut self, overlay: L) {
        #[cfg(feature = "dev")]
        let overlay = Hot(overlay);
        self.layers.push(Box::new(overlay));
        self.names.push(short_name::<L>());
        #[cfg(feature = "dev")]
        self.types.push(type_name::<L>());
    }

    /// The names of the layers' types, from the bottom up.
    pub(crate) fn names(&self) -> &[&'static str] {
        &self.names
    }

    /// Has `recorder` record the run, from the layers' attach.
    pub(crate) fn record(&mut self, recorder: Recorder) {
        self.recorder = Some(recorder);
    }

    /// Whether a recorder records the run.
    #[cfg(feature = "dev")]
    pub(crate) fn records(&self) -> bool {
        self.recorder.is_some()
    }

    /// The lockstep session the run is a peer of: for tests.
    #[cfg(test)]
    pub(crate) fn net_mut(&mut self) -> Option<&mut crate::net::Lockstep> {
        match &mut self.net {
            Some(Session::Peer(net)) => Some(net.as_mut()),
            _ => None,
        }
    }

    /// The session the run takes part in, if it's networked: for tests.
    #[cfg(test)]
    pub(crate) fn session_mut(&mut self) -> Option<&mut Session> {
        self.net.as_mut()
    }

    /// The queue of the layer at `index`, from the bottom, if it queues commands: for tests.
    #[cfg(test)]
    pub(crate) fn commands_of(&mut self, index: usize) -> Option<&mut dyn Commands> {
        self.layers.get_mut(index)?.commands()
    }

    /// What runs each tick again, if the run's ticks are sync-tested: for tests.
    #[cfg(test)]
    pub(crate) fn sync_test_mut(&mut self) -> Option<&mut SyncTest> {
        self.sync_test.as_mut()
    }

    /// Has the run be a peer of `net`'s lockstep session: for tests.
    #[cfg(test)]
    pub(crate) fn lockstep(&mut self, net: crate::net::Lockstep) {
        self.session(Session::Peer(Box::new(net)));
    }

    /// Has the run take part in `session`: as a lockstep session's peer, a server or a client.
    pub(crate) fn session(&mut self, session: Session) {
        self.net = Some(session);
    }

    /// Takes up what a layer asked of the run's session since the last frame, as
    /// [`Context::host`], [`Context::join`] and [`Context::leave`] say: between ticks, as a frame
    /// starts, before the session's turn. A session left logs what it carried and its delay, as one
    /// does as the run ends.
    pub(crate) fn take_asked(&mut self, cx: &mut Context) {
        let Some(asked) = cx.asked.take() else {
            return;
        };
        match asked {
            Asked::Start(session) => {
                if let Some(net) = self.net.take() {
                    net.ended();
                }
                let player = session.player();
                info!(player, "Started the session the game asked for");
                self.net = Some(*session);
            }
            Asked::Leave => {
                if let Some(net) = self.net.take() {
                    net.ended();
                    info!("Left the session, as the game asked");
                }
            }
        }
        self.tell_session(cx);
    }

    /// The run's session as a game's own UI shows it, as [`Session::status`] says: as a frame's
    /// turn leaves it.
    pub(crate) fn session_status(&self) -> Option<SessionStatus> {
        self.net.as_ref().and_then(Session::status)
    }

    /// Has each tick run again once it has run, a replay's too, as a SyncTest (`--sync-test`) runs
    /// it, as [`tick`](Self::tick) says: before the layers attach.
    pub(crate) fn sync_test(&mut self) {
        self.sync_test = Some(SyncTest::default());
    }

    /// Whether the run waits on its session: a joiner without the host's world, a tick without the
    /// other peer's inputs, or a client without the server's world or too far past its latest
    /// state, `next` being the next to run.
    pub(crate) fn net_waits(&self, next: u64) -> bool {
        self.net.as_ref().is_some_and(|net| !net.ready(next))
    }

    /// Waits out a frame until `until`: a server sending its clients' states as soon as its states
    /// thread has written them, as [`Server::wait`](crate::net::Server::wait) says; any other run
    /// asleep.
    pub(crate) fn net_wait(&mut self, until: Instant) {
        match &mut self.net {
            Some(Session::Server(server)) => server.wait(until),
            _ => std::thread::sleep(until.saturating_duration_since(Instant::now())),
        }
    }

    /// The time a frame `dt` after the last owes the simulation: a networked run's as
    /// [`Session::pace`] says, and any other run's `dt`.
    pub(crate) fn paced(&mut self, dt: Duration) -> Duration {
        self.net.as_mut().map_or(dt, |net| net.pace(dt))
    }

    /// Has a client's frame, `dt` after the last, draw the other players' entities as the server's
    /// states have them, as [`Client::interpolate`](crate::net::Client::interpolate) says, in place
    /// of where its ticks left them: before the world is drawn.
    pub(crate) fn interpolate(&mut self, cx: &mut Context, dt: Duration) {
        if let Some(Session::Client(client)) = &mut self.net {
            cx.past.instead(&cx.world, client.interpolate(dt));
        }
    }

    /// Between ticks, as a frame starts, the next tick to run being `next`: takes what the session
    /// sent since the last turn, as [`Session::turn`] says, sending the joiner or a client the
    /// world, if the host or the server owes it, or restoring the host's or the server's in place
    /// of the run's, the run's textures taking the names the host's or the server's gave them, as
    /// they do the names the host gave those it added since; and, in a rollback session or a
    /// client, takes the ticks that may run again, as [`Session::settle`] says, rolling back to the
    /// first to, as [`roll_back`](Self::roll_back) says, or a client's world taken to the server's
    /// where that's at or past `next`. Returns the tick the run's world is at once it restores
    /// another's or is taken past `next`, which its ticks go on from.
    pub(crate) fn net_turn(&mut self, cx: &mut Context, next: u64) -> Option<u64> {
        let mut net = self.net.take()?;
        let mut restored = None;
        match net.turn(next) {
            Turn::None => {}
            Turn::Send => {
                // A lockstep host's logs go, and the slots they kept vacant for their undos with
                // them, freed as the joiner's load of the world frees them, so that the peers give
                // the same slots to the entities spawned after; and no undo reaches past the world
                // the joiner starts from, which its own logs don't hold. Recorded, as an edit
                // between ticks, so that a replay drops them there too. A server's clients take
                // its world again with each state.
                if matches!(net, Session::Peer(_)) {
                    if let Some(recorder) = &mut self.recorder {
                        recorder.edited(&Edit::Renew.to_value(), &cx.world);
                    }
                    cx.apply_edit(Edit::Renew)
                        .expect("a renewal is never refused");
                }
                let scripts = cx.hosts.applied();
                let mods = cx.mods.applied();
                let (app, names) = (net.app(), &self.names);
                // The world whole for a joiner, and what each client sees for it.
                let mut write = |seen: Option<&crate::cells::Seen>| {
                    let of = crate::snapshot::Of {
                        world: &cx.world,
                        scenes: &cx.scenes.files,
                        cached: &cx.scenes.cached,
                        open: &cx.scenes.open,
                        scripts: &scripts,
                        mods: &mods,
                        seen,
                    };
                    let text = crate::snapshot::write(&of, &Run::Playing, app, names, next);
                    let unsent = |error: Error| {
                        error!(
                            code = error.code(),
                            "Can't send the joiner or a client the world: {error:#}"
                        );
                    };
                    text.map_err(unsent).ok()
                };
                net.send_world(&cx.world, &mut write, next);
            }
            Turn::Restore((tick, text)) => {
                let from = format!("the host's or the server's world at tick {tick}");
                let snapshot = Snapshot::parse(text, from, net.app());
                // A client's player is the server's choice, which its layers pick up as it
                // restores.
                cx.player = net.player();
                let ticks = snapshot.and_then(|snapshot| {
                    // Its textures go by the host's names, whatever ID files this run's disk holds.
                    #[cfg(feature = "dev")]
                    cx.name_as(
                        (snapshot.names.iter()).map(|(path, name)| (name.clone(), path.clone())),
                    );
                    cx.load_restored(snapshot.added());
                    // Its host's scripts, which its world's components are as, in place of its
                    // own files', and its host's mods in place of its own.
                    cx.hosts.take(&mut cx.world, &snapshot.scripts);
                    cx.mods.take(&mut cx.world, &snapshot.mods);
                    self.restore_in_place(cx, &snapshot)
                });
                match ticks {
                    Ok(ticks) => restored = Some(net.restored(ticks, &cx.world)),
                    Err(error) => {
                        error!(code = error.code(), "{error:#}");
                        net.lose("its world can't be restored");
                    }
                }
            }
        }
        #[cfg(feature = "dev")]
        if let Session::Peer(peer) = &mut net {
            let names = peer.take_names();
            if !names.is_empty() {
                cx.name_also(names);
            }
        }
        // The ticks that run again: a rollback peer's run on the other's inputs predicted whose
        // inputs have come, or a client's past the server's latest state.
        let next = restored.unwrap_or(next);
        let from = net.settle(next, &cx.world);
        self.net = Some(net);
        match from {
            Some(from) if from < next => self.roll_back(cx, from, next),
            // A client's world, taken to the server's state after the tick before `next`, or past.
            Some(from) => {
                let net = self.net.as_mut().expect("it's back");
                cx.world.rewind(net.start_of(from));
                cx.share_assets();
                net.rolled_back(from, from);
                if from > next {
                    restored = Some(from);
                }
            }
            None => {}
        }
        // A rollback peer's ticks run on the other's inputs predicted that held nothing are final
        // once theirs have come, though they don't run again: the world as the first not yet so
        // started is.
        if let Some(Session::Peer(peer)) = &self.net {
            cx.confirm_scene_from(peer.final_world());
        }
        restored
    }

    /// Tells `cx` the session the run takes part in, if any, as [`Context::player`] and
    /// [`Context::networked`] say, and whether it's a server's or a client's: as the layers attach
    /// or pick up from a restore.
    fn tell_session(&self, cx: &mut Context) {
        cx.player = self.net.as_ref().and_then(Session::player);
        cx.networked = self.net.is_some();
        cx.served = self.served();
        cx.client = matches!(self.net, Some(Session::Client(_)));
        cx.follows |= self.joins();
    }

    /// Whether the run hosts or plays alone, rather than joining a lockstep session or connecting
    /// to a server, whose host or server alone watches the scene and script files the layers
    /// loaded.
    #[cfg(feature = "dev")]
    pub(crate) fn watches_files(&self) -> bool {
        !self.joins()
    }

    /// Whether the run joins a lockstep session or connects to a server, going as the host's or the
    /// server's world goes.
    fn joins(&self) -> bool {
        self.net.as_ref().is_some_and(Session::follows)
    }

    /// Whether the run serves a server's session (`--serve`) or connects to one (`--connect`).
    fn served(&self) -> bool {
        matches!(self.net, Some(Session::Server(_) | Session::Client(_)))
    }

    /// Whether the run is networked, hosting, joining, serving or connecting, whose scene files the
    /// session's others' must stay as.
    #[cfg(feature = "dev")]
    pub(crate) fn is_peer(&self) -> bool {
        self.net.is_some()
    }

    /// Applies `edit`, an author's from outside the layers, between ticks, as [`edit`](Self::edit)
    /// does; or, in a lockstep session, offers it for the tick both peers apply it before, giving
    /// its ticket. A server's client refuses it (`net.no_edit`), as the server's next state would
    /// take the edit away.
    #[cfg(feature = "dev")]
    pub(crate) fn offer(&mut self, cx: &mut Context, edit: Edit) -> Offered {
        match &mut self.net {
            Some(Session::Peer(net)) => Offered::Later(net.offer(edit)),
            Some(Session::Client(_)) => Offered::Now(Err(Error::new(
                "net.no_edit",
                "a client's world goes as its server's, whose next state would take an edit of \
                 it away: edit the server's world",
            ))),
            Some(Session::Server(_)) | None => {
                let apply = |cx: &mut Context, edit| self.edit(cx, edit);
                Offered::Now(scene_files::client_edit(cx, edit, apply))
            }
        }
    }

    /// Applies the save of the scene file whose author is `author`, its text now `text`, between
    /// ticks, as [`scene_files::apply_save`] does; or, in a lockstep session, offers it for the
    /// tick both peers apply it before. Either way it logs what it did.
    #[cfg(feature = "dev")]
    pub(crate) fn offer_save(&mut self, cx: &mut Context, author: String, text: String) {
        match &mut self.net {
            Some(Session::Peer(net)) => {
                // What it names that isn't loaded is loaded now, so that the names the host gives
                // those go out before it. One naming a texture that can't be, which the joiner
                // might stand in for, isn't offered, warning as a save that doesn't load does.
                if let Ok(missing) = cx.load_scene_assets(&text)
                    && !missing.is_empty()
                    && let Err(error) = cx.world().scene_loads(&text)
                {
                    warn!(
                        code = error.code(),
                        path = author,
                        "{author}: {error:#}; the world stays as it was, and the file's next save \
                         is compared with it as last applied"
                    );
                    return;
                }
                net.offer_save(author, text);
            }
            _ => {
                let edit = &mut |cx: &mut Context, edit| self.edit(cx, edit);
                let _ = crate::scene_files::apply_save(cx, edit, &author, text);
            }
        }
    }

    /// Reloads the script whose path from the project's root is `path`, its text now `text`, as
    /// its file is saved, through an edit, between ticks, as [`edit`](Self::edit) applies it; or,
    /// in a lockstep session, offers that edit for the tick both peers apply it before. Either
    /// way, what it did is logged, as [`Hosts::reload`](crate::luau::Hosts::reload) says.
    #[cfg(feature = "dev")]
    pub(crate) fn offer_script(&mut self, cx: &mut Context, path: String, text: String) {
        let edit = Edit::Script { path, text };
        match &mut self.net {
            Some(Session::Peer(net)) => net.offer_unanswered(edit),
            _ => {
                let _ = self.edit(cx, edit);
            }
        }
    }

    /// Offers the patch `table` names, as
    /// [`Lockstep::offer_patch`](crate::net::Lockstep::offer_patch) does; `None` outside a
    /// lockstep session, where it applies now.
    #[cfg(feature = "dev")]
    pub(crate) fn offer_patch(
        &mut self,
        table: subsecond::JumpTable,
    ) -> std::result::Result<Ticket, subsecond::JumpTable> {
        match &mut self.net {
            Some(Session::Peer(net)) => Ok(net.offer_patch(table)),
            _ => Err(table),
        }
    }

    /// Applies the patch `table` names, the simulation having run `ticks` ticks, as
    /// [`patch::apply`](crate::patch::apply) does, outside a lockstep session, which
    /// [`offer_patch`](Self::offer_patch) offers it to; a recording gets it once it has applied,
    /// as [`Recorder::patched`] says.
    #[cfg(feature = "dev")]
    pub(crate) fn patch(&mut self, table: subsecond::JumpTable, ticks: u64) -> Result<()> {
        patched(&mut self.recorder, table, ticks)
    }

    /// What the edit or the patch offered with `ticket` did, and the tick it applied before, once
    /// it has, which it gives once.
    #[cfg(feature = "dev")]
    pub(crate) fn offered(&mut self, ticket: Ticket) -> Option<(u64, Result<Done>)> {
        match &mut self.net {
            Some(Session::Peer(net)) => net.done(ticket),
            _ => None,
        }
    }

    /// Whether the edit or the patch offered with `ticket` has applied, or been refused.
    #[cfg(feature = "dev")]
    pub(crate) fn offered_is_done(&self, ticket: Ticket) -> bool {
        matches!(&self.net, Some(Session::Peer(net)) if net.is_done(ticket))
    }

    /// Fails, once the run's recording has stopped, as [`Recorder::check`] says.
    pub(crate) fn recorded(&self) -> Result<()> {
        self.recorder.as_ref().map_or(Ok(()), Recorder::check)
    }

    /// Loads the layers, then attaches them, each from the bottom up, in `cx`, whose world nothing
    /// has drawn an ID from: its seed is what a recording starts from.
    pub(crate) fn attach(&mut self, cx: &mut Context) {
        let _zone = gg_core::zone!();
        if let Some(recorder) = &mut self.recorder {
            recorder.start(&self.names, cx.world.stream_state());
        }
        self.tell_session(cx);
        #[cfg(feature = "dev")]
        {
            cx.sync_tested = self.sync_test.is_some();
        }
        self.load(cx);
        #[cfg(feature = "dev")]
        cx.read_scenes();
        for layer in &mut self.layers {
            layer.attach(cx);
            cx.share_assets();
        }
        cx.attached = true;
        if let Some(recorder) = &mut self.recorder {
            recorder.attached(&cx.world, &cx.mods.applied());
        }
    }

    /// Loads the layers, then puts `snapshot`'s world in `cx`'s place, as a run restored from it
    /// starts, and has each layer pick up from it, each from the bottom up; returns the ticks the
    /// simulation had run as it was taken, which the run's go on from, and whether its run was
    /// stopped, as the run then is. A development build's run that a tool API `serves`, and that
    /// can stop, as [`refuses_stop`](Self::refuses_stop) says, goes on as the snapshot's run was:
    /// one taken while a copy played holds the world as edited too, which comes back held aside, as
    /// [`play`](Self::play) holds it, the authors' edits afresh, as after any restore. Any other
    /// run plays on with the world that played, warning `snapshot.plays`, with why, of a snapshot
    /// whose run was stopped or held a world as edited.
    ///
    /// Fails as [`Snapshot::restore`] does.
    pub(crate) fn restore(
        &mut self,
        cx: &mut Context,
        snapshot: &Snapshot,
        serves: bool,
    ) -> Result<(u64, bool)> {
        let _zone = gg_core::zone!();
        self.tell_session(cx);
        #[cfg(feature = "dev")]
        {
            cx.sync_tested = self.sync_test.is_some();
        }
        self.load(cx);
        cx.attached = true;
        cx.load_restored(snapshot.added());
        #[cfg(feature = "dev")]
        if serves && self.refuses_stop().is_none() {
            // As the layers loaded it, with their types, assets and resources, for the world as
            // edited to load into.
            let loaded = snapshot.held.as_ref().map(|held| (held, cx.world.clone()));
            let ticks = self.restore_in_place(cx, snapshot)?;
            if let Some((held, mut world)) = loaded {
                snapshot.restore_held(&mut world)?;
                world.share_assets(&cx.assets);
                let start = cx.start.as_deref();
                let open =
                    (held.open.clone()).unwrap_or_else(|| scene_files::opened(start, &held.scenes));
                cx.edited = Some(Edited {
                    world,
                    ticks: held.ticks,
                    scenes: held.scenes.clone(),
                    cached: cx.scenes.cached.clone(),
                    open,
                    authors: Authors::default(),
                });
            }
            return Ok((ticks, snapshot.stopped));
        }
        let _ = serves;
        let ticks = self.restore_in_place(cx, snapshot)?;
        if snapshot.stopped || snapshot.held.is_some() {
            #[cfg(feature = "dev")]
            let why = self
                .refuses_stop()
                .unwrap_or("the run serves no tool API, whose `play` alone plays a stopped run");
            #[cfg(not(feature = "dev"))]
            let why = "the build serves no tool API, whose `play` alone plays a stopped run";
            warn!(
                code = "snapshot.plays",
                path = %snapshot.from,
                why,
                "The snapshot's run was stopped, or held a world as edited, but this run plays \
                 from start to end: it goes on with the world that played"
            );
        }
        Ok((ticks, false))
    }

    /// Whether the tool API's `stop` can't stop the run, and if so, why: a recorded run's replay
    /// couldn't follow a world brought back between ticks, its ticks taken back, and a lockstep
    /// peer's world would part from the other's. Such a run plays from start to end.
    #[cfg(feature = "dev")]
    pub(crate) fn refuses_stop(&self) -> Option<&'static str> {
        if self.recorder.is_some() {
            Some(
                "the run is recorded, and its replay couldn't follow a world brought back between \
                 ticks, its ticks taken back",
            )
        } else if self.net.is_some() {
            Some("the run is networked, and its world would part from the session's others")
        } else {
            None
        }
    }

    /// Has a copy of `cx`'s world, the world as edited, which is at `ticks`, play in its place, as
    /// the tool API's `play` asks of a stopped run: holds the world as edited aside, with its
    /// ticks, its scene files, its open scene and the authors' edits of it, whose logs the copy
    /// starts afresh, until [`stop`](Self::stop); the copy's open scene starts as a copy of it.
    /// The layers are told nothing: what they hold of the world holds of the copy, which goes on
    /// exactly as the world would.
    #[cfg(feature = "dev")]
    pub(crate) fn play(&self, cx: &mut Context, ticks: u64) {
        let _zone = gg_core::zone!();
        let copy = cx.world.clone();
        let world = mem::replace(&mut cx.world, copy);
        let entities = world.entities(None).map_or(0, |entities| entities.len());
        cx.edited = Some(Edited {
            world,
            ticks,
            scenes: cx.scenes.files.clone(),
            cached: cx.scenes.cached.clone(),
            open: cx.scenes.open.clone(),
            authors: mem::take(&mut cx.authors),
        });
        info!(ticks, entities, "Playing a copy of the world as edited");
    }

    /// Brings back the world as edited that [`play`](Self::play) holds aside, if it holds one, as
    /// the tool API's `stop` asks: the very world, with its scene files, its open scene and the
    /// authors' edits of it, the copy and all done to it going, but for the scripts reloaded since,
    /// whose components it holds as they declare them now, as [`Scripts`](crate::Scripts) says;
    /// empties each layer's queue, which
    /// the copy's ticks were to apply; has each layer pick up from the world, from the bottom up,
    /// as [`Layer::restored`] says; and returns the ticks the world as edited is at. A run playing
    /// from its start holds none: `None`, and nothing changes.
    #[cfg(feature = "dev")]
    pub(crate) fn stop(&mut self, cx: &mut Context) -> Option<u64> {
        let edited = cx.edited.take()?;
        let _zone = gg_core::zone!();
        cx.world = edited.world;
        cx.share_assets();
        cx.authors = edited.authors;
        cx.scenes.files = edited.scenes;
        cx.scenes.cached = edited.cached;
        cx.scenes.open = edited.open;
        // Of the world as edited, as its scene files and open scene are.
        cx.seen_scene();
        // Its scripts' components as their scripts declare them now, though they reloaded since.
        cx.hosts.migrate(&mut cx.world);
        for layer in &mut self.layers {
            if let Some(queue) = layer.commands() {
                queue.clear();
            }
        }
        for layer in &mut self.layers {
            layer.restored(cx);
        }
        info!(ticks = edited.ticks, "Stopped: the world as edited is back");
        Some(edited.ticks)
    }

    /// Has `apply` change `cx`'s world, between ticks, through a log of its own, which no author
    /// undoes, as the tool API's `scene open` and `scene new` change it; and then starts every
    /// author's edits afresh, since none of them could undo past it: the world is renewed, as
    /// [`World::renew`] says, refusing every log's entries from before, the authors' logs go, and
    /// each layer picks up from the world, from the bottom up, as at [`stop`](Self::stop), so that
    /// the in-process Inspector lets go of its own. Refused, what `apply` applied comes undone,
    /// and nothing else changes.
    ///
    /// Fails as `apply` does.
    #[cfg(feature = "dev")]
    pub(crate) fn rewrite(
        &mut self,
        cx: &mut Context,
        apply: impl FnOnce(&mut World, &mut crate::Log) -> Result<()>,
    ) -> Result<()> {
        let mut log = crate::Log::default();
        if let Err(error) = apply(&mut cx.world, &mut log) {
            // What applied before the refusal, which a refusal of the log's own undoes already.
            let _ = log.undo(&mut cx.world);
            log.clear(&mut cx.world);
            return Err(error);
        }
        // Dropped, as the renewed world lets go of what it kept.
        drop(log);
        cx.world.renew();
        cx.authors = Authors::default();
        cx.scenes.changed();
        cx.hosts.migrate(&mut cx.world);
        for layer in &mut self.layers {
            layer.restored(cx);
        }
        Ok(())
    }

    /// Puts `snapshot`'s world in `cx`'s place, the layers having loaded, with its scene files and
    /// its open scene, or the start scene's file open for a snapshot that holds none, its scripts'
    /// components as the layers' scripts declare them, and has each layer pick up from it, from the
    /// bottom up; returns the ticks the simulation had run as it was taken.
    ///
    /// Fails as [`Snapshot::restore`] does.
    fn restore_in_place(&mut self, cx: &mut Context, snapshot: &Snapshot) -> Result<u64> {
        let ticks = snapshot.restore(&mut cx.world, &self.names)?;
        cx.scenes.files.clone_from(&snapshot.scenes);
        cx.scenes.cached.clone_from(&snapshot.cached);
        cx.seen_scene();
        cx.scenes.open = (snapshot.open.clone())
            .unwrap_or_else(|| scene_files::opened(cx.start.as_deref(), &snapshot.scenes));
        #[cfg(feature = "dev")]
        cx.scenes.changed();
        cx.hosts.migrate(&mut cx.world);
        for layer in &mut self.layers {
            layer.restored(cx);
        }
        Ok(ticks)
    }

    /// Loads the layers, from the bottom up.
    ///
    /// # Panics
    ///
    /// If a layer spawned an entity, or changed the world's rows otherwise, as it loaded: a
    /// restored run, whose world comes from a snapshot, would spawn it again.
    fn load(&mut self, cx: &mut Context) {
        let before = cx.world.version();
        for (layer, name) in self.layers.iter_mut().zip(&self.names) {
            layer.load(cx);
            cx.share_assets();
            assert_eq!(
                cx.world.version(),
                before,
                "layer {name} changed the world's entities as it loaded, which its attach does, so \
                 that a run restored from a snapshot doesn't spawn them again"
            );
        }
    }

    /// Detaches from the top down, the reverse of attaching, once a recording has its end, a
    /// SyncTest that found no tick going another way has logged how many it ran again
    /// (`sync_test.matched`), and a rollback session's peer how it rolled back (`net.rollback`);
    /// and takes from `cx` whether a layer ended the run with an error, as
    /// [`failed`](Self::failed) tells, as it detached too.
    pub(crate) fn detach(&mut self, cx: &mut Context) {
        let _zone = gg_core::zone!();
        if let Some(recorder) = &mut self.recorder {
            recorder.end();
        }
        if let Some(sync_test) = &self.sync_test {
            info!(
                code = "sync_test.matched",
                ticks = sync_test.matched(),
                "Each tick ran again on a copy of the world taken as it started, and left it as it \
                 left the world"
            );
        }
        if let Some(net) = &self.net {
            net.ended();
        }
        cx.attached = false;
        cx.authors.clear(&mut cx.world);
        for layer in self.layers.iter_mut().rev() {
            layer.detach(cx);
        }
        self.failed |= cx.failed;
    }

    /// Whether a layer ended the run with an error, as [`Context::fail`] says, once the layers have
    /// detached, which makes the run's status 1.
    pub(crate) fn failed(&self) -> bool {
        self.failed
    }

    #[cfg(feature = "dev")]
    /// Applies `edit`, an author's from outside the layers, to `cx`'s world, between ticks, and says
    /// what it did. A recording gets it first, as it was asked for, before the commands of the tick
    /// after, so that a replay applies it there, and is refused where the run refused it.
    ///
    /// Fails as [`Authors::edit`] does.
    pub(crate) fn edit(&mut self, cx: &mut Context, edit: Edit) -> Result<Done> {
        if let Some(recorder) = &mut self.recorder {
            recorder.edited(&edit.to_value(), &cx.world);
        }
        cx.apply_edit(edit)
    }

    /// Runs a tick of the simulation, as [`run`](Self::run) says. A tick waits for the cells it
    /// brings in as it starts to be prepared, returning `false` without running until they are, as
    /// [`Context::cells_ready`] says. In a lockstep session, the tick also waits for the other
    /// peer's inputs, or, in a rollback session, until it's near enough them, and a client's for the
    /// server's world, and while it's too far past the latest state, as [`Session::ready`] says;
    /// once it can run, the run's queued commands go out as its inputs, as [`Session::start_tick`]
    /// says.
    pub(crate) fn tick(&mut self, cx: &mut Context, tick: Tick) -> bool {
        let _zone = gg_core::zone!();
        if !cx.cells_ready() {
            return false;
        }
        let Self {
            layers,
            names,
            recorder,
            net,
            sync_test,
            ..
        } = self;
        if let Some(net) = net.as_mut() {
            if !net.ready(tick.number) {
                net.waited(tick.number);
                return false;
            }
            let queues = layers.iter_mut().enumerate();
            let queues = queues.filter_map(|(index, layer)| Some((index, layer.commands()?)));
            net.start_tick(tick.number, queues, &cx.world);
        }
        Self::run(layers, names, recorder, net.as_mut(), sync_test, cx, tick);
        true
    }

    /// Runs tick `tick`: each layer's, from the bottom up, after which the world shares the
    /// context's assets again, as [`Context::share_assets`] says; then the world's cells come in and
    /// go out, as [`Context::stream_cells`] says, as they do wherever a tick runs; and then the
    /// sounds the tick queued play, as [`World::play_sound`] says. A recording gets each layer's
    /// commands just before its tick applies them, so that a tick that panics leaves them, and the
    /// world's hash after the last. In a networked session, the tick runs on the session's inputs
    /// for it, as [`Session::inputs`] says: in a lockstep session, both peers', the edits among
    /// them applying first, as a recording keeps them; or, in a rollback session, before the
    /// other's have come, on them predicted, a copy of the world kept as it starts, which a
    /// rollback runs it again from; a server's own and its clients'; and a client's own. Where the
    /// tick may run again, as a rollback peer's or a client's, the sounds it plays are kept, which
    /// a run again doesn't play again, as [`Context::play_queued_again`] says; and the session is
    /// told once it has run, as [`Session::ran`] says. A SyncTest takes a copy of the world once
    /// the edits have applied, keeps each layer's commands as its tick is about to apply them, and
    /// runs the tick again once it has run, as [`run_again`](Self::run_again) says: one that fails
    /// ends the run with its error, as [`Context::fail`] does, and the SyncTest with it.
    fn run(
        layers: &mut [Box<dyn Layer>],
        names: &[&'static str],
        recorder: &mut Option<Recorder>,
        mut net: Option<&mut Session>,
        sync_test: &mut Option<SyncTest>,
        cx: &mut Context,
        tick: Tick,
    ) {
        // A client's ticks may all run again, as may a rollback peer's on the other's inputs
        // predicted, whose world before them is then final no more than after.
        let final_tick = match net.as_deref() {
            None | Some(Session::Server(_)) => true,
            Some(Session::Peer(peer)) => peer.settled(tick.number),
            Some(Session::Client(_)) => false,
        };
        cx.drop_untimely_scene();
        if final_tick {
            cx.confirm_scene();
        }
        let merged = match net.as_deref_mut() {
            Some(net) => {
                let merged = net.inputs(tick.number, cx);
                for (ticket, input) in &merged.inputs {
                    let done = match input {
                        // A client's, on either peer.
                        Outside::Edit(edit) => {
                            let apply = |cx: &mut Context, edit: Edit| {
                                if let Some(recorder) = recorder.as_mut() {
                                    recorder.edited(&edit.to_value(), &cx.world);
                                }
                                cx.apply_edit(edit)
                            };
                            scene_files::client_edit(cx, edit.clone(), apply)
                        }
                        #[cfg(feature = "dev")]
                        Outside::Scene { author, text } => {
                            let mut edit = |cx: &mut Context, edit: Edit| {
                                if let Some(recorder) = recorder.as_mut() {
                                    recorder.edited(&edit.to_value(), &cx.world);
                                }
                                cx.apply_edit(edit)
                            };
                            crate::scene_files::apply_save(cx, &mut edit, author, text.clone())
                        }
                        // Only a development build's host watches scene files.
                        #[cfg(not(feature = "dev"))]
                        Outside::Scene { author, text } => Err(Error::new(
                            "net.bad_command",
                            format!(
                                "a peer sent {author}'s save, {} bytes, which only development \
                                 builds apply",
                                text.len()
                            ),
                        )),
                    };
                    if let Some(ticket) = ticket {
                        net.did(*ticket, tick.number, done);
                    }
                }
                #[cfg(feature = "dev")]
                for (ticket, table) in merged.patches.iter().cloned() {
                    let patched = patched(recorder, table, tick.number);
                    net.did(
                        ticket,
                        tick.number,
                        patched.map(|()| Done::Applied(Vec::new())),
                    );
                }
                Some(merged)
            }
            None => None,
        };
        if let Some(sync_test) = sync_test.as_mut() {
            sync_test.take(&cx.world);
        }
        cx.enter_coming();
        if let Some(recorder) = recorder.as_mut() {
            for &(cell, digest) in &cx.cells.came {
                recorder.cell_came(cell, digest);
            }
        }
        for (index, layer) in layers.iter_mut().enumerate() {
            if let Some(merged) = &merged
                && let Some(queue) = layer.commands()
            {
                merged.fill(index, names[index], queue, cx);
            }
            if let Some(recorder) = recorder.as_mut()
                && let Some(commands) = layer.commands()
            {
                recorder.queued(index, names[index], commands, &cx.world);
            }
            if let Some(sync_test) = sync_test.as_mut()
                && let Some(commands) = layer.commands()
            {
                sync_test.queued(index, names[index], commands, &cx.world);
            }
            layer.tick(&mut cx.world, tick);
            cx.change_asked_scene();
            cx.share_assets();
        }
        cx.stream_cells();
        match net.as_deref_mut().and_then(|net| net.played(tick.number)) {
            Some(played) => cx.play_queued_again(played),
            None => cx.play_queued(),
        }
        if let Some(again) = sync_test.as_mut()
            && let Err(error) = Self::run_again(layers, again, cx, tick)
        {
            error!(code = error.code(), tick = tick.number, "{error:#}");
            // As `Context::fail` ends it, with the error logged.
            cx.exiting = true;
            cx.failed = true;
            *sync_test = None;
        }
        if final_tick {
            cx.confirm_scene();
        }
        // A rollback session's isn't recorded, as `--rollback` takes no `--record`, nor a
        // client's, as `--connect` takes none.
        if let Some(recorder) = recorder {
            recorder.ticked(tick, &cx.world);
        }
        if let Some(net) = net {
            net.ran(tick.number, &mut cx.world);
        }
    }

    /// Takes the world back to where tick `from` started, as [`Session::start_of`] has it, the
    /// start of a tick run on the other lockstep peer's inputs predicted, which held something, or
    /// a client's server's world after the tick before, and runs it and the ticks after again, to
    /// `next`, the next to run, as rollback does, the world going on as they leave it, as
    /// [`run`](Self::run) runs each: the very world, as [`World::rewind`] says, so that what's kept
    /// of it holds, and the layers are told nothing. Each layer's queue, which holds what it queued
    /// since the last tick, is as it was once they've run; and the frames after draw on from where
    /// the last started.
    fn roll_back(&mut self, cx: &mut Context, from: u64, next: u64) {
        let _zone = gg_core::zone!();
        let Self {
            layers,
            names,
            recorder,
            net,
            sync_test,
            ..
        } = self;
        let Some(net) = net.as_mut() else { return };
        cx.world.rewind(net.start_of(from));
        let queued: Vec<(usize, Vec<Value>)> = (layers.iter_mut().enumerate())
            .filter_map(|(index, layer)| {
                let queue = layer.commands()?;
                Some((index, (0..queue.len()).map(|i| queue.value(i)).collect()))
            })
            .collect();
        for number in from..next {
            if number + 1 == next {
                cx.capture();
            }
            let tick = Tick { number };
            Self::run(
                layers,
                names,
                recorder,
                Some(&mut *net),
                sync_test,
                cx,
                tick,
            );
        }
        for (index, values) in queued {
            let queue = layers[index].commands().expect("it queued them");
            queue.clear();
            for value in &values {
                let pushed = queue.push(value);
                pushed.expect("a queue queues again what it held");
            }
        }
        net.rolled_back(from, next);
    }

    /// Runs a tick as a replay does: as [`tick`](Self::tick) does, but never waiting, the cells it
    /// brings in prepared then and there unless the tick before prepared them, with each layer's
    /// queue emptied and filled again by `fill`, which gets the layer's index, its queue, if it has
    /// one, and the world as its tick finds it; and a SyncTest runs it again.
    ///
    /// Fails as `fill` does, and as [`run_again`](Self::run_again) does, which ends the SyncTest.
    pub(crate) fn replay_tick(
        &mut self,
        cx: &mut Context,
        tick: Tick,
        mut fill: impl FnMut(usize, Option<&mut dyn Commands>, &World) -> Result<()>,
    ) -> Result<()> {
        let _zone = gg_core::zone!();
        let Self {
            layers,
            names,
            sync_test,
            ..
        } = self;
        cx.drop_untimely_scene();
        cx.confirm_scene();
        // What the tick before prepared ahead, on the replay's own thread.
        cx.prepare_cells();
        if let Some(sync_test) = sync_test.as_mut() {
            sync_test.take(&cx.world);
        }
        cx.enter_coming();
        for (index, layer) in layers.iter_mut().enumerate() {
            fill(index, layer.commands(), &cx.world)?;
            if let Some(sync_test) = sync_test.as_mut()
                && let Some(commands) = layer.commands()
            {
                sync_test.queued(index, names[index], commands, &cx.world);
            }
            layer.tick(&mut cx.world, tick);
            cx.change_asked_scene();
            cx.share_assets();
        }
        cx.stream_cells();
        cx.play_queued();
        if let Some(again) = sync_test.as_mut()
            && let Err(error) = Self::run_again(layers, again, cx, tick)
        {
            *sync_test = None;
            return Err(error);
        }
        cx.confirm_scene();
        Ok(())
    }

    /// Runs `tick` again, as a SyncTest does once it has run, `sync_test` having kept a copy of the
    /// world as it started and each layer's commands as its tick applied them: on the copy, put in
    /// the world's place, each layer told as it's put there and as the world comes back, as
    /// [`Layer::swapped`] says, and given back the commands its tick applied; the sounds the tick
    /// queues there stay in the copy, which nothing plays, as they played as it ran on the world.
    /// Then checks that it left the copy as it left the world.
    ///
    /// Fails, without running it again, as [`SyncTest::refused`] does, and as
    /// [`SyncTest::compare`] does.
    fn run_again(
        layers: &mut [Box<dyn Layer>],
        sync_test: &mut SyncTest,
        cx: &mut Context,
        tick: Tick,
    ) -> Result<()> {
        let _zone = gg_core::zone!();
        sync_test.refused()?;
        mem::swap(&mut cx.world, sync_test.world_mut());
        for layer in layers.iter_mut() {
            layer.swapped(cx);
        }
        cx.enter_coming();
        for (index, layer) in layers.iter_mut().enumerate() {
            if let Some(queue) = layer.commands() {
                sync_test.give_back(index, queue);
            }
            layer.tick(&mut cx.world, tick);
            cx.change_asked_scene();
            cx.share_assets();
        }
        cx.stream_cells();
        mem::swap(&mut cx.world, sync_test.world_mut());
        for layer in layers.iter_mut() {
            layer.swapped(cx);
        }
        sync_test.compare(tick, &cx.world)
    }

    pub(crate) fn update(&mut self, cx: &mut Context, dt: Duration) {
        let _zone = gg_core::zone!();
        for layer in &mut self.layers {
            layer.update(cx, dt);
        }
    }

    /// Draws UI from the bottom up, so the layers above draw over those below, and returns
    /// whether any layer draws UI: every layer does but one whose `ui` is its own type's default,
    /// [`Layer::ui`]'s. One whose `ui` calls another layer's default draws UI as far as this goes,
    /// whatever it draws.
    #[cfg(feature = "dev")]
    pub(crate) fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) -> bool {
        let _zone = gg_core::zone!();
        let mut draws = false;
        for (layer, &own) in self.layers.iter_mut().zip(&self.types) {
            layer.ui(cx, ui);
            draws |= ui.take_default_ui() != Some(own);
        }
        draws
    }

    /// Records `event` in the input state that layers poll, then hands it down from the top
    /// until a layer handles it. `Break` means one did. A release, a gamepad's stick or trigger
    /// back at rest, and the loss of focus, go on to the layers below one that handles them, as
    /// releases go past the UI, so that none is left holding what it saw pressed: a release without
    /// its press changes nothing. A game UI that takes the mouse's buttons or its wheel from the
    /// layers below, as [`GameUi::event`] says, handles them for the layer that sent it them.
    pub(crate) fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        cx.input.record(event);
        let lets_go = match *event {
            Event::Key { pressed, .. }
            | Event::MouseButton { pressed, .. }
            | Event::PadButton { pressed, .. } => !pressed,
            Event::PadStick { value, .. } => value == Vec2::ZERO,
            Event::PadTrigger { value, .. } => value == 0.0,
            Event::Focused(focused) => !focused,
            _ => false,
        };
        let mut handled = ControlFlow::Continue(());
        for layer in self.layers.iter_mut().rev() {
            cx.uis.forget_consumed();
            // A UI a layer sent the event to took it, if the layer let it go on.
            if layer.event(cx, event).is_break() || cx.uis.consumed() {
                handled = ControlFlow::Break(());
                if !lets_go {
                    break;
                }
            }
        }
        handled
    }
}

/// A layer as a development build's stack holds it, which calls each of the layer's methods
/// through subsecond's `HotFn`, as [`Layer`] says, since the stack calls what it holds through a
/// vtable, which no patch changes, while `HotFn`, in a build with debug assertions, looks each call
/// up in the jump table of the last patch applied. Each call goes through the method as a function
/// item, such as `L::tick`: `HotFn` finds a function of no size in the table by its own `call_it`
/// for it, which the game's crate makes, since `L` is the game's, and so the patch makes again; it
/// takes anything the size of a pointer, a closure that holds one among them, for a function
/// pointer, and calls it as one. A method added to `Layer` needs its call here too, or the trait's
/// default runs in place of the layer's own. The stack names the layer by `L`, and `L`'s default
/// `ui` names `L`, as the stack's types do. A call allocates nothing.
#[cfg(feature = "dev")]
pub(super) struct Hot<L>(pub(super) L);

#[cfg(feature = "dev")]
impl<L: Layer> Layer for Hot<L> {
    fn load(&mut self, cx: &mut Context) {
        HotFn::current(L::load).call((&mut self.0, cx));
    }

    fn attach(&mut self, cx: &mut Context) {
        HotFn::current(L::attach).call((&mut self.0, cx));
    }

    fn restored(&mut self, cx: &mut Context) {
        HotFn::current(L::restored).call((&mut self.0, cx));
    }

    fn swapped(&mut self, cx: &mut Context) {
        HotFn::current(L::swapped).call((&mut self.0, cx));
    }

    fn detach(&mut self, cx: &mut Context) {
        HotFn::current(L::detach).call((&mut self.0, cx));
    }

    fn tick(&mut self, world: &mut World, tick: Tick) {
        HotFn::current(L::tick).call((&mut self.0, world, tick));
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        HotFn::current(L::commands).call((&mut self.0,))
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        HotFn::current(L::update).call((&mut self.0, cx, dt));
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        HotFn::current(L::event).call((&mut self.0, cx, event))
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        HotFn::current(L::ui).call((&mut self.0, cx, ui));
    }
}

/// Applies the patch `table` names, the simulation having run `ticks` ticks, as
/// [`patch::apply`](crate::patch::apply) does, and has `recorder`, if the run is recorded, write it
/// once it has applied, so that the recording's replay, which can't apply it, stops where the run's
/// code changed rather than going another way from there unsaid.
#[cfg(feature = "dev")]
fn patched(recorder: &mut Option<Recorder>, table: subsecond::JumpTable, ticks: u64) -> Result<()> {
    let library = table.lib.clone();
    crate::patch::apply(table, ticks)?;
    if let Some(recorder) = recorder {
        recorder.patched(&library);
    }
    Ok(())
}

/// The name of type `L`, without its path or its generics: such as `Inspector`.
pub(super) fn short_name<L>() -> &'static str {
    let name = type_name::<L>();
    let name = name.split('<').next().unwrap_or(name);
    name.rsplit("::").next().unwrap_or(name)
}
