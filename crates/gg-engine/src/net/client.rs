//! A client of a server that owns the world (`--connect`;
//! [Networking](../../../../ARCHITECTURE.md#networking)), over the lines and packets
//! [`server`](super::server) has. It starts from the server's world, as far ahead as the round
//! trip and [`LEAD`] more, and runs its ticks on its own inputs alone, sending them for the server
//! to apply at the same ticks. That predicts its own player exactly and the others as going on as
//! their last commands had them.
//!
//! Each state that comes, the world after a server tick, is read into a world of its own (the
//! client's types and assets) and its hash compared with the world after that tick as predicted.
//! Where they differ, the client takes its world back to the server's and runs the ticks after
//! again; its inputs the server hasn't applied apply in them, those whose ticks have gone at the
//! first, as the server would apply them.
//!
//! Its ticks run a little faster or slower than real time so its inputs come [`LEAD`] ticks before
//! the server runs theirs, and catch up at once where they come more than [`LATE`] ticks late. Its
//! frames draw the other players' entities ([`Owner`]) as the latest states have them,
//! [`INTERPOLATION`] ticks behind.
//!
//! A client's ticks allocate, for their inputs' text, the packets and the world's hash, and the
//! states it takes, for their text and the other players' poses.

use std::collections::{BTreeMap, VecDeque};
use std::mem;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use tracing::{debug, error, info, warn};

use super::server::{KEPT, PART, PROTOCOL};
use super::states::rebuild;
use super::{
    Code, Handshaken, Inputs, Link, Looking, MAX_PACKET_TEXT, MAX_WORLD, MOST_PACE, Merged, PACE,
    Queued, REFUSED, Stalls, Turn, connect, hello, tick_of, write_inputs,
};
use crate::args::{Conditions, Toward};
use crate::layer::Commands;
use crate::recording::text;
use crate::{Entity, EntityId, Owner, Sound, Tick, Transform, World, WorldHash, interpolation};

/// How many ticks before the server runs their tick the client's inputs aim to come: 31.25 ms, so
/// an input lost (the next packet carries it again) or held by jitter still comes in time.
pub(super) const LEAD: u64 = 2;

/// How many ticks late the client's inputs may come, as a state says, before it catches up at once
/// rather than at its pace: past the two or three a jittery link has them late, short of the
/// second or so a client joining over a slow link starts behind (the world's age and loading
/// aren't counted where it starts).
pub(super) const LATE: i64 = 4;

/// The most ticks a catching-up frame runs past those its time owes, so its frames run fewer than
/// the stepper's most at 30 frames a second, and it catches up a second behind in 16 frames.
const CATCH_UP: u64 = 4;

/// The most ticks a client runs past the latest state it has: half a second's. Then it waits for
/// the next state.
const WINDOW: u64 = 32;

/// How many ticks behind the latest state the frames draw the other players' entities: 78.125 ms,
/// so past a state lost or held by jitter they draw toward the one after, states coming two ticks
/// apart.
const INTERPOLATION: f64 = 5.0;

/// How many of the latest states' poses of the other players' entities the client keeps.
const SAMPLES: usize = 16;

/// How many of the latest states still coming the client gathers the parts of at once.
const GATHERED: usize = 4;

/// A client.
pub(crate) struct Client {
    /// Where the server is and the session's code: connects by them again each time it's lost.
    toward: Toward,
    code: Code,
    connecting: Option<Receiver<Handshaken>>,
    looking: Looking,
    pub(super) app: &'static str,
    /// The run's [build name](super::Build::name) and layers' names (bottom up): the server's must
    /// match.
    build: String,
    layers: String,
    /// What `--link` has the link do, and the state of the random numbers it draws.
    pub(super) conditions: Option<Conditions>,
    pub(super) random: u64,
    link: Option<Link>,
    /// When the hello went, until the server's world came; the round trip; and when the world came.
    hello: Option<Instant>,
    trip: Duration,
    came: Instant,
    /// The server's world as it comes: its tick, and its lines so far.
    coming: Option<(u64, String)>,
    pub(super) player: Option<u8>,
    /// The tick the server's world was at as it came.
    since: u64,
    /// The client's inputs, by tick, kept until the server has applied them; the layers whose
    /// queues edit (only the server applies their commands, as a tick run again would apply them
    /// again); and the next tick of the client's inputs.
    ours: BTreeMap<u64, Inputs>,
    edits: Vec<usize>,
    tagged: u64,
    /// The first tick of the client's inputs that the world as it stands doesn't hold.
    local: u64,
    /// The text of each of the latest [`KEPT`] states, by tick, whole, which a state is against.
    texts: BTreeMap<u64, String>,
    parts: BTreeMap<u64, Parts>,
    /// The latest state whole, until it's taken between ticks.
    state: Option<Header>,
    /// The server's world, as the latest state taken has it.
    world: World,
    /// The world's hash after each tick run past the latest state taken.
    hashes: BTreeMap<u64, WorldHash>,
    /// The sounds each tick's runs played, past the latest state taken.
    pub(super) played: BTreeMap<u64, Vec<Sound>>,
    /// How many ticks before the server ran their tick the client's inputs came, on average; how
    /// many ticks it still owes, catching up; and, once it has started to, or its world has gone
    /// on from a state past its own ticks, the tick of its inputs the server applies once it's
    /// there: the states until then say what came before.
    pub(super) lead: f64,
    behind: u64,
    caught: Option<u64>,
    /// The poses of the other players' entities in each of the latest states, oldest first; when
    /// the latest came; where among the server's ticks the frames draw them; and what the last
    /// frame drew.
    samples: VecDeque<Sample>,
    arrived: Option<Instant>,
    clock: Option<f64>,
    drawn: Vec<(EntityId, Transform)>,
    /// Corrections of its world; ticks run again; the most at once; states taken. Stalls, past its
    /// window, are [`Stalls`].
    pub(super) corrected: [u64; 4],
    stalls: Stalls,
    /// Whether the latest state's world, read from its text, didn't hash as the server's did.
    desynced: bool,
    sent: Option<Instant>,
    buffer: Vec<u8>,
}

