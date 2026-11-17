//! Lockstep between two peers, per [Networking](../../../ARCHITECTURE.md#networking): a host
//! (player 0) listens, a joiner (player 1) connects and starts from the host's world and scripts.
//! A tick's inputs are every layer's commands and the edits that apply before it. Inputs go
//! [`DELAY`] ticks ahead ([`ROLLBACK_DELAY`] with `--rollback`, running up to [`PREDICTED`] ticks
//! on the other's inputs predicted empty); [`Lockstep::follow_link`] and [`Lockstep::pace`] tune
//! both. Handshakes ([`secure`]), relays ([`listen`], [`connect`]) and direct packets ([`path`])
//! are in their modules.
//!
//! The stream is lines of text over TCP without Nagle's delay, sealed in frames. One thread reads
//! and another writes, so a slow link never holds the main thread:
//!
//! ```text
//! hello\tgg-lockstep 9\tShards\t0.1.0 debug dev\t3f8a6c2e9b1d4057\tShards Inspector\t50123
//!                                                    the joiner, as it connects: its app, engine,
//!                                                      build, layers, and the port its packets
//!                                                      come from; `gg-rollback 9` if rollback
//! refused <why>                                      the host, refusing it, before it closes
//! k                                                  either, after a second with nothing else to
//!                                                      write
//! at 203.0.113.7:40312 192.168.1.5:50123             either, on a relay link: where its packets'
//!                                                      socket is as the relay saw it and as
//!                                                      bound; the other probes ([`path`])
//! world 1234 1238 2 50124                            the host's world at tick 1234, the first
//! gg-snapshot 2                                        tick it needs the joiner's inputs for, its
//! ...                                                  code 2 patches on, and its packets' port,
//! end                                                  as a snapshot file's text to `end`
//! joined 1234                                        the joiner, once it has restored it
//! c 1238 0 Variant("Move", [...])                    a command for tick 1238, of layer 0
//! e 1238 Variant("Apply", [...])                     an edit before tick 1238, or a script's
//!                                                      reload, `Variant("Script", [...])`
//! s 1238 List([String("scenes/arena.ron"), ...])     a scene file's save before tick 1238
//! p 3 1238                                           the host's third patch, before tick 1238
//! t 1238                                             the inputs for tick 1238 are whole
//! h 1234 58183b6352a09a44 fb695ce29daa8479           the world's hash after tick 1234
//! parted 1234                                        the host's world after tick 1234, where
//! stream 3f8a6c2e9b1d4057                              the hashes parted, its lines as a
//! ...                                                  snapshot's world, to `end`
//! end
//! n List([String("5f3c9a1e07b2d846"), String(...)])  the name the host gave a texture it added,
//!                                                      and its path
//! ```
//!
//! A tick's inputs come whole one way. On the stream, if they hold an edit or a patch, name a
//! texture the host has just named, or are too big for a packet. Otherwise in packets: UDP, each
//! sealed, sent each tick and again a tick's time after the last. A packet carries this peer's
//! inputs for each tick from the first the other hasn't said it has, as many as fit
//! [`MAX_PACKET_TEXT`], so a lost packet loses none. It holds the ticks' lines as text
//! ([`read_batches`]), after a byte, 1 or 0, saying whether the peer has lately had a packet direct
//! from the other, on a relay link ([`path`]):
//!
//! ```text
//! a 1234 1236 1235 3 51234 50987 4                   the tick of the host's world that last made
//!                                                      the peers' alike (a packet naming another
//!                                                      is dropped); the other's inputs whole
//!                                                      before tick 1236; this peer's next tick,
//!                                                      1235, and how far past the other's it was
//!                                                      as the other's latest packet came, or `-`;
//!                                                      its clock in ms since the link was made,
//!                                                      and the other's latest, held 4 ms, or
//!                                                      `- -`
//! c 1238 0 Variant("Move", [...])                    this peer's for tick 1238, as on the stream
//! t 1238
//! t 1239
//! ```
//!
//! Inputs for a tick that come again (from the world, or from packets sent before) are the same;
//! the first whole is kept. The other is lost on:
//!
//! - a line or packet naming a tick no run reaches, [`TICKS`] or past;
//! - the other's inputs for a tick [`AHEAD`] or more past the first it hasn't had whole;
//! - a link nothing has come by for [`LOST`];
//! - a host-taken link whose hello hasn't come within [`HELLO`].
//!
//! A hello of another protocol, app, engine, build or layers is refused, and so is a command
//! naming another player than the one whose inputs carry it ([`steers_its_own`]).
//!
//! Values are written as recordings write them, entities by ID and textures by name, and read
//! only at their tick, against the world then. The joiner names textures as its host does,
//! whatever ID files its disk holds: as the host's world gives them, and as its `n` lines name
//! those it adds after, before the inputs that may name them. A peer's ticks allocate, for inputs
//! and hash lines, as recorded ticks do; its packets reuse their text.
//!
//! [`server`] and [`client`] hold a server that owns the world and its clients, over links like a
//! peer's, each a [`Session`].

