//! A client of a server that owns the world (`--connect`), as
//! [Networking](../../../../ARCHITECTURE.md#networking) describes, over the lines and packets
//! [`server`](super::server) has: it starts from the server's world, as a lockstep joiner starts
//! from its host's, as far ahead of the server as the way there and back and [`LEAD`] more, and
//! runs its ticks on its own inputs alone, sending them for the server to apply at the same ticks,
//! which predicts its own player exactly and the others as going on as their last commands had
//! them; and each state that comes, the world after a tick of the server's, it reads into a world
//! of its own, of the client's types and assets, compares with the world after that tick as it
//! predicted it, by their hashes, and where they differ, takes its world back to the server's and
//! runs the ticks after again, its inputs the server hasn't applied applying in them, those whose
//! ticks have gone at the first, as the server would apply them. Its ticks run a little faster or
//! slower than real time, so that its inputs come [`LEAD`] ticks before the server runs theirs; and
//! its frames draw the other players' entities, as [`Owner`] says, as the latest states have them,
//! [`INTERPOLATION`] ticks behind.
//!
//! A client's ticks allocate, for their inputs' text, the packets and the world's hash, and the
//! states it takes, for their text and the other players' poses.

use std::collections::{BTreeMap, VecDeque};
use std::mem;
use std::net::TcpStream;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use tracing::{debug, error, info, warn};

use super::server::{KEPT, PART, PROTOCOL};
use super::{
    Inputs, Link, MAX_PACKET, MAX_WORLD, Merged, REFUSED, Turn, connect, hello, tick_of,
    write_inputs,
};
use crate::args::Conditions;
use crate::layer::Commands;
use crate::recording::text;
use crate::{Entity, EntityId, Owner, Sound, Tick, Transform, World, WorldHash, interpolation};

/// How many ticks before the server runs their tick the client's inputs aim to come: 31.25 ms, so
/// that one lost, which the next packet carries again, or held by a link's jitter, still comes in
/// time.
pub(super) const LEAD: u64 = 2;

/// How much faster or slower than real time the client's ticks run for each tick its inputs come
/// later or earlier than [`LEAD`], on average, and the most, which takes it back within seconds
/// without overshooting much.
const PACE: f64 = 0.02;
const MOST_PACE: f64 = 0.1;

/// The most ticks a client runs past the latest it has the server's state after, half a second's,
/// after which it waits for the next state, as over a link slower than that it would.
const WINDOW: u64 = 32;

/// How many ticks behind the latest state come the frames draw the other players' entities: 46.875
/// ms, so that past a state lost, or held by a link's jitter, they draw toward the one after.
const INTERPOLATION: f64 = 3.0;

/// How many of the latest states' poses of the other players' entities the client keeps.
const SAMPLES: usize = 16;

/// How many states still coming the client gathers the parts of at once, the latest.
const GATHERED: usize = 4;

/// A client.
pub(crate) struct Client {
    address: String,
    connecting: Option<Receiver<TcpStream>>,
    pub(super) app: &'static str,
    /// The layers' names, from the bottom up, which the server's must be.
    layers: String,
    /// What `--link` has the link do to what comes, and the state of the random numbers it draws.
    pub(super) conditions: Option<Conditions>,
    pub(super) random: u64,
    link: Option<Link>,
    /// When the hello went, until the server's world came, and the time between, the way to the
    /// server and back; and when the world came.
    hello: Option<Instant>,
    trip: Duration,
    came: Instant,
    /// The server's world as it comes: the tick it's at, and its lines so far.
    coming: Option<(u64, String)>,
    /// The client's player, as the server's world gave it.
    pub(super) player: Option<u8>,
    /// The tick the server's world was at as it came.
    since: u64,
    /// The client's inputs, by tick, kept until the server has applied them; the layers whose
    /// queues edit, whose commands the server alone applies, as a tick that the client runs again
    /// would apply them again; and the next tick of the client's inputs.
    ours: BTreeMap<u64, Inputs>,
    edits: Vec<usize>,
    tagged: u64,
    /// The first tick of the client's inputs that the world as it stands doesn't hold.
    local: u64,
    /// The text of each of the latest [`KEPT`] states, by tick, which a state is compressed
    /// against.
    texts: BTreeMap<u64, String>,
    /// The parts of the latest states still coming, by tick.
    parts: BTreeMap<u64, Parts>,
    /// The latest state whole, until it's taken between ticks.
    state: Option<Header>,
    /// The server's world, as the latest state taken has it.
    world: World,
    /// The world's hash after each tick the client ran past the latest state taken, by tick.
    hashes: BTreeMap<u64, WorldHash>,
    /// The sounds each tick's runs played, past the latest state taken, by tick.
    pub(super) played: BTreeMap<u64, Vec<Sound>>,
    /// How many ticks before the server ran their tick the client's inputs came, on average.
    pub(super) lead: f64,
    /// The poses of the other players' entities in each of the latest states, oldest first, and
    /// when the latest came; and where among the server's ticks the frames draw them, and what they
    /// draw, the last frame's.
    samples: VecDeque<Sample>,
    arrived: Option<Instant>,
    clock: Option<f64>,
    drawn: Vec<(EntityId, Transform)>,
    /// How many times the client corrected its world, how many ticks it ran again, the most at
    /// once, and how many states it took.
    pub(super) corrected: [u64; 4],
    /// Whether the latest state's world, read from its text, didn't hash as the server's did.
    desynced: bool,
    /// When the client last sent its packets.
    sent: Option<Instant>,
    /// What a packet is read into.
    buffer: Vec<u8>,
}