/// A state's line ([`server`](super::server)).
#[derive(Clone, Copy)]
struct Header {
    tick: u64,
    base: Option<u64>,
    next: u64,
    lead: i64,
    hash: WorldHash,
    part: usize,
    parts: usize,
    size: usize,
}

/// The parts of a state that have come, by number, of how many: kept as they come, so a state's
/// line, which anyone at the server's address can forge, holds no memory but what came.
struct Parts {
    of: usize,
    come: BTreeMap<usize, Vec<u8>>,
}

/// The poses of the other players' entities in a state, by ID, and the tick it's after.
struct Sample {
    tick: u64,
    poses: Vec<(EntityId, Transform)>,
}

impl Client {
    /// A client of the server `toward` finds, for app `app`, [build](super::Build::name) `build`
    /// and `layers` (bottom up). Tries to connect in the background until it does.
    pub(crate) fn start(
        toward: &Toward,
        code: Code,
        app: &'static str,
        build: &str,
        layers: &[&str],
    ) -> Self {
        Self {
            toward: toward.clone(),
            code,
            connecting: None,
            looking: Looking::default(),
            app,
            build: build.to_owned(),
            layers: layers.join(" "),
            conditions: None,
            random: 0,
            link: None,
            hello: None,
            trip: Duration::ZERO,
            came: Instant::now(),
            coming: None,
            player: None,
            since: 0,
            ours: BTreeMap::new(),
            edits: Vec::new(),
            tagged: 0,
            local: 0,
            texts: BTreeMap::new(),
            parts: BTreeMap::new(),
            state: None,
            world: World::default(),
            hashes: BTreeMap::new(),
            played: BTreeMap::new(),
            lead: LEAD as f64,
            behind: 0,
            caught: None,
            samples: VecDeque::new(),
            arrived: None,
            clock: None,
            drawn: Vec::new(),
            corrected: [0; 4],
            stalls: Stalls::default(),
            desynced: false,
            sent: None,
            buffer: vec![0; 1 << 16],
        }
    }

    fn joined(&self) -> bool {
        self.link.as_ref().is_some_and(|link| link.joined) && self.coming.is_none()
    }

    /// Whether tick `tick` can run: the client has joined, and the tick is within [`WINDOW`] of
    /// the latest state it has, or of the server's world.
    pub(crate) fn ready(&self, tick: u64) -> bool {
        let latest = self
            .texts
            .last_key_value()
            .map_or(self.since, |(&tick, _)| tick + 1);
        self.joined() && tick < latest + WINDOW
    }

    /// Notes that a tick waited, as it isn't [`ready`](Self::ready). Once joined, past its window,
    /// that starts a stall, unless one has; the next tick that runs ends it.
    pub(crate) fn waited(&mut self) {
        if self.joined() {
            self.stalls.waited(Instant::now());
        }
    }

    /// Between ticks, as a frame starts: connects if it hasn't, saying hello; takes the lines and
    /// packets come since the last turn (`--link` holds them until due), gathering each state's
    /// parts and reading the latest whole; returns the server's world once it has come whole.
    /// Sends its packets again a tick's time after it last did. Logs the server lost or refusing.
    pub(crate) fn turn(&mut self) -> Turn {
        let now = Instant::now();
        self.find(now);
        let (lines, packets, lost) = match &mut self.link {
            Some(link) => link.take(&mut self.buffer, self.conditions, &mut self.random, now),
            None => return Turn::None,
        };
        let mut turn = Turn::None;
        for line in lines {
            if let Some(world) = self.read(&line, now) {
                turn = Turn::Restore(world);
            }
            if self.link.is_none() {
                return Turn::None;
            }
        }
        for packet in packets {
            if !self.joined() {
                break;
            }
            if self.read_packet(&packet, now).is_none() {
                self.lose("the server sent a packet that isn't one");
            }
        }
        if let Some(why) = lost
            && self.link.is_some()
        {
            self.lose(&why);
        }
        if self
            .sent
            .is_some_and(|sent| now.saturating_duration_since(sent) >= Tick::DT)
        {
            self.send_packets();
        }
        turn
    }