mod client;
mod desync;
mod handshake;
mod link;
mod lockstep;
mod path;
mod secure;
mod server;
mod states;

pub(crate) use client::Client;
pub(crate) use handshake::Build;
pub(crate) use lockstep::Lockstep;
pub(crate) use secure::Code;
#[cfg(test)]
pub(crate) use secure::TESTED;
/// The build of the tests' sessions ([`Build::name`]).
#[cfg(test)]
pub(crate) const BUILD: &str = "0123456789abcdef-fedcba9876543210";
pub(crate) use server::Server;

use std::fmt::Write as _;
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Write as _};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gg_core::{Error, Result};
use tracing::{error, info};

use crate::args::{Conditions, Net};
use crate::assets::Asset;
use crate::cells::Seen;
use crate::edits::{Done, Edit};
use crate::layer::Commands;
#[cfg(doc)]
use crate::recording::text;
use crate::recording::text::Text;
use crate::reflect::Value;
use crate::{Context, Sound, World};
use handshake::{Accepted, Handshaken, Looking, connect, engine, hello, listen, port_of};
use link::Link;

/// Ticks ahead a lockstep peer sends its inputs until its round trip is measured: 62.5 ms, enough
/// across a LAN, and as long from press to effect. Then the delay follows the link
/// ([`Lockstep::follow_link`]), from [`LEAST_DELAY`] to [`MOST_DELAY`]: 31.25 ms to 250 ms.
pub(crate) const DELAY: u64 = 4;
const LEAST_DELAY: u64 = 2;
const MOST_DELAY: u64 = 16;

/// Ticks ahead a rollback peer sends its inputs: 31.25 ms from press to effect. The other peer
/// predicts them for as long as they take past that.
pub(crate) const ROLLBACK_DELAY: u64 = 2;

/// The most ticks a rollback peer runs on the other's inputs predicted, its window; then it waits,
/// as a lockstep peer does. So also the most it reruns at once: 125 ms until its round trip is
/// measured, then as many as the link needs ([`Lockstep::follow_link`]), up to [`MOST_PREDICTED`],
/// 250 ms.
pub(crate) const PREDICTED: u64 = 8;
const MOST_PREDICTED: u64 = 16;

/// A delay or window falls to the most the link needed in the last 2 s, so jitter doesn't move it
/// up and down. It rises at once.
const FALLS: Duration = Duration::from_secs(2);

/// How much slower than real time a peer's ticks run per tick it's ahead of the other past the
/// first, and a client's slower or faster per tick its inputs come earlier or later than
/// [`LEAD`](client::LEAD) ticks before the server runs them. At most [`MOST_PACE`], a tenth: it
/// takes them back within seconds without overshooting much.
const PACE: f64 = 0.02;
const MOST_PACE: f64 = 0.1;