/// A state's line, as [`server`](super::server)'s states have it.
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

/// The parts of a state that have come.
struct Parts(Vec<Option<Vec<u8>>>);

/// The poses of the other players' entities in a state, by their IDs, and the tick it's after.
struct Sample {
    tick: u64,
    poses: Vec<(EntityId, Transform)>,
}

impl Client {
    /// A client of the server at `address`, for app `app`, whose layers are `layers`, from the
    /// bottom up, which tries to connect to it in the background, until it does.
    pub(crate) fn start(address: &str, app: &'static str, layers: &[&str]) -> Self {
        Self {
            address: address.to_owned(),
            connecting: None,
            app,
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
            samples: VecDeque::new(),
            arrived: None,
            clock: None,
            drawn: Vec::new(),
            corrected: [0; 4],
            desynced: false,
            sent: None,
            buffer: vec![0; 1 << 16],
        }
    }

    /// Whether the client has joined: it has the server's world, which it said it restored.
    fn joined(&self) -> bool {
        self.link.as_ref().is_some_and(|link| link.joined) && self.coming.is_none()
    }

    /// Whether tick `tick` can run: the client has joined, and the tick is within [`WINDOW`] of
    /// the latest it has the server's state after, or of the server's world.
    pub(crate) fn ready(&self, tick: u64) -> bool {
        let latest = self
            .texts
            .last_key_value()
            .map_or(self.since, |(&tick, _)| tick + 1);
        self.joined() && tick < latest + WINDOW
    }