    /// Without a link, connects in the background and says hello. Logs a handshake that didn't
    /// hold as refused and tries again after [`REFUSED`].
    fn find(&mut self, now: Instant) {
        if self.link.is_some() {
            return;
        }
        let (toward, code, looking) = (&self.toward, self.code, &self.looking);
        let made = (self.connecting).get_or_insert_with(|| {
            connect(toward.clone(), "the server", Duration::ZERO, code, looking)
        });
        let Ok(handshaken) = made.try_recv() else {
            return;
        };
        self.connecting = None;
        let shaken = match handshaken {
            Ok(shaken) => shaken,
            Err(why) => {
                let again = REFUSED.as_secs();
                error!(
                    code = "net.refused",
                    "The server refused this client in the handshake: {why}; it tries again in \
                     {again} s"
                );
                let toward = self.toward.clone();
                let connecting = connect(toward, "the server", REFUSED, self.code, &self.looking);
                self.connecting = Some(connecting);
                return;
            }
        };
        let hello = |link: &Link| hello(PROTOCOL, self.app, &self.build, &self.layers, link.port());
        let link = Link::new(shaken.stream, shaken.keys, false, shaken.relayed);
        match link.and_then(|mut link| link.send(hello(&link)).map(|()| link)) {
            Ok(link) => {
                match &self.toward {
                    Toward::Address(address) => {
                        info!(address = %address, "Connected to the server")
                    }
                    Toward::Relay(relay) => {
                        info!(relay = %relay, "Connected to the server through its relay");
                    }
                }
                self.link = Some(link);
                self.hello = Some(now);
            }
            Err(error) => warn!(code = "net.link", "Can't reach the server: {error}"),
        }
    }

    /// Takes `line` from the server. Returns the server's world, its tick and text, once its last
    /// line has come.
    fn read(&mut self, line: &str, now: Instant) -> Option<(u64, String)> {
        if let Some((_, text)) = &mut self.coming {
            if text.len() + line.len() >= MAX_WORLD {
                self.lose(&format!(
                    "the server's world is more than {MAX_WORLD} bytes"
                ));
                return None;
            }
            text.push_str(line);
            text.push('\n');
            return (line == "end").then(|| self.coming.take()).flatten();
        }
        let (kind, rest) = line.split_once(' ').unwrap_or((line, ""));
        match kind {
            // The server's writing thread, saying it's there.
            "k" => {}
            "at" => {
                let at = self.link.as_mut().map(|link| link.at(rest));
                if let Some(Err(why)) = at {
                    self.lose(&why);
                }
            }
            "refused" => {
                error!(
                    code = "net.refused",
                    "The server refused this client: {rest}; it tries again in {} s",
                    REFUSED.as_secs()
                );
                self.lose("the server refused it");
                let toward = self.toward.clone();
                let connecting = connect(toward, "the server", REFUSED, self.code, &self.looking);
                self.connecting = Some(connecting);
            }
            "world" => {
                let mut words = rest.split(' ');
                let mut number = || words.next().and_then(tick_of);
                let world = (number(), number(), number());
                match world {
                    (Some(tick), Some(player), Some(port)) => {
                        let (Ok(player), Ok(port)) = (u8::try_from(player), u16::try_from(port))
                        else {
                            self.lose("the server's world names no player or port");
                            return None;
                        };
                        self.trip = self
                            .hello
                            .take()
                            .map_or(Duration::ZERO, |hello| now - hello);
                        self.came = now;
                        self.player = Some(player);
                        if let Some(link) = &mut self.link {
                            link.packets_from(port);
                        }
                        self.coming = Some((tick, String::new()));
                    }
                    _ => self.lose("the server's world names no tick, player or port"),
                }
            }
            _ => self.lose(&format!("the server sent a line that isn't one: {line}")),
        }
        None
    }

    /// Once the server's world at tick `tick` is `world`, in the client's place: says it joined and
    /// returns the tick to go on from, ahead by the round trip and [`LEAD`], at most half
    /// [`WINDOW`], so its inputs come in time.
    pub(crate) fn restored(&mut self, tick: u64, world: &World) -> u64 {
        // The client's types and assets, for each state to be read into.
        self.world.clone_from(world);
        self.since = tick;
        self.ours.clear();
        self.texts.clear();
        self.parts.clear();
        self.state = None;
        self.hashes.clear();
        self.played.clear();
        self.samples.clear();
        (self.arrived, self.clock) = (None, None);
        (self.lead, self.behind, self.caught) = (LEAD as f64, 0, None);
        let trip = (self.trip.as_secs_f64() / Tick::DT.as_secs_f64()).ceil() as u64;
        let ahead = (trip + LEAD).min(WINDOW / 2);
        (self.local, self.tagged) = (tick, tick + ahead);
        if let Some(link) = &mut self.link {
            link.joined = true;
            if let Err(error) = link.send(format!("joined {tick}\n")) {
                self.lose(&format!("can't tell the server it joined: {error}"));
            }
        }
        info!(tick, player = self.player, ahead, "Joined the server");
        tick + ahead
    }

    /// As tick `tick` starts: empties the queues (each layer's, by index) into the client's inputs
    /// for the tick, written against `world`, and sends them with those the server hasn't said it
    /// applied, on the stream if too large for a packet.
    pub(crate) fn start_tick<'q>(
        &mut self,
        tick: u64,
        queues: impl Iterator<Item = (usize, &'q mut dyn Commands)>,
        world: &World,
    ) {
        self.stalls.end(Instant::now());
        let mut inputs = Inputs::default();
        self.edits.clear();
        for (layer, queue) in queues {
            if queue.edits() {
                self.edits.push(layer);
            }
            for i in 0..queue.len() {
                let mut value = String::new();
                text::write(&mut value, &queue.value(i), world);
                inputs.commands.push((layer, value));
            }
            queue.clear();
        }
        // At most: each line's tick and layer, and the packet's first line.
        let lines = inputs
            .commands
            .iter()
            .map(|(_, command)| command.len() + 32);
        inputs.streamed = 64 + lines.sum::<usize>() > MAX_PACKET_TEXT;
        if inputs.streamed
            && let Some(link) = &mut self.link
        {
            let mut text = String::new();
            write_inputs(&mut text, tick, &inputs);
            if let Err(error) = link.send(text) {
                self.lose(&format!("can't send to the server: {error}"));
            }
        }
        self.ours.insert(tick, inputs);
        self.tagged = tick + 1;
        self.send_packets();
    }