/// The most bytes a packet may have: what a path of the internet's least MTU carries whole, as
/// QUIC keeps its packets. Its sealed text ([`secure`]) may have this less the byte saying whether
/// its link hears the other end direct ([`Link::send_packet`]). A tick's inputs that don't fit go
/// on the stream.
const MAX_PACKET: usize = 1200;
const MAX_PACKET_TEXT: usize = MAX_PACKET - secure::SEALED - 1;

/// The most bytes a line may have, its line break aside; the reader drops the connection past it.
/// A scene file's save is one line, as the tool API's lines are kept to the same.
const MAX_LINE: usize = 16 << 20;

/// The most bytes of the host's world, its lines together; the joiner drops the connection past
/// it.
const MAX_WORLD: usize = 1 << 30;

/// A tick a line or packet names is below this: some 70,000 years of ticks, past any run, and far
/// enough from `u64::MAX` that ticks counted on from one can't overflow.
const TICKS: u64 = 1 << 47;

/// The most ticks past the first whose inputs a peer or server hasn't had whole that the other's
/// inputs may be for, a second's; past it the other is lost. It keeps the inputs held for later
/// few: a peer runs at most [`MOST_DELAY`] or [`MOST_PREDICTED`] ticks past the other, a client at
/// most half a second past its server's latest state.
const AHEAD: u64 = 64;

/// How long a refused joiner waits before trying again: a host of another build refuses until one
/// is rebuilt.
const REFUSED: Duration = Duration::from_secs(5);

/// A link that receives nothing, line or packet, for this long is lost: the other end went without
/// closing it. Also the longest a write may wait, as on an end that stopped reading.
const LOST: Duration = Duration::from_secs(10);

/// How long a link accepted by a host or server waits for the other's hello before it's lost, so a
/// connection that never says one doesn't hold a joiner's place.
const HELLO: Duration = Duration::from_secs(5);

/// A ticket for an offered edit or patch; its result comes back with it ([`Lockstep::done`]).
pub(crate) type Ticket = u64;

/// What applies before a tick beside the layers' commands: an edit, or in a development build a
/// scene file's save (author and text), which each peer turns into the operations taking its world
/// from the file's text as last applied.
pub(crate) enum Outside {
    Edit(Edit),
    Scene { author: String, text: String },
}

/// One peer's inputs for a tick, each value as [`text::write`] writes it: commands by layer index;
/// what applies before the tick, with the ticket of each this peer offered; and the host's patches
/// by number.
#[derive(Default)]
struct Inputs {
    commands: Vec<(usize, String)>,
    edits: Vec<(Option<Ticket>, String)>,
    scenes: Vec<(Option<Ticket>, String)>,
    patches: Vec<usize>,
    /// The other peer's: whether they've come whole.
    whole: bool,
    /// This peer's: whether they hold an edit (from outside the layers, or of a layer whose
    /// commands edit) or a patch, which apply only at a tick run on both peers' inputs; and whether
    /// they went on the stream, not in packets.
    authored: bool,
    streamed: bool,
}

impl Inputs {
    /// Whether they hold anything: the other peer's, predicted empty, are missed if so.
    fn any(&self) -> bool {
        !(self.commands.is_empty()
            && self.edits.is_empty()
            && self.scenes.is_empty()
            && self.patches.is_empty())
    }
}

/// What a tick runs on, both peers' inputs merged, the host's first: what applies before it, each
/// with its ticket if this peer offered it, and each layer's commands as text.
#[derive(Default)]
pub(crate) struct Merged {
    pub(crate) inputs: Vec<(Option<Ticket>, Outside)>,
    commands: Vec<Queued>,
    /// The patches applying before the tick, in order, with their tickets.
    #[cfg(feature = "dev")]
    pub(crate) patches: Vec<(Ticket, subsecond::JumpTable)>,
}

/// A layer's command for a tick, as text, and whose it is.
struct Queued {
    layer: usize,
    /// The player whose command it is; `None` for a server's own.
    player: Option<u8>,
    /// Whether it's a server's client's, taken by the server or predicted by the client: its text
    /// may name only loaded assets ([`unloaded`]), so a client predicts as refused what its server
    /// refuses. A lockstep peer's and a server's own commands load what they name, as each peer
    /// loads what both peers' inputs name and a replay what its recording's name.
    remote: bool,
    text: String,
}

