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
    /// Each layer's type's name, in the same order: a recording names layers by it.
    names: Vec<&'static str>,
    /// Each layer's type's full name: tells a layer's own default `ui` from another's that it
    /// calls.
    #[cfg(feature = "dev")]
    types: Vec<&'static str>,
    /// Where the overlays start.
    overlays: usize,
    recorder: Option<Recorder>,
    /// A lockstep session's peer, a server, or a server's client.
    net: Option<Session>,
    /// Runs each tick again, if sync-tested, until one goes another way.
    sync_test: Option<SyncTest>,
    /// Whether a layer called [`Context::fail`]; the context tells it as the layers detach.
    failed: bool,
}

/// What became of an edit from outside the layers: see [`Stack::offer`].
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

    /// The lockstep session the run is a peer of.
    #[cfg(test)]
    pub(crate) fn net_mut(&mut self) -> Option<&mut crate::net::Lockstep> {
        match &mut self.net {
            Some(Session::Peer(net)) => Some(net.as_mut()),
            _ => None,
        }
    }

    /// The session the run takes part in, if networked.
    #[cfg(test)]
    pub(crate) fn session_mut(&mut self) -> Option<&mut Session> {
        self.net.as_mut()
    }

    /// The queue of the layer at `index` from the bottom, if it queues commands.
    #[cfg(test)]
    pub(crate) fn commands_of(&mut self, index: usize) -> Option<&mut dyn Commands> {
        self.layers.get_mut(index)?.commands()
    }

    /// What runs each tick again, if sync-tested.
    #[cfg(test)]
    pub(crate) fn sync_test_mut(&mut self) -> Option<&mut SyncTest> {
        self.sync_test.as_mut()
    }

    /// Makes the run a peer of `net`'s lockstep session.
    #[cfg(test)]
    pub(crate) fn lockstep(&mut self, net: crate::net::Lockstep) {
        self.session(Session::Peer(Box::new(net)));
    }

    /// Has the run take part in `session`.
    pub(crate) fn session(&mut self, session: Session) {
        self.net = Some(session);
    }

    /// Applies the layers' [`Context::host`], [`Context::join`] and [`Context::leave`] requests
    /// since the last frame. Runs between ticks, as a frame starts, before the session's turn.
    /// Leaving logs what the session carried and its delay.
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

    /// The session's [`Session::status`] after this frame's turn.
    pub(crate) fn session_status(&self) -> Option<SessionStatus> {
        self.net.as_ref().and_then(Session::status)
    }

    /// Runs each tick, a replay's too, again as a SyncTest (`--sync-test`; see
    /// [`tick`](Self::tick)). Call before the layers attach.
    pub(crate) fn sync_test(&mut self) {
        self.sync_test = Some(SyncTest::default());
    }

    /// Whether the run waits on its session before tick `next`: a joiner without the host's
    /// world, a tick without the other peer's inputs, or a client without the server's world or
    /// too far past its latest state.
    pub(crate) fn net_waits(&self, next: u64) -> bool {
        self.net.as_ref().is_some_and(|net| !net.ready(next))
    }

    /// Waits until `until`. A server sends clients' states as its states thread writes them
    /// ([`Server::wait`](crate::net::Server::wait)); other runs sleep.
    pub(crate) fn net_wait(&mut self, until: Instant) {
        match &mut self.net {
            Some(Session::Server(server)) => server.wait(until),
            _ => std::thread::sleep(until.saturating_duration_since(Instant::now())),
        }
    }

    /// The time a frame `dt` after the last owes the simulation: [`Session::pace`] for a networked
    /// run, else `dt`.
    pub(crate) fn paced(&mut self, dt: Duration) -> Duration {
        self.net.as_mut().map_or(dt, |net| net.pace(dt))
    }

    /// Has a client's frame, `dt` after the last, draw the other players' entities where the
    /// server's states put them ([`Client::interpolate`](crate::net::Client::interpolate)), not
    /// where its ticks left them. Call before the world is drawn.
    pub(crate) fn interpolate(&mut self, cx: &mut Context, dt: Duration) {
        if let Some(Session::Client(client)) = &mut self.net {
            cx.past.instead(&cx.world, client.interpolate(dt));
        }
    }

    /// Takes what the session sent since the last turn ([`Session::turn`]), between ticks as a
    /// frame starts; `next` is the next tick. A host or server sends a joiner or client the world
    /// it owes. A joiner or client restores that world in place of its own, its textures taking the
    /// host's names, also for what it added since.
    ///
    /// Then, in a rollback session or a client, it settles the ticks that may run again
    /// ([`Session::settle`]): rolling back to the first ([`roll_back`](Self::roll_back)), or taking
    /// a client's world to the server's at or past `next`.
    ///
    /// Returns the tick the world is at after a restore or being taken past `next`; ticks go on
    /// from there.
    pub(crate) fn net_turn(&mut self, cx: &mut Context, next: u64) -> Option<u64> {
        let mut net = self.net.take()?;
        let mut restored = None;
        match net.turn(next) {
            Turn::None => {}
            Turn::Send => {
                // A lockstep host drops its logs and the slots they kept vacant for undos, as the
                // joiner's load frees them. So peers give later spawns the same slots, and no undo
                // reaches past the joiner's start. Recorded as an edit between ticks, so a replay
                // drops them there too. A server's clients take its world again with each state.
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
                // A client's player is the server's choice; its layers pick it up as it restores.
                cx.player = net.player();
                let ticks = snapshot.and_then(|snapshot| {
                    // Its textures go by the host's names, whatever ID files this run's disk holds.
                    #[cfg(feature = "dev")]
                    cx.name_as(
                        (snapshot.names.iter()).map(|(path, name)| (name.clone(), path.clone())),
                    );
                    cx.load_restored(snapshot.added());
                    // Its host's scripts, which its world's components follow, replace its own
                    // files', and its host's mods replace its own.
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

    /// Sets `cx`'s [`Context::player`], [`Context::networked`] and server or client state, as the
    /// layers attach or pick up from a restore.
    fn tell_session(&self, cx: &mut Context) {
        cx.player = self.net.as_ref().and_then(Session::player);
        cx.networked = self.net.is_some();
        cx.served = self.served();
        cx.client = matches!(self.net, Some(Session::Client(_)));
        cx.follows |= self.joins();
    }

    /// Whether the run hosts, serves or plays alone, rather than joining or connecting. Only such a
    /// run watches the layers' scene and script files.
    #[cfg(feature = "dev")]
    pub(crate) fn watches_files(&self) -> bool {
        !self.joins()
    }

    /// Whether the run joins a lockstep session or connects to a server.
    fn joins(&self) -> bool {
        self.net.as_ref().is_some_and(Session::follows)
    }

    /// Whether the run serves a server's session (`--serve`) or connects to one (`--connect`).
    fn served(&self) -> bool {
        matches!(self.net, Some(Session::Server(_) | Session::Client(_)))
    }

    /// Whether the run hosts, joins, serves or connects. Its scene files must match its peers'.
    #[cfg(feature = "dev")]
    pub(crate) fn is_peer(&self) -> bool {
        self.net.is_some()
    }

    /// Like [`edit`](Self::edit), but in a lockstep session it's offered for a tick both peers
    /// apply it before, returning the ticket. A server's client refuses it (`net.no_edit`): the
    /// server's next state would undo it.
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

    /// Applies the save of `author`'s scene file, now `text`, between ticks
    /// ([`scene_files::apply_save`]), or offers it in a lockstep session like
    /// [`offer`](Self::offer). It logs what it did.
    #[cfg(feature = "dev")]
    pub(crate) fn offer_save(&mut self, cx: &mut Context, author: String, text: String) {
        match &mut self.net {
            Some(Session::Peer(net)) => {
                // What it names loads now, so the host's names for those go out first. One naming
                // a texture that can't load (the joiner might stand in for it) isn't offered, and
                // warns like a save that doesn't load.
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

    /// Reloads the script at `path` (from the project's root), now `text`, as its file is saved:
    /// an [`edit`](Self::edit), or offered in a lockstep session like [`offer`](Self::offer). It
    /// logs what it did ([`Hosts::reload`](crate::luau::Hosts::reload)).
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

    /// Offers the patch `table` names, like
    /// [`Lockstep::offer_patch`](crate::net::Lockstep::offer_patch). Outside a lockstep session
    /// it gives `table` back: it applies now.
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

    /// Applies the patch `table` names after `ticks` ticks ([`patch::apply`](crate::patch::apply)),
    /// outside a lockstep session ([`offer_patch`](Self::offer_patch)). A recording gets it once
    /// applied ([`Recorder::patched`]).
    #[cfg(feature = "dev")]
    pub(crate) fn patch(&mut self, table: subsecond::JumpTable, ticks: u64) -> Result<()> {
        patched(&mut self.recorder, table, ticks)
    }

    /// What the edit or patch offered with `ticket` did, and the tick it applied before, once it
    /// has. Given once.
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

    /// Fails once the run's recording has stopped ([`Recorder::check`]).
    pub(crate) fn recorded(&self) -> Result<()> {
        self.recorder.as_ref().map_or(Ok(()), Recorder::check)
    }

    /// Loads the layers, then attaches them, each from the bottom up, in `cx`, whose world nothing
    /// has drawn an ID from: its seed is where a recording starts.
    pub(crate) fn attach(&mut self, cx: &mut Context) {
        let _zone = gg_core::zone!();
        if let Some(recorder) = &mut self.recorder {
            recorder.start(&self.names, cx.world.stream_state());
        }
        self.tell_session(cx);
        #[cfg(feature = "dev")]
        {
            cx.dev.sync_tested = self.sync_test.is_some();
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

    /// Loads the layers, puts `snapshot`'s world in `cx`'s place, and has each layer pick up from
    /// it, bottom up: a restored run's start. Returns the ticks run when it was taken, which the
    /// run goes on from, and whether its run was stopped.
    ///
    /// A development build's run serving a tool API that can stop
    /// ([`refuses_stop`](Self::refuses_stop)) goes on as the snapshot's run was. A snapshot taken
    /// while a copy played also holds the world as edited, held aside again like
    /// [`play`](Self::play)'s, with the authors' edits afresh. Any other run plays on with the
    /// world that played, warning `snapshot.plays`, with why, if the snapshot's run was stopped or
    /// held a world as edited.
    ///
    /// Fails like [`Snapshot::restore`].
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
            cx.dev.sync_tested = self.sync_test.is_some();
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
                cx.dev.edited = Some(Edited {
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

    /// Why the tool API's `stop` can't stop the run, if it can't: a recorded run's replay couldn't
    /// follow a world brought back between ticks, and a lockstep peer's world would part from the
    /// other's. Such a run plays from start to end.
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

    /// Plays a copy of `cx`'s world, the world as edited, at `ticks`, in its place: the tool API's
    /// `play` of a stopped run. The world as edited is held aside until [`stop`](Self::stop),
    /// with its ticks, scene files, open scene and the authors' edits; the copy starts their logs
    /// afresh and its open scene as a copy. The layers are told nothing: what they hold of the
    /// world holds of the copy, which goes on exactly as the world would.
    #[cfg(feature = "dev")]
    pub(crate) fn play(&self, cx: &mut Context, ticks: u64) {
        let _zone = gg_core::zone!();
        let copy = cx.world.clone();
        let world = mem::replace(&mut cx.world, copy);
        let entities = world.entities(None).map_or(0, |entities| entities.len());
        cx.dev.edited = Some(Edited {
            world,
            ticks,
            scenes: cx.scenes.files.clone(),
            cached: cx.scenes.cached.clone(),
            open: cx.scenes.open.clone(),
            authors: mem::take(&mut cx.authors),
        });
        info!(ticks, entities, "Playing a copy of the world as edited");
    }

    /// Brings back the world as edited that [`play`](Self::play) held aside, for the tool API's
    /// `stop`: the very world, with its scene files, open scene and authors' edits. The copy and
    /// all done to it go, except scripts reloaded since: the world holds their components as they
    /// declare them now ([`Scripts`](crate::Scripts)). Empties each layer's queue, meant for the
    /// copy's ticks, and has each layer pick up from the world, bottom up ([`Layer::restored`]).
    /// Returns the world's ticks; `None`, changing nothing, for a run playing from its start.
    #[cfg(feature = "dev")]
    pub(crate) fn stop(&mut self, cx: &mut Context) -> Option<u64> {
        let edited = cx.dev.edited.take()?;
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

    /// Has `apply` change `cx`'s world between ticks through a log no author undoes, for the tool
    /// API's `scene open` and `scene new`. Then every author's edits start afresh, since none could
    /// undo past it: the world is [renewed](World::renew), refusing every log's earlier entries;
    /// the authors' logs go; and each layer picks up from the world, bottom up, like
    /// [`stop`](Self::stop), so the in-process Inspector drops its own. If `apply` is refused, its
    /// changes are undone and nothing else changes.
    ///
    /// Fails if `apply` does.
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

    /// Puts `snapshot`'s world in `cx`'s place once the layers loaded: its scene files, open scene
    /// (the start scene's file if it holds none), and scripts' components as the layers' scripts
    /// declare them. Each layer picks up from it, bottom up. Returns the ticks run when it was
    /// taken.
    ///
    /// Fails like [`Snapshot::restore`].
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

    /// Detaches top down, the reverse of attaching, after a recording gets its end, a SyncTest with
    /// no diverging tick logs how many it reran (`sync_test.matched`), and a rollback peer logs how
    /// it rolled back (`net.rollback`).
    /// Takes from `cx` whether a layer failed the run as it detached ([`failed`](Self::failed)).
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

    /// Whether a layer ended the run with an error ([`Context::fail`]), once the layers have
    /// detached. The run's status is then 1.
    pub(crate) fn failed(&self) -> bool {
        self.failed
    }

    #[cfg(feature = "dev")]
    /// Applies an author's `edit` from outside the layers to `cx`'s world between ticks, saying
    /// what it did. A recording gets it first, before the next tick's commands, so a replay
    /// applies it there, refused where the run refused it.
    ///
    /// Fails like [`Authors::edit`].
    pub(crate) fn edit(&mut self, cx: &mut Context, edit: Edit) -> Result<Done> {
        if let Some(recorder) = &mut self.recorder {
            recorder.edited(&edit.to_value(), &cx.world);
        }
        cx.apply_edit(edit)
    }

    /// Runs a tick ([`run`](Self::run)), or returns `false` until the cells it brings in are
    /// prepared ([`Context::cells_ready`]) and the session is ready ([`Session::ready`]). A
    /// lockstep peer waits for the other's inputs (a rollback peer until near enough them), a
    /// client for the server's world and while too far past its latest state. Then the queued
    /// commands go out as inputs ([`Session::start_tick`]).
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

    /// Runs tick `tick`, in order:
    /// 1. Each layer's tick, from the bottom up, the world sharing the context's assets again
    ///    after each ([`Context::share_assets`]).
    /// 2. The world's cells come in and go out ([`Context::stream_cells`]), wherever a tick runs.
    /// 3. The sounds the tick queued play ([`World::play_sound`]).
    ///
    /// A recording gets each layer's commands just before its tick applies them, so a tick that
    /// panics leaves them, and the world's hash after the last.
    ///
    /// In a networked session the tick runs on the session's inputs for it
    /// ([`Session::inputs`]):
    /// - lockstep: both peers', the edits among them applying first, as a recording keeps them;
    /// - rollback, before the other's have come: on them predicted, with a copy of the world kept
    ///   as it starts, for a rollback to run it again from;
    /// - a server: its own and its clients';
    /// - a client: its own.
    ///
    /// Where the tick may run again (a rollback peer's or a client's), its sounds are kept so a
    /// rerun doesn't replay them ([`Context::play_queued_again`]). The session is told once it has
    /// run ([`Session::ran`]).
    ///
    /// A SyncTest copies the world once the edits apply, keeps each layer's commands as its tick
    /// applies them, and reruns the tick ([`run_again`](Self::run_again)). A failure ends the run
    /// and the SyncTest with its error, like [`Context::fail`].
    fn run(
        layers: &mut [Box<dyn Layer>],
        names: &[&'static str],
        recorder: &mut Option<Recorder>,
        mut net: Option<&mut Session>,
        sync_test: &mut Option<SyncTest>,
        cx: &mut Context,
        tick: Tick,
    ) {
        // A client's ticks may all run again, as may a rollback peer's on predicted inputs; the
        // world before them is no more final than after.
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

    /// Takes the world back to where tick `from` started ([`Session::start_of`]): a tick run on
    /// the other peer's predicted inputs, which held something, or a client's server's world after
    /// the tick before. Reruns it and the ticks after, up to `next`, like [`run`](Self::run); the
    /// world goes on from there. It's the very world ([`World::rewind`]), so what's kept of it
    /// holds, and the layers are told nothing. Each layer's queue, holding what it queued since
    /// the last tick, is restored after, and later frames draw on from where the last started.
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

    /// Runs a replay's tick: like [`tick`](Self::tick), never waiting. Cells it brings in are
    /// prepared then, unless the tick before did. `fill` refills each layer's emptied queue, given
    /// the layer's index, its queue if any, and the world as its tick finds it. A SyncTest reruns
    /// it.
    ///
    /// Fails if `fill` does, or like [`run_again`](Self::run_again), which ends the SyncTest.
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

    /// Reruns `tick` for a SyncTest, from the world copy and commands `sync_test` kept. The copy
    /// takes the world's place; each layer is told ([`Layer::swapped`]) as it goes in and as the
    /// world comes back, and gets back its tick's commands. The copy's queued sounds aren't
    /// played: they played on the world. Then it checks the tick left the copy as it left the
    /// world.
    ///
    /// Fails like [`SyncTest::refused`], without running again, and like [`SyncTest::compare`].
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

    /// Draws UI bottom up, so upper layers draw over lower ones. Returns whether any layer draws
    /// UI: any whose `ui` isn't its own type's default ([`Layer::ui`]). One calling another
    /// layer's default counts, whatever it draws.
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

    /// Records `event` in the polled input state, then hands it down from the top until a layer
    /// handles it (`Break`). Releases, sticks or triggers back at rest, and focus loss go on past a
    /// handling layer, as releases pass the UI, so none is left holding a press; a release without
    /// its press changes nothing. A game UI taking the mouse's buttons or wheel
    /// ([`GameUi::event`]) handles them for the layer that sent them.
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

/// A layer in a development build's stack. Each of its methods is called through subsecond's
/// `HotFn` (see [`Layer`]): a vtable call never sees a patch, while `HotFn`, with debug
/// assertions, looks each call up in the last patch's jump table.
///
/// Each call goes through the method as a function item, such as `L::tick`. `HotFn` finds a
/// zero-sized function in the table by its own `call_it`, which the game's crate makes (`L` is the
/// game's), as does the patch. `HotFn` takes anything pointer-sized, a closure holding one
/// included, as a function pointer and calls it as one.
///
/// A method added to `Layer` needs its call here too, or the trait's default runs instead. The
/// stack names the layer by `L`, as `L`'s default `ui` does. A call allocates nothing.
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

/// Applies the patch `table` names after `ticks` ticks ([`patch::apply`](crate::patch::apply));
/// `recorder`, if any, writes it once applied. A replay can't apply it, so it stops where the code
/// changed rather than diverging silently.
#[cfg(feature = "dev")]
fn patched(recorder: &mut Option<Recorder>, table: subsecond::JumpTable, ticks: u64) -> Result<()> {
    let library = table.lib.clone();
    crate::patch::apply(table, ticks)?;
    if let Some(recorder) = recorder {
        recorder.patched(&library);
    }
    Ok(())
}

/// The name of type `L`, without path or generics: `Inspector`.
pub(super) fn short_name<L>() -> &'static str {
    let name = type_name::<L>();
    let name = name.split('<').next().unwrap_or(name);
    name.rsplit("::").next().unwrap_or(name)
}