    /// What tick `tick` runs on: the client's inputs from the first the world doesn't hold to the
    /// tick's, those of queues that edit aside. So a tick run again after a correction applies
    /// those the server hadn't applied at the first.
    pub(crate) fn inputs(&mut self, tick: u64) -> Merged {
        let mut merged = Merged::default();
        if self.local <= tick {
            for inputs in self.ours.range(self.local..=tick).map(|(_, inputs)| inputs) {
                let theirs = Queued::of(inputs.commands.iter().cloned(), self.player, true);
                let theirs = theirs.filter(|queued| !self.edits.contains(&queued.layer));
                merged.commands.extend(theirs);
            }
            self.local = tick + 1;
        }
        merged
    }

    pub(crate) fn end_tick(&mut self, tick: u64, world: &World) {
        self.hashes.insert(tick, world.hash());
    }

    /// Takes `packet`, a part of a state, while the state is after a tick later than the latest
    /// whole and the server can have run by `now`; reads the state once whole. `None` for a packet
    /// that isn't one.
    fn read_packet(&mut self, packet: &[u8], now: Instant) -> Option<()> {
        let split = packet.iter().position(|&byte| byte == b'\n')?;
        let header = header(std::str::from_utf8(&packet[..split]).ok()?)?;
        let latest = self.texts.last_key_value().map(|(&tick, _)| tick);
        if header.tick < self.since || latest.is_some_and(|latest| header.tick <= latest) {
            return Some(());
        }
        // Past the ticks the server can have run: real time from its world's tick, by its clock, a
        // 256th faster than this one's at most, since no sooner than the round trip before its
        // world came, plus half a second. Such a packet is from elsewhere (only the server's
        // address vouches for it) and would hold back the server's states until their ticks passed
        // its.
        let ran = now.saturating_duration_since(self.came) + self.trip;
        let ran = (ran.as_secs_f64() / Tick::DT.as_secs_f64()) as u64;
        if header.tick > self.since + ran + ran / 256 + WINDOW {
            warn!(
                code = "net.bad_state",
                tick = header.tick,
                "Dropped a state after tick {}, which the server can't have run yet",
                header.tick
            );
            return Some(());
        }
        let parts = self.parts.entry(header.tick);
        let parts = parts.or_insert_with(|| Parts {
            of: header.parts,
            come: BTreeMap::new(),
        });
        if parts.of != header.parts {
            return None;
        }
        (parts.come).insert(header.part, packet[split + 1..].to_vec());
        if parts.come.len() == parts.of {
            let parts = self.parts.remove(&header.tick)?;
            self.parts = self.parts.split_off(&header.tick);
            self.decode(header, parts.come.into_values().flatten().collect(), now);
        }
        while self.parts.len() > GATHERED {
            self.parts.pop_first();
        }
        Some(())
    }

    /// Reads the bytes of the state `header` heads, come whole at `now`: the text of the world
    /// after its tick, against the state it names, [rebuilt](rebuild) whole as the latest whole.
    fn decode(&mut self, header: Header, bytes: Vec<u8>, now: Instant) {
        let tick = header.tick;
        let base = match header.base {
            Some(base) => {
                let Some(base) = self.texts.get(&base) else {
                    debug!(
                        tick,
                        base, "Dropped a state against one the client let go of"
                    );
                    return;
                };
                Some(base.as_str())
            }
            None => None,
        };
        let text = decompress(&bytes, base.map(str::as_bytes), header.size)
            .map_err(|error| error.to_string())
            .and_then(|text| String::from_utf8(text).map_err(|error| error.to_string()))
            .and_then(|text| rebuild(base.unwrap_or(""), &text));
        match text {
            Ok(text) => {
                self.texts.insert(tick, text);
                while self.texts.len() > KEPT {
                    self.texts.pop_first();
                }
                self.state = Some(header);
                self.arrived = Some(now);
                self.corrected[3] += 1;
            }
            Err(error) => warn!(
                code = "net.bad_state",
                tick, "The server's state after tick {tick} isn't one: {error}"
            ),
        }
    }