impl Queued {
    fn of(
        commands: impl IntoIterator<Item = (usize, String)>,
        player: Option<u8>,
        remote: bool,
    ) -> impl Iterator<Item = Self> {
        commands.into_iter().map(move |(layer, text)| Self {
            layer,
            player,
            remote,
            text,
        })
    }
}

impl Merged {
    /// Empties `queue`, layer `layer`'s, of type `name`, and fills it with the layer's commands for
    /// the tick, read against `cx`'s world. A server's client's commands name assets alone: those
    /// the server hasn't loaded are refused, the rest loaded as a run that follows another loads
    /// them.
    ///
    /// A command the layer refuses, or that isn't one, is refused alone; the rest go on. Refusals
    /// are logged once per layer and tick as `net.bad_command`, with the count and the first's
    /// player and why, as a client may send thousands a packet.
    pub(crate) fn fill(
        &self,
        layer: usize,
        name: &str,
        queue: &mut dyn Commands,
        cx: &mut Context,
    ) {
        queue.clear();
        let mut refused = 0;
        let mut first = None;
        let commands = self.commands.iter().filter(|queued| queued.layer == layer);
        for queued in commands {
            let pushed = Text::parse(&queued.text, 1).and_then(|text| {
                if queued.remote {
                    unloaded(&text, cx.world())?;
                } else {
                    cx.load_followed(&text);
                }
                let command = text.value(cx.world());
                if let Some(player) = queued.player {
                    steers_its_own(&command, player)?;
                }
                queue.push(&command)
            });
            if let Err(error) = pushed {
                refused += 1;
                first.get_or_insert((queued.player, error));
            }
        }
        if let Some((player, error)) = first {
            let whose = match player {
                Some(player) => format!("player {player}'s"),
                None => "the server's own".to_owned(),
            };
            error!(
                code = "net.bad_command",
                layer = name,
                player,
                refused,
                "Refused {refused} commands for layer {name}, the first {whose}: {error:#}"
            );
        }
    }
}

/// Fails with `net.bad_command` if `command`, from player `player`'s inputs, names another player:
/// a variant or struct whose field `player` holds an integer, as Shards' commands name the player
/// they steer. Layers queue commands for their own player alone, and peers refuse alike. A
/// server's own commands, which steer any player, aren't checked.
fn steers_its_own(command: &Value, player: u8) -> Result<()> {
    let (Value::Variant(_, fields) | Value::Struct(fields)) = command else {
        return Ok(());
    };
    let named = fields.iter().find(|(name, _)| name == "player");
    let named = match named.map(|(_, value)| value) {
        Some(&Value::UInt(named)) => i128::from(named),
        Some(&Value::Int(named)) => i128::from(named),
        _ => return Ok(()),
    };
    if named == i128::from(player) {
        return Ok(());
    }
    let message = format!(
        "it names player {named}, and came in player {player}'s inputs: a player's commands steer \
         that player alone"
    );
    Err(Error::new("net.bad_command", message))
}

/// Fails with `net.bad_command` if `text`, a command from a server's client or its own as it
/// predicts it, names a texture, model, font or prefab `world` hasn't loaded. A client names only
/// what its server has, as its state does, so it can't make the server load or stand in for what
/// it names. A name the server would find in the game but hasn't loaded is refused too: once
/// loaded, the server's states would name it, and a client that hasn't can't read them.
fn unloaded(text: &Text, world: &World) -> Result<()> {
    let mut names = Vec::new();
    text.assets(&mut names);
    let assets = world.assets();
    let Some(unloaded) = names.iter().find(|named| !assets.has(named)) else {
        return Ok(());
    };
    let kind = match unloaded {
        Asset::Texture(_) => "texture",
        Asset::Model(_) => "model",
        Asset::Font(_) => "font",
        Asset::Prefab(_) => "prefab",
    };
    let message = format!(
        "it names {kind} `{}`, which the server hasn't loaded, and a client's commands name only \
         those it has",
        unloaded.name()
    );
    Err(Error::new("net.bad_command", message))
}