    /// Between ticks, as a frame starts: connects to the server, if the client hasn't, saying hello;
    /// takes the lines read and the packets come since the last turn, those `--link` holds once
    /// they're due, gathering each state's parts, and reading the latest whole; and returns the
    /// server's world, once it has come whole. Sends the client's packets again a tick's time after
    /// it last did, as a client waiting for a state does. Logs the server lost or refusing it.
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
        if lost && self.link.is_some() {
            self.lose("the connection closed");
        }
        if self
            .sent
            .is_some_and(|sent| now.saturating_duration_since(sent) >= Tick::DT)
        {
            self.send_packets();
        }
        turn
    }

    /// Connects to the server, in the background, if the client has no link, and says hello once
    /// it has, at `now`.
    fn find(&mut self, now: Instant) {
        if self.link.is_some() {
            return;
        }
        let address = &self.address;
        let made = (self.connecting)
            .get_or_insert_with(|| connect(address.clone(), "the server", Duration::ZERO));
        let Ok(stream) = made.try_recv() else { return };
        self.connecting = None;
        let hello = |link: &Link| hello(PROTOCOL, self.app, &self.layers, link.port());
        match Link::new(stream).and_then(|mut link| link.send(&hello(&link)).map(|()| link)) {
            Ok(link) => {
                info!(address = %self.address, "Connected to the server");
                self.link = Some(link);
                self.hello = Some(now);
            }
            Err(error) => warn!(code = "net.link", "Can't reach the server: {error}"),
        }
    }

    /// Takes `line`, from the server, at `now`; returns the server's world, the tick it's at and its
    /// text, once its last line has come.
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
            "refused" => {
                error!(
                    code = "net.refused",
                    "The server refused this client: {rest}; it tries again in {} s",
                    REFUSED.as_secs()
                );
                self.lose("the server refused it");
                let address = self.address.clone();
                self.connecting = Some(connect(address, "the server", REFUSED));
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

    /// Once the server's world at tick `tick` is `world`, in the client's world's place: says it has
    /// joined, and returns the tick the client goes on from, as far ahead as the way to the server
    /// and back and [`LEAD`] more, at most half [`WINDOW`], so that its inputs come in time.
    pub(crate) fn restored(&mut self, tick: u64, world: &World) -> u64 {
        // Of the client's types and assets, as the server's are, for each state to be read into.
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
        self.lead = LEAD as f64;
        let trip = (self.trip.as_secs_f64() / Tick::DT.as_secs_f64()).ceil() as u64;
        let ahead = (trip + LEAD).min(WINDOW / 2);
        (self.local, self.tagged) = (tick, tick + ahead);
        if let Some(link) = &mut self.link {
            link.joined = true;
            if let Err(error) = link.send(&format!("joined {tick}\n")) {
                self.lose(&format!("can't tell the server it joined: {error}"));
            }
        }
        info!(tick, player = self.player, ahead, "Joined the server");
        tick + ahead
    }

    /// As tick `tick` starts: takes the queues' commands, each layer's by its index, written
    /// against `world`, as the client's inputs for the tick, emptying them, and sends them, with
    /// those the server hasn't said it applied, on the stream if they're too many bytes for a
    /// packet.
    pub(crate) fn start_tick<'q>(
        &mut self,
        tick: u64,
        queues: impl Iterator<Item = (usize, &'q mut dyn Commands)>,
        world: &World,
    ) {
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
        // Each line, its tick and its layer, and the packet's first, at most.
        let lines = inputs
            .commands
            .iter()
            .map(|(_, command)| command.len() + 32);
        inputs.streamed = 64 + lines.sum::<usize>() > MAX_PACKET;
        if inputs.streamed
            && let Some(link) = &mut self.link
        {
            let mut text = String::new();
            write_inputs(&mut text, tick, &inputs);
            if let Err(error) = link.send(&text) {
                self.lose(&format!("can't send to the server: {error}"));
            }
        }
        self.ours.insert(tick, inputs);
        self.tagged = tick + 1;
        self.send_packets();
    }

    /// What tick `tick` runs on: the client's inputs from the first the world doesn't hold to the
    /// tick's, those of queues that edit aside, so that a tick run again after a correction applies
    /// those whose ticks the server hadn't applied at the first.
    pub(crate) fn inputs(&mut self, tick: u64) -> Merged {
        let mut merged = Merged::default();
        if self.local <= tick {
            for inputs in self.ours.range(self.local..=tick).map(|(_, inputs)| inputs) {
                let theirs = inputs.commands.iter();
                let theirs = theirs.filter(|(layer, _)| !self.edits.contains(layer));
                merged.commands.extend(theirs.cloned());
            }
            self.local = tick + 1;
        }
        merged
    }

    /// Once tick `tick` has run, leaving `world`: keeps its hash, for the server's state after the
    /// tick to be compared with.
    pub(crate) fn end_tick(&mut self, tick: u64, world: &World) {
        self.hashes.insert(tick, world.hash());
    }

    /// Takes `packet`, a part of a state, which the client gathers while it's of a state after a
    /// tick later than the latest whole, and the server can have run by `now`, reading a state
    /// once it's whole. `None` for a packet that isn't one.
    fn read_packet(&mut self, packet: &[u8], now: Instant) -> Option<()> {
        let split = packet.iter().position(|&byte| byte == b'\n')?;
        let header = header(std::str::from_utf8(&packet[..split]).ok()?)?;
        let latest = self.texts.last_key_value().map(|(&tick, _)| tick);
        if header.tick < self.since || latest.is_some_and(|latest| header.tick <= latest) {
            return Some(());
        }
        // Past the ticks the server can have run, which go on in real time from its world's, as its
        // clock has it, a 256th faster than this one's at most, since no sooner than the way there
        // and back before its world came, and half a second's more: a packet from elsewhere, which
        // the server's address alone vouches for, which would hold back the server's states until
        // their ticks passed its.
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
        let parts = parts.or_insert_with(|| Parts(vec![None; header.parts]));
        if parts.0.len() != header.parts {
            return None;
        }
        parts.0[header.part] = Some(packet[split + 1..].to_vec());
        if parts.0.iter().all(Option::is_some) {
            let Parts(parts) = self.parts.remove(&header.tick)?;
            self.parts = self.parts.split_off(&header.tick);
            self.decode(header, parts.into_iter().flatten().flatten().collect(), now);
        }
        while self.parts.len() > GATHERED {
            self.parts.pop_first();
        }
        Some(())
    }

    /// Reads the bytes of the state `header` heads, come whole at `now`, as the text of the world
    /// after its tick, against the state it names, as the latest whole.
    fn decode(&mut self, header: Header, bytes: Vec<u8>, now: Instant) {
        let tick = header.tick;
        let text = match header.base {
            Some(base) => {
                let Some(base) = self.texts.get(&base) else {
                    debug!(
                        tick,
                        base, "Dropped a state against one the client let go of"
                    );
                    return;
                };
                zstd::bulk::Decompressor::with_dictionary(base.as_bytes())
                    .and_then(|mut decompressor| decompressor.decompress(&bytes, header.size))
            }
            None => zstd::bulk::decompress(&bytes, header.size),
        };
        let text = text
            .map_err(|error| error.to_string())
            .and_then(|text| String::from_utf8(text).map_err(|error| error.to_string()));
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

    /// Between ticks, the next to run being `next`, `world` the client's: takes the latest state
    /// whole, reading the server's world after its tick into the client's own copy, of `world`'s
    /// assets, and logging `net.desync` if it doesn't hash as the server's did, as the state's text
    /// then leaves out what the hash reads; keeps the other players' entities' poses there; and
    /// returns the tick after the state's, for the client's world to be taken back to the server's
    /// there, [`start_of`](Self::start_of), and the ticks from it to `next` run again, unless the
    /// world the client predicted after that tick hashes as the server's.
    pub(crate) fn settle(&mut self, next: u64, world: &World) -> Option<u64> {
        let state = self.state.take()?;
        let tick = state.tick;
        // As the client's layers have added since, which the text may name.
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
        // The navmesh, which no state holds, kept up to date in the copy, so that the copy holds
        // the resources the client's world does: a world taken back to one holding other types'
        // copies them all, and a navmesh dropped there would be baked again at the next tick.
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
        self.lead += (state.lead as f64 - self.lead) / 8.0;
        self.played = self.played.split_off(&(tick + 1));
        let predicted = self.hashes.get(&tick) == Some(&state.hash);
        self.hashes = self.hashes.split_off(&(tick + 1));
        if predicted && tick < next {
            return None;
        }
        self.local = state.next;
        Some(tick + 1)
    }

    /// The world as tick `tick`, which [`settle`](Self::settle) gave, starts: the server's after
    /// the tick before.
    pub(crate) fn start_of(&self, _tick: u64) -> &World {
        &self.world
    }

    /// Counts a correction from tick `from` to `next`, the next to run, the ticks between run
    /// again.
    pub(crate) fn corrected(&mut self, from: u64, next: u64) {
        let ticks = next.saturating_sub(from);
        let [corrections, again, most, _] = &mut self.corrected;
        *corrections += 1;
        *again += ticks;
        *most = (*most).max(ticks);
        debug!(tick = from, ticks, "Corrected toward the server");
    }

    /// Keeps the poses, in the server's world after tick `tick`, of the entities of players other
    /// than the client's, as [`Owner`] says, and of their descendants.
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

    /// Where a frame `dt` after the last draws the other players' entities: each by its ID, as far
    /// on between the two states about [`INTERPOLATION`] ticks behind the latest as that time is,
    /// in the server's ticks, as the time since the latest came puts it, eased toward it from where
    /// the last frame drew, so that a state's jitter doesn't show; as the latest has them past it,
    /// and none before a state has come.
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

    /// The time a frame `dt` after the last owes the client's ticks: a little more or less, as its
    /// inputs came later or earlier than [`LEAD`] ticks before the server ran their ticks, on
    /// average, once it has joined.
    pub(crate) fn pace(&self, dt: Duration) -> Duration {
        if !self.joined() {
            return dt;
        }
        let pace = ((LEAD as f64 - self.lead) * PACE).clamp(-MOST_PACE, MOST_PACE);
        dt.mul_f64(1.0 + pace)
    }

    /// Sends the server the client's packets, once it has joined: the latest state it has, and its
    /// inputs for each tick the server hasn't said it applied that didn't go on the stream, oldest
    /// first, as many to a packet as fit [`MAX_PACKET`], a tick's in one.
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
            if packet.len() + lines.len() > MAX_PACKET && packet.len() > header.len() {
                link.send_packet(packet.as_bytes());
                packet.clone_from(&header);
            }
            packet.push_str(&lines);
        }
        link.send_packet(packet.as_bytes());
        self.sent = Some(Instant::now());
    }

    /// Loses the server, as `why` says: the client's world stands still until it has the server's
    /// world again.
    pub(crate) fn lose(&mut self, why: &str) {
        if self.link.is_some() {
            warn!(code = "net.lost", "Lost the server: {why}");
        }
        self.link = None;
        self.coming = None;
        self.hello = None;
    }

    /// Logs how the client was corrected toward the server (`net.corrected`), as the run ends: how
    /// many times, how many ticks it ran again, the most at once, and how many states it took.
    pub(crate) fn ended(&self) {
        let [corrections, ticks, most, states] = self.corrected;
        info!(
            code = "net.corrected",
            corrections,
            ticks,
            most,
            states,
            "Took {states} states, and corrected toward the server's {corrections} times, running \
             {ticks} ticks again, at most {most} at once"
        );
    }
}