    /// Between ticks, `next` being the next to run: takes the latest state whole, reading the
    /// server's world after its tick into the client's copy (of `world`'s assets). Logs
    /// `net.desync` if it doesn't hash as the server's did: its text leaves out what the hash
    /// reads. Keeps the other players' poses there. Returns the tick after the state's, for the
    /// client's world to be taken back to the server's there ([`start_of`](Self::start_of)) and the
    /// ticks to `next` run again, unless the prediction after that tick hashes as the server's.
    pub(crate) fn settle(&mut self, next: u64, world: &World) -> Option<u64> {
        let state = self.state.take()?;
        let tick = state.tick;
        // The client's layers have added some since, which the text may name.
        self.world.share_assets(world.shared_assets());
        let lines = (1..).zip(self.texts.get(&tick)?.lines());
        if let Err(error) = self.world.load_whole(lines, true) {
            warn!(
                code = error.code(),
                tick,
                "Can't read the server's world after tick {tick}, which the client doesn't \
                 correct to: {error:#}"
            );
            return None;
        }
        // No state holds the navmesh: keep it up to date in the copy so it holds the resources the
        // client's world does. A world taken back to one copies them all, and a dropped navmesh
        // would be baked again at the next tick.
        crate::navigation::refresh(&mut self.world);
        let hash = self.world.hash();
        if hash == state.hash {
            self.desynced = false;
        } else if !mem::replace(&mut self.desynced, true) {
            error!(
                code = "net.desync",
                tick,
                "The server's world after tick {tick}, read from its text, hashes {hash}, and the \
                 server's {}: its text leaves out what its hash reads",
                state.hash
            );
        }
        self.sample(tick);
        self.ours = self.ours.split_off(&state.next);
        if tick < next {
            self.follow_lead(&state);
        } else {
            // The client fell behind: its world goes on from the state's, past its own ticks.
            // Those ticks' inputs go out empty, as the server ran them without any of the client's,
            // so it doesn't wait for them (it applies a client's inputs in order). The ticks owed,
            // catching up, are fewer by those gone past. The states until the server applies its
            // inputs from there say what came before, and `follow_lead` passes them over, unless a
            // catch-up or a jump before already has them.
            for skipped in next..=tick {
                self.ours.entry(skipped).or_default();
            }
            self.behind = self.behind.saturating_sub(tick + 1 - next);
            self.caught.get_or_insert(tick + 1);
        }
        self.played = self.played.split_off(&(tick + 1));
        let predicted = self.hashes.get(&tick) == Some(&state.hash);
        self.hashes = self.hashes.split_off(&(tick + 1));
        if predicted && tick < next {
            return None;
        }
        self.local = state.next;
        Some(tick + 1)
    }

    /// Takes how early the client's latest inputs came, as `state` says, a state its world doesn't
    /// go on from, into the average its [pace](Self::pace) follows. If they came more than [`LATE`]
    /// ticks late (a client joining over a slow link starts behind, or one whose frames stopped
    /// falls behind), it at once owes the ticks that would have them come [`LEAD`] ticks early,
    /// which its frames pay, predicted. The states after, until the server has applied the inputs
    /// sent once caught up (a round trip or so), say what came before and are passed over.
    fn follow_lead(&mut self, state: &Header) {
        if self.caught.is_some_and(|caught| state.next <= caught) {
            return;
        }
        self.caught = None;
        if state.lead >= -LATE {
            self.lead += (state.lead as f64 - self.lead) / 8.0;
            return;
        }
        // At most a window's: a client further behind goes on from the states, as in `settle`.
        let behind = ((LEAD as i64).saturating_sub(state.lead) as u64).min(WINDOW);
        (self.lead, self.behind, self.caught) = (LEAD as f64, behind, Some(self.tagged + behind));
        debug!(
            tick = state.tick,
            behind, "The client's inputs came late; it catches up {behind} ticks"
        );
    }

    pub(crate) fn start_of(&self, _tick: u64) -> &World {
        &self.world
    }

    pub(crate) fn rolled_back(&mut self, from: u64, next: u64) {
        let ticks = next.saturating_sub(from);
        let [corrections, again, most, _] = &mut self.corrected;
        *corrections += 1;
        *again += ticks;
        *most = (*most).max(ticks);
        debug!(tick = from, ticks, "Corrected toward the server");
    }

    /// Keeps the poses, in the server's world after tick `tick`, of the entities of players other
    /// than the client's ([`Owner`]) and their descendants.
    fn sample(&mut self, tick: u64) {
        let world = &self.world;
        let mut theirs: Vec<Entity> = (world.query::<(Entity, &Owner)>())
            .filter(|(_, owner)| Some(owner.player) != self.player)
            .map(|(entity, _)| entity)
            .collect();
        let mut at = 0;
        while let Some(&entity) = theirs.get(at) {
            if let Ok(children) = world.children(entity) {
                let children: Vec<Entity> = children.collect();
                theirs.extend(children);
            }
            at += 1;
        }
        let poses = theirs
            .into_iter()
            .filter_map(|entity| Some((world.id(entity)?, *world.get::<Transform>(entity)?)));
        let mut poses: Vec<(EntityId, Transform)> = poses.collect();
        poses.sort_unstable_by_key(|&(id, _)| id);
        self.samples.push_back(Sample { tick, poses });
        while self.samples.len() > SAMPLES {
            self.samples.pop_front();
        }
    }

    /// Where a frame `dt` after the last draws the other players' entities, by ID: between the two
    /// states about [`INTERPOLATION`] ticks behind the latest, as far on as the time since the
    /// latest came puts it in the server's ticks, eased from where the last frame drew so a state's
    /// jitter doesn't show. Past the latest, as it has them; none before a state has come.
    pub(crate) fn interpolate(&mut self, dt: Duration) -> &[(EntityId, Transform)] {
        let Self {
            samples,
            arrived,
            clock,
            drawn,
            ..
        } = self;
        drawn.clear();
        let (Some(latest), Some(arrived)) = (samples.back(), *arrived) else {
            return drawn.as_slice();
        };
        let ticks = |time: Duration| time.as_secs_f64() / Tick::DT.as_secs_f64();
        let target = latest.tick as f64 + ticks(arrived.elapsed()) - INTERPOLATION;
        let at = match *clock {
            Some(at) => {
                let on = at + ticks(dt);
                if (target - on).abs() > INTERPOLATION {
                    target
                } else {
                    on + (target - on) / 16.0
                }
            }
            None => target,
        };
        *clock = Some(at);
        let after = samples.partition_point(|sample| sample.tick as f64 <= at);
        let before = after.checked_sub(1).map(|before| &samples[before]);
        match (before, samples.get(after)) {
            (Some(from), Some(to)) => {
                let alpha = ((at - from.tick as f64) / (to.tick - from.tick) as f64) as f32;
                drawn.extend(to.poses.iter().map(|&(id, pose)| {
                    let was = from.poses.binary_search_by_key(&id, |&(of, _)| of);
                    let was = was.map(|was| &from.poses[was].1);
                    (
                        id,
                        was.map_or(pose, |was| interpolation::between(was, &pose, alpha)),
                    )
                }));
            }
            (Some(only), None) | (None, Some(only)) => drawn.extend_from_slice(&only.poses),
            (None, None) => {}
        }
        drawn.as_slice()
    }