/// A run's stalls: runs of frames that waited, ticks owed, for what the next tick needs from the
/// link's other end (a lockstep peer's inputs, a client's server states past its window): how many,
/// their total and longest time, which `net.delay` and `net.corrected` log; and when the current
/// one started.
#[derive(Default)]
struct Stalls {
    count: u64,
    total: Duration,
    longest: Duration,
    since: Option<Instant>,
}

impl Stalls {
    /// Notes a tick waited at `now`; starts a stall unless one has.
    fn waited(&mut self, now: Instant) {
        self.since.get_or_insert(now);
    }

    /// Ends the stall going on at `now`, as a tick runs or the link is lost.
    fn end(&mut self, now: Instant) {
        if let Some(since) = self.since.take() {
            let stall = now.saturating_duration_since(since);
            self.count += 1;
            self.total += stall;
            self.longest = self.longest.max(stall);
        }
    }

    /// Count, total and longest, in milliseconds.
    fn logged(&self) -> (u64, u64, u64) {
        let millis = |time: Duration| time.as_millis() as u64;
        (self.count, millis(self.total), millis(self.longest))
    }
}

/// What [`Lockstep::turn`], [`Server::turn`] and [`Client::turn`] ask for between ticks.
pub(crate) enum Turn {
    None,
    /// The host owes its joiner its world, or the server a client its own.
    Send,
    /// The joiner has its host's world, or the client its server's: its tick and text.
    Restore((u64, String)),
}

/// A run's session as a game's UI shows it ([`Context::session`]): code, player and phase.
#[derive(Clone)]
pub struct SessionStatus {
    code: Arc<str>,
    player: u8,
    phase: SessionPhase,
}

impl SessionStatus {
    /// A status as a run's would be: for tests, via [`Context::set_session`].
    #[doc(hidden)]
    pub fn new(code: &str, player: u8, phase: SessionPhase) -> Self {
        Self {
            code: code.into(),
            player,
            phase,
        }
    }

    /// The session's code in fours, as `7KQ2-9XMB-4TZD-P0WH`: for the host's UI to show, for the
    /// player to give the joiner. It's the session's key: the engine logs it nowhere, nor writes
    /// it in this type's `Debug`.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The run's player: 0 for the host, 1 for the joiner.
    pub fn player(&self) -> u8 {
        self.player
    }

    /// Where the session stands for this run.
    pub fn phase(&self) -> &SessionPhase {
        &self.phase
    }
}

impl std::fmt::Debug for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionStatus")
            .field("player", &self.player)
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

/// Where a session stands for a run ([`SessionStatus::phase`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    /// The host's, with no one joined, or the joiner lost: it plays on alone.
    Waiting,
    /// The joiner's, while no run hosts its code at the relay: it asks again each second.
    Unhosted,
    /// The joiner's, connected or connecting, until the host's world takes its own's place.
    Joining,
    /// Both runs play.
    Playing,
    /// The joiner's, refused by its host, as a run of another build is, saying why: it retries in
    /// 5 s.
    Refused { why: Arc<str> },
    /// The relay can't be reached, or refused or lost the session's room, saying why: the run
    /// retries, the host every 5 s and the joiner every second.
    Unreachable { why: Arc<str> },
    /// The joiner's, whose host is lost: frozen, it reconnects, and the host's world takes its
    /// place again once it does.
    Lost,
}

/// The networked session a run takes part in, as `--host`, `--join`, `--serve` or `--connect` sets
/// it: a lockstep peer, a server that owns the world, or a client. Boxed, as their sizes differ.
pub(crate) enum Session {
    Peer(Box<Lockstep>),
    Server(Box<Server>),
    Client(Box<Client>),
}