/// Reads a state's line, which [`server`](super::server)'s states start with; `None` if it isn't
/// one.
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
    use gg_core::math::Vec3;

    use super::*;

    /// A transform at `x`.
    fn at(x: f32) -> Transform {
        Transform {
            translation: Vec3::new(x, 0.0, 0.0),
            ..Transform::IDENTITY
        }
    }

    /// The x where `drawn` has the entity `id` drawn.
    fn x_of(drawn: &[(EntityId, Transform)], id: EntityId) -> f32 {
        let found = drawn.iter().find(|&&(of, _)| of == id);
        found.expect("drawn").1.translation.x
    }

    /// A state after a tick the server can't have run yet, as a packet from elsewhere may name,
    /// however far on, is dropped, and the server's own after it are taken.
    #[test]
    fn a_state_the_server_cant_have_run_yet_is_dropped() {
        let mut client = Client::start("127.0.0.1:1", "Tested", &[]);
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

    /// The other players' entities draw between the two states [`INTERPOLATION`] ticks behind the
    /// latest, as far on as that time is, each by its ID, or as the later state has one the earlier
    /// hasn't; and as the latest has them, once that time is past it.
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
        let samples = [
            sample(10, &[(a, at(0.0))]),
            sample(12, &[(a, at(2.0)), (b, at(7.0))]),
            sample(14, &[(a, at(4.0)), (b, at(9.0))]),
        ];
        let client = |arrived: Instant| {
            let mut client = Client::start("127.0.0.1:1", "Tested", &[]);
            client.samples = samples.iter().map(|s| sample(s.tick, &s.poses)).collect();
            client.arrived = Some(arrived);
            client
        };
        // 11 ticks on, halfway from the state after tick 10 to the one after 12, and as far past as
        // the time since the latest came, under 3 ms.
        let mut between = client(Instant::now());
        let drawn = between.interpolate(Tick::DT);
        assert!((1.0..1.1).contains(&x_of(drawn, a)), "{drawn:?}");
        assert_eq!(x_of(drawn, b), 7.0);
        // The latest came 10 ticks ago, so that the time is past it.
        let past = Instant::now()
            .checked_sub(Tick::DT * 10)
            .expect("an instant");
        let mut held = client(past);
        let drawn = held.interpolate(Tick::DT);
        assert_eq!((x_of(drawn, a), x_of(drawn, b)), (4.0, 9.0));
    }

    /// A correction takes the client's world back to the server's latest state, read into its copy,
    /// keeping the navmesh: the copy brings its own up to date as it reads each state, which no
    /// state holds, so that the world taken back to it holds the same resources, and no tick after
    /// bakes it again.
    #[test]
    fn a_correction_keeps_the_navmesh() {
        use crate::navigation::navmesh_of;
        use crate::{BodyType, BoxCollider3D, NavMeshVolume, RigidBody3D, step_navigation};

        let mut server = World::seeded(3);
        let floor = Transform {
            translation: Vec3::new(0.0, -0.5, 0.0),
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
        // The client's world, from the server's, as it joined.
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
        let mut client = Client::start("127.0.0.1:1", "Tested", &[]);
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