    /// The time a frame `dt` after the last owes the client's ticks once joined: a little more or
    /// less as its inputs came, on average, later or earlier than [`LEAD`] ticks before the
    /// server's; while it catches up ([`follow_lead`](Self::follow_lead)), up to [`CATCH_UP`]
    /// ticks' more of those it owes.
    pub(crate) fn pace(&mut self, dt: Duration) -> Duration {
        if !self.joined() {
            return dt;
        }
        let pace = ((LEAD as f64 - self.lead) * PACE).clamp(-MOST_PACE, MOST_PACE);
        let paid = self.behind.min(CATCH_UP);
        self.behind -= paid;
        dt.mul_f64(1.0 + pace) + Tick::DT * paid as u32
    }

    /// Sends the packets once joined ([`server`](super::server)): the latest state it has and the
    /// unapplied inputs that didn't go on the stream, oldest first, as many as fit
    /// [`MAX_PACKET_TEXT`], a tick's in one.
    fn send_packets(&mut self) {
        let joined = self.joined();
        let Some(link) = self.link.as_mut().filter(|_| joined) else {
            return;
        };
        let latest = self.texts.last_key_value();
        let have = latest.map_or_else(|| "-".to_owned(), |(tick, _)| tick.to_string());
        let first = self
            .ours
            .first_key_value()
            .map_or(self.tagged, |(&tick, _)| tick);
        let header = format!("i {have} {first}\n");
        let (mut packet, mut lines) = (header.clone(), String::new());
        for (&tick, inputs) in self.ours.iter().filter(|(_, inputs)| !inputs.streamed) {
            lines.clear();
            write_inputs(&mut lines, tick, inputs);
            if packet.len() + lines.len() > MAX_PACKET_TEXT && packet.len() > header.len() {
                link.send_packet(packet.as_bytes());
                packet.clone_from(&header);
            }
            packet.push_str(&lines);
        }
        link.send_packet(packet.as_bytes());
        self.sent = Some(Instant::now());
    }

    /// Loses the server for `why`, logging the link ([`Link::ended`]) and ending any stall. The
    /// world stands still until it has the server's again.
    pub(crate) fn lose(&mut self, why: &str) {
        if let Some(link) = self.link.take() {
            warn!(code = "net.lost", "Lost the server: {why}");
            link.ended("the server");
        }
        self.stalls.end(Instant::now());
        self.coming = None;
        self.hello = None;
    }

    /// Logs, as the run ends, its link to the server, if any ([`Link::ended`]), and how it was
    /// corrected (`net.corrected`): times, ticks run again, the most at once, states taken, and its
    /// [`Stalls`].
    pub(crate) fn ended(&self) {
        if let Some(link) = &self.link {
            link.ended("the server");
        }
        let [corrections, ticks, most, states] = self.corrected;
        let (stalls, stalled_ms, longest_stall_ms) = self.stalls.logged();
        info!(
            code = "net.corrected",
            corrections,
            ticks,
            most,
            states,
            stalls,
            stalled_ms,
            longest_stall_ms,
            "Took {states} states, and corrected toward the server's {corrections} times, running \
             {ticks} ticks again, at most {most} at once; past its window, it stalled {stalls} \
             times, for {stalled_ms} ms together, the longest {longest_stall_ms} ms"
        );
    }
}

/// The `size` bytes that zstd-compressed `bytes` decompress to, against the dictionary `base` if
/// given. Read as they decompress, not into `size` bytes held at once, so a state's line, which a
/// hostile server may write as it likes, takes no more memory than its bytes decompress to.
///
/// Fails as zstd does, and for bytes that decompress to more or fewer than `size`.
fn decompress(bytes: &[u8], base: Option<&[u8]>, size: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;

    // Room for up to a MiB at once, which a forged line can't exceed; past it the text grows as
    // its bytes decompress.
    let mut text = Vec::with_capacity(size.min(1 << 20));
    match base {
        Some(base) => zstd::stream::read::Decoder::with_dictionary(bytes, base)?
            .take(size as u64 + 1)
            .read_to_end(&mut text)?,
        None => zstd::stream::read::Decoder::with_buffer(bytes)?
            .take(size as u64 + 1)
            .read_to_end(&mut text)?,
    };
    if text.len() != size {
        let message = format!("its bytes aren't the {size} its line says");
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        ));
    }
    Ok(text)
}