impl Session {
    /// Starts the session `net` says, keyed by `code`, for app `app` of build `build` with `layers`
    /// bottom up ([`Lockstep::start`], [`Server::start`], [`Client::start`]). A host or server
    /// given no code makes one and prints it to stdout, `code 7KQ2-9XMB-4TZD-P0WH`, for the
    /// joining runs. The log says only that it made one (`net.code`), as it's the session's key.
    ///
    /// Fails with `net.build` if the run's build can't be read ([`Build::name`]), and with
    /// `net.start` if a host or server can't listen at its address or make or print its code, or
    /// a joiner or client is given none. An unreachable relay isn't a failure: a host or server
    /// registers once it can, a joiner or client comes for its room once it can ([`listen`],
    /// [`connect`]).
    pub(crate) fn start(
        net: &Net,
        code: Option<Code>,
        app: &'static str,
        build: &Build,
        layers: &[&str],
    ) -> Result<Self> {
        let build = build.name()?;
        let code = match code {
            Some(code) => code,
            None if matches!(net, Net::Host(_) | Net::Serve(_)) => {
                let failed = |why: &str, error: io::Error| {
                    Error::new("net.start", format!("can't {why} the session's code"))
                        .with_source(error)
                };
                let code = Code::make().map_err(|error| failed("make", error))?;
                writeln!(io::stdout(), "code {code}").map_err(|error| failed("print", error))?;
                info!(
                    code = "net.code",
                    "Made the session's code, which this run printed to stdout, for the runs that \
                     join it to be given"
                );
                code
            }
            None => {
                let why = "a joiner or a client is given the session's code, as its host made it";
                return Err(Error::new("net.start", why));
            }
        };
        Ok(match net {
            Net::Host(_) | Net::Join(_) => {
                Self::Peer(Box::new(Lockstep::start(net, code, app, &build, layers)?))
            }
            Net::Serve(at) => Self::Server(Box::new(Server::start(at, code, app, &build, layers)?)),
            Net::Connect(toward) => {
                Self::Client(Box::new(Client::start(toward, code, app, &build, layers)))
            }
        })
    }

    /// Starts the session as [`start`](Self::start), its peers predicting if `rollback`
    /// ([`predict`](Self::predict)) and its links conditioned by `link`
    /// ([`condition`](Self::condition)).
    ///
    /// Fails as `start` does.
    pub(crate) fn begin(
        net: &Net,
        code: Option<Code>,
        app: &'static str,
        build: &Build,
        layers: &[&str],
        rollback: bool,
        link: Option<Conditions>,
    ) -> Result<Self> {
        let mut session = Self::start(net, code, app, build, layers)?;
        if rollback {
            session.predict();
        }
        if let Some(link) = link {
            session.condition(link);
        }
        Ok(session)
    }

    pub(crate) fn predict(&mut self) {
        if let Self::Peer(net) = self {
            net.predict();
        }
    }

    pub(crate) fn condition(&mut self, conditions: Conditions) {
        let random = RandomState::new().hash_one(0);
        match self {
            Self::Peer(net) => net.condition(conditions),
            Self::Server(server) => (server.conditions, server.random) = (Some(conditions), random),
            Self::Client(client) => (client.conditions, client.random) = (Some(conditions), random),
        }
    }

    /// The session as a game's UI shows it: a peer's; `None` for a server or client, which a
    /// game's UI doesn't start.
    pub(crate) fn status(&self) -> Option<SessionStatus> {
        match self {
            Self::Peer(net) => Some(SessionStatus {
                code: net.code.clone(),
                player: net.player(),
                phase: net.phase(),
            }),
            Self::Server(_) | Self::Client(_) => None,
        }
    }

    pub(crate) fn app(&self) -> &'static str {
        match self {
            Self::Peer(net) => net.app(),
            Self::Server(server) => server.app,
            Self::Client(client) => client.app,
        }
    }

    pub(crate) fn player(&self) -> Option<u8> {
        match self {
            Self::Peer(net) => Some(net.player()),
            Self::Server(_) => None,
            Self::Client(client) => client.player,
        }
    }

    /// Whether the run follows another's world (a joiner's, a client's): it takes its own's place.
    pub(crate) fn follows(&self) -> bool {
        match self {
            Self::Peer(net) => !net.hosts(),
            Self::Server(_) => false,
            Self::Client(_) => true,
        }
    }

    pub(crate) fn ready(&self, tick: u64) -> bool {
        match self {
            Self::Peer(net) => net.ready(tick),
            Self::Server(_) => true,
            Self::Client(client) => client.ready(tick),
        }
    }

    pub(crate) fn waited(&mut self, tick: u64) {
        match self {
            Self::Peer(net) => net.waited(tick),
            Self::Server(_) => {}
            Self::Client(client) => client.waited(),
        }
    }

    /// The time a frame `dt` after the last owes the run's ticks; a server's is `dt`.
    pub(crate) fn pace(&mut self, dt: Duration) -> Duration {
        match self {
            Self::Peer(net) => net.pace(dt),
            Self::Server(_) => dt,
            Self::Client(client) => client.pace(dt),
        }
    }

    pub(crate) fn start_tick<'q>(
        &mut self,
        tick: u64,
        queues: impl Iterator<Item = (usize, &'q mut dyn Commands)>,
        world: &World,
    ) {
        match self {
            Self::Peer(net) => net.start_tick(tick, queues, world),
            Self::Server(server) => server.start_tick(queues, world),
            Self::Client(client) => client.start_tick(tick, queues, world),
        }
    }

    /// What tick `tick` runs on. A peer keeps a world copy ([`Lockstep::keep`]) as a tick on
    /// predicted inputs starts.
    pub(crate) fn inputs(&mut self, tick: u64, cx: &mut Context) -> Merged {
        match self {
            Self::Peer(net) => {
                if !net.settled(tick) {
                    net.keep(tick, cx.world());
                }
                net.inputs(tick, cx)
            }
            Self::Server(server) => server.inputs(tick),
            Self::Client(client) => client.inputs(tick),
        }
    }

    /// Keeps the result of the edit offered with `ticket`, applied before `tick`. Only a peer's
    /// ticks apply edits.
    pub(crate) fn did(&mut self, ticket: Ticket, tick: u64, done: Result<Done>) {
        if let Self::Peer(net) = self {
            net.did(ticket, tick, done);
        }
    }

    /// The sounds the runs of tick `tick` played, if it may run again: a rollback peer's tick on
    /// predicted inputs, or a client's, which a server's state may correct.
    pub(crate) fn played(&mut self, tick: u64) -> Option<&mut Vec<Sound>> {
        match self {
            Self::Peer(net) => net.played(tick),
            Self::Server(_) => None,
            Self::Client(client) => Some(client.played.entry(tick).or_default()),
        }
    }

    /// Once tick `tick` has run: a peer ends it only if it ran on both peers' inputs.
    pub(crate) fn ran(&mut self, tick: u64, world: &mut World) {
        match self {
            Self::Peer(net) if net.settled(tick) => net.end_tick(tick, world),
            Self::Peer(_) => {}
            Self::Server(server) => server.end_tick(tick, world),
            Self::Client(client) => client.end_tick(tick, world),
        }
    }

    pub(crate) fn turn(&mut self, next: u64) -> Turn {
        match self {
            Self::Peer(net) => net.turn(next),
            Self::Server(server) => server.turn(next),
            Self::Client(client) => client.turn(),
        }
    }

    /// Sends `world` at tick `next` to the joiner or clients owed it, as `write` writes it, a
    /// snapshot file's text: whole for a joiner, of what each sees for a client. `write` gives
    /// nothing for a world that can't be written, which stays owed.
    pub(crate) fn send_world(
        &mut self,
        world: &World,
        write: &mut dyn FnMut(Option<&Seen>) -> Option<String>,
        next: u64,
    ) {
        match self {
            Self::Peer(net) => {
                if let Some(text) = write(None) {
                    net.send_world(&text, world.assets().counts(), next);
                }
            }
            Self::Server(server) => server.send_world(world, write, next),
            Self::Client(_) => {}
        }
    }

    /// Says the world of a joiner's host or a client's server is in place; returns the tick to go
    /// on from.
    pub(crate) fn restored(&mut self, tick: u64, world: &World) -> u64 {
        match self {
            Self::Peer(net) => {
                net.restored(tick);
                tick
            }
            Self::Server(_) => tick,
            Self::Client(client) => client.restored(tick, world),
        }
    }

    pub(crate) fn lose(&mut self, why: &str) {
        match self {
            Self::Peer(net) => net.lose(why),
            Self::Server(_) => {}
            Self::Client(client) => client.lose(why),
        }
    }

    /// The first tick to run again from, if any, whose world [`start_of`](Self::start_of) gives.
    pub(crate) fn settle(&mut self, next: u64, world: &World) -> Option<u64> {
        match self {
            Self::Peer(net) => net.settle(next, world),
            Self::Server(_) => None,
            Self::Client(client) => client.settle(next, world),
        }
    }

    pub(crate) fn start_of(&self, tick: u64) -> &World {
        match self {
            Self::Peer(net) => net.start_of(tick),
            Self::Server(_) => unreachable!("a server runs no tick again"),
            Self::Client(client) => client.start_of(tick),
        }
    }

    pub(crate) fn rolled_back(&mut self, from: u64, next: u64) {
        match self {
            Self::Peer(net) => net.rolled_back(from, next),
            Self::Server(_) => {}
            Self::Client(client) => client.rolled_back(from, next),
        }
    }

    pub(crate) fn ended(&self) {
        match self {
            Self::Peer(net) => net.ended(),
            Self::Server(server) => server.ended(),
            Self::Client(client) => client.ended(),
        }
    }
}