/// Reads a state's line ([`server`](super::server)); `None` if it isn't one.
fn header(line: &str) -> Option<Header> {
    let mut words = line.strip_prefix("s ")?.split(' ');
    let tick = words.next()?.parse().ok()?;
    let base = match words.next()? {
        "-" => None,
        base => Some(base.parse().ok()?),
    };
    let (next, lead) = (words.next()?.parse().ok()?, words.next()?.parse().ok()?);
    let hex = |text: &str| u64::from_str_radix(text, 16).ok();
    let hash = WorldHash {
        content: hex(words.next()?)?,
        layout: hex(words.next()?)?,
    };
    let part = words.next()?.parse().ok()?;
    let parts = words.next()?.parse().ok()?;
    let size = words.next()?.parse().ok()?;
    let fits = part < parts && parts <= MAX_WORLD / PART && size <= MAX_WORLD;
    (words.next().is_none() && fits).then_some(Header {
        tick,
        base,
        next,
        lead,
        hash,
        part,
        parts,
        size,
    })
}

#[cfg(test)]
mod tests {
    use gg_core::math::{DVec3, Vec3};

    use super::super::{BUILD, TESTED};
    use super::*;

    fn at(x: f32) -> Transform {
        Transform {
            translation: DVec3::new(f64::from(x), 0.0, 0.0),
            ..Transform::IDENTITY
        }
    }

    fn x_of(drawn: &[(EntityId, Transform)], id: EntityId) -> f32 {
        let found = drawn.iter().find(|&&(of, _)| of == id);
        found.expect("drawn").1.translation.x as f32
    }

    /// A state after a tick the server can't have run yet, however far on, as a packet from
    /// elsewhere may name, is dropped; the server's own after it are taken.
    #[test]
    fn a_state_the_server_cant_have_run_yet_is_dropped() {
        let mut client = Client::start(
            &Toward::Address("127.0.0.1:1".to_owned()),
            TESTED,
            "Tested",
            BUILD,
            &[],
        );
        let now = Instant::now();
        (client.since, client.came) = (100, now);
        let state = |tick: u64| {
            let text = "gg-snapshot 2\n";
            let bytes = zstd::bulk::compress(text.as_bytes(), 3).expect("compressed");
            let line = format!("s {tick} - 0 0 0 0 0 1 {}\n", text.len());
            [line.as_bytes(), &bytes].concat()
        };
        for forged in [100 + 10_000, u64::MAX] {
            assert_eq!(client.read_packet(&state(forged), now), Some(()));
        }
        assert!(client.texts.is_empty());
        assert!(!client.ready(101), "not joined");
        client.read_packet(&state(101), now).expect("a state");
        assert_eq!(client.texts.keys().copied().collect::<Vec<_>>(), [101]);
    }

    /// A forged state line holds the client to no more memory than its packets bring, however many
    /// parts and bytes it claims: parts are kept as they come, bytes read as they decompress, and a
    /// state whose bytes come to more or fewer than its line says is dropped.
    #[test]
    fn a_forged_state_s_line_holds_no_memory_but_what_came() {
        let mut client = Client::start(
            &Toward::Address("127.0.0.1:1".to_owned()),
            TESTED,
            "Tested",
            BUILD,
            &[],
        );
        let now = Instant::now();
        (client.since, client.came) = (100, now);
        let text = "gg-snapshot 2\n";
        let bytes = zstd::bulk::compress(text.as_bytes(), 3).expect("compressed");
        let state = |tick: u64, part: usize, parts: usize, size: usize| {
            let line = format!("s {tick} - 0 0 0 0 {part} {parts} {size}\n");
            [line.as_bytes(), &bytes].concat()
        };
        let most = MAX_WORLD / PART;
        client.read_packet(&state(101, 7, most, MAX_WORLD), now);
        assert_eq!(client.parts[&101].come.len(), 1);
        for size in [MAX_WORLD, text.len() + 1, text.len() - 1] {
            client.read_packet(&state(102, 0, 1, size), now);
            assert!(client.texts.is_empty(), "{size}");
        }
        client.read_packet(&state(103, 0, 1, text.len()), now);
        assert_eq!(client.texts.keys().copied().collect::<Vec<_>>(), [103]);
        let base = Some(text.as_bytes());
        let against = zstd::bulk::Compressor::with_dictionary(3, text.as_bytes())
            .and_then(|mut compressor| compressor.compress(text.as_bytes()))
            .expect("compressed");
        let read = decompress(&against, base, text.len()).expect("decompressed");
        assert_eq!(read, text.as_bytes());
    }

    /// A state rebuilds whole against its base, kept for the next; one whose `kept` lines name more
    /// than its base holds is `net.bad_state` and dropped.
    #[test]
    fn a_state_rebuilds_against_its_base_or_is_bad() {
        let logged = crate::testing::logged();
        let mut client = Client::start(
            &Toward::Address("127.0.0.1:1".to_owned()),
            TESTED,
            "Tested",
            BUILD,
            &[],
        );
        let now = Instant::now();
        (client.since, client.came) = (100, now);
        let state = |tick: u64, base: Option<(u64, &str)>, text: &str| {
            let bytes = match base {
                Some((_, base)) => zstd::bulk::Compressor::with_dictionary(3, base.as_bytes())
                    .and_then(|mut compressor| compressor.compress(text.as_bytes())),
                None => zstd::bulk::compress(text.as_bytes(), 3),
            };
            let against = base.map_or_else(|| "-".to_owned(), |(tick, _)| tick.to_string());
            let line = format!("s {tick} {against} 0 0 0 0 0 1 {}\n", text.len());
            [line.as_bytes(), &bytes.expect("compressed")].concat()
        };
        let base = "stream 1\ntable A\nrow 0 1 [Int(1)]\nresource Cells in\n";
        client.read_packet(&state(101, None, base), now);
        let kept = "stream 2\nkept tables 1\nkept resource Cells\n";
        client.read_packet(&state(102, Some((101, base)), kept), now);
        assert_eq!(client.texts[&102], base.replace("stream 1", "stream 2"));
        for (tick, bad) in [(103, "kept tables 2\n"), (104, "kept resource Audio\n")] {
            client.read_packet(&state(tick, Some((101, base)), bad), now);
            assert!(!client.texts.contains_key(&tick), "{bad}");
        }
        let codes: Vec<String> = logged.codes.try_iter().collect();
        let bad = codes.iter().filter(|&code| code == "net.bad_state");
        assert_eq!(bad.count(), 2, "{codes:?}");
    }

    /// The other players' entities draw between the two states [`INTERPOLATION`] ticks behind the
    /// latest, by ID (as the later state has one the earlier hasn't), and as the latest has them
    /// once that time is past it.
    #[test]
    fn the_others_draw_between_the_states_behind_the_latest() {
        let mut world = World::seeded(1);
        let [a, b] = [0.0, 1.0].map(|x| world.spawn(at(x)));
        let [a, b] = [a, b].map(|entity| world.id(entity).expect("alive"));
        let sample = |tick, poses: &[(EntityId, Transform)]| {
            let mut poses = poses.to_vec();
            poses.sort_unstable_by_key(|&(id, _)| id);
            Sample { tick, poses }
        };
        // The latest `INTERPOLATION` ticks past 11.
        let samples = [
            sample(10, &[(a, at(0.0))]),
            sample(12, &[(a, at(2.0)), (b, at(7.0))]),
            sample(11 + INTERPOLATION as u64, &[(a, at(4.0)), (b, at(9.0))]),
        ];
        // The latest came `ago` before now.
        let client = |ago: Duration| {
            let mut client = Client::start(
                &Toward::Address("127.0.0.1:1".to_owned()),
                TESTED,
                "Tested",
                BUILD,
                &[],
            );
            client.samples = samples.iter().map(|s| sample(s.tick, &s.poses)).collect();
            client.arrived = Some(Instant::now().checked_sub(ago).expect("an instant"));
            client
        };
        // 11 ticks on: halfway from the state after tick 10 to the one after 12, plus the time
        // since the latest came, under 3 ms.
        let mut between = client(Duration::ZERO);
        let drawn = between.interpolate(Tick::DT);
        assert!((1.0..1.1).contains(&x_of(drawn, a)), "{drawn:?}");
        assert_eq!(x_of(drawn, b), 7.0);
        // The latest came 10 ticks ago: the time is past it.
        let mut held = client(Tick::DT * 10);
        let drawn = held.interpolate(Tick::DT);
        assert_eq!((x_of(drawn, a), x_of(drawn, b)), (4.0, 9.0));
    }

    /// A correction takes the client's world back to the server's latest state, keeping the
    /// navmesh: no state holds it, so the copy keeps its own up to date, and no tick after bakes it
    /// again.
    #[test]
    fn a_correction_keeps_the_navmesh() {
        use crate::navigation::navmesh_of;
        use crate::{BodyType, BoxCollider3D, NavMeshVolume, RigidBody3D, step_navigation};

        let mut server = World::seeded(3);
        let floor = Transform {
            translation: DVec3::new(0.0, -0.5, 0.0),
            scale: Vec3::new(10.0, 1.0, 10.0),
            ..Transform::IDENTITY
        };
        let fixed = RigidBody3D {
            body_type: BodyType::Static,
            ..RigidBody3D::default()
        };
        server.spawn((floor, fixed, BoxCollider3D::default()));
        let volume = NavMeshVolume {
            size: Vec3::new(12.0, 4.0, 12.0),
            ..NavMeshVolume::default()
        };
        server.spawn((Transform::IDENTITY, volume));
        step_navigation(&mut server);
        let mut text = String::new();
        server.save_whole(&mut text).expect("saved");
        // The client's world, as it joined.
        let mut ours = World::seeded(4);
        for registered in [
            ours.register::<Transform>(),
            ours.register::<RigidBody3D>(),
            ours.register::<BoxCollider3D>(),
            ours.register::<NavMeshVolume>(),
        ] {
            registered.expect("registered");
        }
        ours.load_whole((1..).zip(text.lines()), true)
            .expect("loaded");
        let mut client = Client::start(
            &Toward::Address("127.0.0.1:1".to_owned()),
            TESTED,
            "Tested",
            BUILD,
            &[],
        );
        client.restored(0, &ours);
        step_navigation(&mut ours);
        client.texts.insert(1, text.clone());
        client.state = Some(Header {
            tick: 1,
            base: None,
            next: 2,
            lead: 0,
            hash: server.hash(),
            part: 0,
            parts: 1,
            size: text.len(),
        });
        let from = client.settle(2, &ours).expect("a correction");
        ours.rewind(client.start_of(from));
        let kept = navmesh_of(&ours);
        assert!(kept.is_some(), "the copy held it");
        assert_eq!(kept, navmesh_of(&client.world), "shared with the copy");
        step_navigation(&mut ours);
        assert_eq!(navmesh_of(&ours), kept, "not baked again");
    }
}