/// The tick `text` names; `None` unless it's a number below [`TICKS`].
fn tick_of(text: &str) -> Option<u64> {
    text.parse().ok().filter(|&tick| tick < TICKS)
}

/// The number `text` is, or none if `-` (a packet's header number not yet had); `None` if neither.
fn optional<T: std::str::FromStr>(text: &str) -> Option<Option<T>> {
    if text == "-" {
        Some(None)
    } else {
        text.parse().ok().map(Some)
    }
}

/// Reads a packet's inputs, `lines`: each tick's `c` lines then the `t` saying they're whole, which
/// gives the tick and its commands, with their layers' indices, to `each`. `None` at a line that
/// isn't one, those before given.
fn read_batches(
    lines: std::str::Lines<'_>,
    mut each: impl FnMut(u64, Vec<(usize, String)>),
) -> Option<()> {
    let mut commands = Vec::new();
    for line in lines {
        let (kind, rest) = line.split_once(' ')?;
        let mut parts = rest.splitn(3, ' ');
        let tick = tick_of(parts.next()?)?;
        match kind {
            "c" => {
                let layer = parts.next()?.parse::<usize>().ok()?;
                commands.push((tick, layer, parts.next()?.to_owned()));
            }
            "t" if parts.next().is_none() => {
                if commands.iter().any(|&(of, ..)| of != tick) {
                    return None;
                }
                each(
                    tick,
                    commands
                        .drain(..)
                        .map(|(_, layer, value)| (layer, value))
                        .collect(),
                );
            }
            _ => return None,
        }
    }
    Some(())
}

fn write_inputs(out: &mut String, tick: u64, inputs: &Inputs) {
    let written = "a string takes any text";
    for (layer, command) in &inputs.commands {
        writeln!(out, "c {tick} {layer} {command}").expect(written);
    }
    for (_, edit) in &inputs.edits {
        writeln!(out, "e {tick} {edit}").expect(written);
    }
    for (_, scene) in &inputs.scenes {
        writeln!(out, "s {tick} {scene}").expect(written);
    }
    for number in &inputs.patches {
        writeln!(out, "p {number} {tick}").expect(written);
    }
    writeln!(out, "t {tick}").expect(written);
}

#[cfg(test)]
mod tests;
