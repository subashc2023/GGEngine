//! Lockstep between two peers, as [Networking](../../../ARCHITECTURE.md#networking) describes: a
//! host, player 0, that listens, and a joiner, player 1, that connects and starts from the host's
//! world and scripts. A tick's inputs are every layer's commands and the edits that apply before
//! it, a script's reload among them.
//!
//! - Lockstep: a peer sends its inputs [`DELAY`] ticks ahead until the link is timed, then as far
//!   ahead as the link needs, and runs a tick once both peers' inputs are in.
//! - Rollback (`--rollback`): a peer sends its inputs [`ROLLBACK_DELAY`] ahead and runs up to its
//!   window, from [`PREDICTED`] ticks, on the other's predicted to hold nothing. From the first
//!   tick whose inputs held something, it takes the world back and runs the ticks again.
//!   [`Lockstep::follow_link`] says how the delay and the window follow the link.
//! - [`Lockstep::pace`] keeps either peer from running far ahead of the other.
//! - The peers compare the world's hash after each tick run on both peers' inputs, each keeping a
//!   copy of its world until then. Where they part, the host sends its world after that tick, and
//!   the joiner names the field where its own differs.
//! - In development builds, the patches of the game's code apply at a tick the host chooses.
//!
//! The session's code (`--code`) keys each connection's handshake, as [`secure`] says. The end
//! that took the connection refuses a run of another code there. The keys the handshake makes seal
//! the stream's frames and the packets, so nothing between the runs can be read, forged or played
//! again on the way.
//!
//! Runs that can't connect to each other, as two behind their own routers can't, meet through a
//! relay (`--relay`), which `gg-relay` runs. The host registers the room the code gives, and its
//! joiner comes for it, as [`listen`] and [`connect`] say. The relay copies the stream each way and
//! forwards the packets, which it can't read or forge; packets go direct where the routers let
//! them, as [`path`] says.
//!
//! The session's stream is lines of text over TCP, without Nagle's delay, sealed in frames. A
//! thread of the connection's reads them, and another writes what the main thread sends, so a slow
//! link never holds the main thread:
//!
//! ```text
//! hello\tgg-lockstep 9\tShards\t0.1.0 debug dev\t3f8a6c2e9b1d4057\tShards Inspector\t50123
//!                                                    the joiner, as it connects: its app, engine,
//!                                                      build and layers, and the port its packets
//!                                                      come from, as `gg-rollback 9` for a
//!                                                      rollback session's
//! refused <why>                                      the host, refusing it, before it closes
//! k                                                  either, after a second with nothing else to
//!                                                      write, to say it's there
//! at 203.0.113.7:40312 192.168.1.5:50123             either, on a link through a relay: where its
//!                                                      packets' socket is, as the relay saw it
//!                                                      and as it's bound, which the other probes,
//!                                                      as [`path`] says
//! world 1234 1238 2 50124                            the host's world, at tick 1234, the first
//! gg-snapshot 2                                        tick it needs the joiner's inputs for, its
//! ...                                                  code 2 patches on, and the port its packets
//! end                                                  come from, as a snapshot file's text, to
//!                                                      its `end`
//! joined 1234                                        the joiner, once it has restored it
//! c 1238 0 Variant("Move", [...])                    a command for tick 1238, of layer 0
//! e 1238 Variant("Apply", [...])                     an edit before tick 1238, or a script's
//!                                                      reload, `Variant("Script", [...])`
//! s 1238 List([String("scenes/arena.ron"), ...])     a scene file's save before tick 1238
//! p 3 1238                                           the host's third patch, before tick 1238
//! t 1238                                             the inputs for tick 1238 are whole
//! h 1234 58183b6352a09a44 fb695ce29daa8479           the world's hash after tick 1234
//! parted 1234                                        the host's world after tick 1234, where
//! stream 3f8a6c2e9b1d4057                              the peers' hashes parted, its lines as a
//! ...                                                  snapshot file's world's, to its `end`
//! end
//! n List([String("5f3c9a1e07b2d846"), String(...)])  the name the host gave a texture it added,
//!                                                      and the path it names
//! ```
//!
//! A tick's inputs come whole one way: on the stream, if they hold an edit or a patch, name a
//! texture the host has just named, or are too many bytes for a packet; otherwise in the packets,
//! UDP's, each sealed, sent at each tick and again a tick's time after the last, which carry this
//! peer's inputs for each tick from the first the other hasn't said it has, as many as fit
//! [`MAX_PACKET_TEXT`], so that a packet lost loses none, the ticks' lines, as their text, as
//! [`read_batches`] reads them, after a byte, 1 or 0, that says whether the peer has had a packet
//! direct from the other lately, on a link through a relay, as [`path`] says:
//!
//! ```text
//! a 1234 1236 1235 3 51234 50987 4                   the tick the peers' worlds were last made
//!                                                      the same at, the host's world's, a packet
//!                                                      of another dropped; the other's inputs
//!                                                      whole before tick 1236; this peer's next
//!                                                      tick, 1235, and how many it was past the
//!                                                      other's as the other's latest packet came,
//!                                                      or `-`; its clock, in milliseconds since
//!                                                      the link was made, and the other's latest,
//!                                                      held 4 ms, or `- -`
//! c 1238 0 Variant("Move", [...])                    this peer's for tick 1238, as on the stream
//! t 1238
//! t 1239
//! ```
//!
//! A tick's inputs that come again, as those the world gave and the packets sent before it, are
//! the same, and the first whole is kept. A line or a packet that names a tick no run reaches,
//! [`TICKS`] or past, or the other's inputs for a tick [`AHEAD`] or more past the first it hasn't
//! had whole, loses the other, as inputs that far past a server's next tick lose their client; as
//! does a link nothing has come by for [`LOST`], and one the host took whose hello hasn't come
//! within [`HELLO`]. A hello of another protocol, app, engine, build or layers is refused, and a
//! command naming another player than the one whose inputs carry it, as [`steers_its_own`] says.
//! Values are written as recordings write them, entities by their IDs and textures by their names,
//! and read into values only at their tick, against the world then. The joiner names its textures
//! as its host does, whatever ID files its own disk holds: as its host's world gives them, and as
//! the host's `n` lines give the names of those it adds after, before the inputs that may name
//! them. A peer's ticks allocate, for their inputs' text, the hash's lines and the packets, as
//! recorded ticks do.
//!
//! A server that owns the world, and its clients, are [`server`]'s and [`client`]'s, over links as
//! a session's peers have, a [`Session`] each as a peer's is.

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
/// The build the tests' sessions are of, as [`Build::name`] names one.
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

/// How many ticks ahead a lockstep peer sends its inputs until its link's round trip is measured:
/// 62.5 ms, so that they reach the other peer before it needs them across a LAN, at the cost of as
/// long between a press and what it does. Then its delay follows the link, as
/// [`Lockstep::follow_link`] says, from [`LEAST_DELAY`] to [`MOST_DELAY`], 31.25 ms to 250 ms.
pub(crate) const DELAY: u64 = 4;
const LEAST_DELAY: u64 = 2;
const MOST_DELAY: u64 = 16;

/// How many ticks ahead a rollback peer sends its inputs: 31.25 ms between a press and what it
/// does, the other peer predicting them for as long as they take past that.
pub(crate) const ROLLBACK_DELAY: u64 = 2;

/// The most ticks a rollback peer runs on the other's inputs predicted, its window, after which it
/// waits for them, as a lockstep peer waits for each; and so the most it runs again at once: 125 ms
/// of them until its link's round trip is measured, and then as many as the link needs, as
/// [`Lockstep::follow_link`] says, up to [`MOST_PREDICTED`], 250 ms.
pub(crate) const PREDICTED: u64 = 8;
const MOST_PREDICTED: u64 = 16;

/// How long what a link needed counts: a delay or window falls to the most it needed in the last
/// 2 s, so that a link's jitter doesn't have it go up and down; it rises at once.
const FALLS: Duration = Duration::from_secs(2);

/// How much slower than real time a peer's ticks run for each tick it's ahead of the other past
/// the first, and a client's slower or faster for each its inputs come earlier or later than
/// [`LEAD`](client::LEAD) ticks before the server runs them; and the most, a tenth, which takes
/// them back within seconds without overshooting much.
const PACE: f64 = 0.02;
const MOST_PACE: f64 = 0.1;

/// The most bytes a packet may have: what a path of the internet's least MTU carries whole, as
/// QUIC's packets are kept to; and so the most its text may have, sealed, as [`secure`] says, after
/// the byte that says whether its link hears the other end direct, as [`Link::send_packet`] says.
/// A tick's inputs that wouldn't fit go on the stream.
const MAX_PACKET: usize = 1200;
const MAX_PACKET_TEXT: usize = MAX_PACKET - secure::SEALED - 1;

/// The most bytes a line may have, its line break aside, past which the peer reading it drops the
/// connection: a scene file's save among them, whose text is one line, as the tool API's lines are
/// kept to the same.
const MAX_LINE: usize = 16 << 20;

/// The most bytes the host's world may have as it comes, its lines together, past which the joiner
/// drops the connection.
const MAX_WORLD: usize = 1 << 30;

/// The ticks a line or a packet may name are fewer than this: some 70,000 years of them, past any
/// run, and far enough from `u64::MAX` that the ticks counted on from one can't overflow.
const TICKS: u64 = 1 << 47;

/// The most ticks past the first whose inputs a peer, or a server, hasn't had whole from the
/// other, that the other's inputs may be for, a second's, past which the other is lost, so that
/// the inputs kept for later are few: a peer runs no more than [`MOST_DELAY`] or
/// [`MOST_PREDICTED`] ticks past the other, and a client no more than half a second past its
/// server's latest state.
const AHEAD: u64 = 64;

/// How long a joiner its host refused waits before trying it again, as a host of another build
/// refuses it until one of them is rebuilt.
const REFUSED: Duration = Duration::from_secs(5);

/// How long a link goes with nothing coming by it, line or packet, before it's lost, as the other
/// end has gone without closing, as one cut off from the network or stopped in a debugger has; and
/// how long a write to it may wait, as on one that has stopped reading.
const LOST: Duration = Duration::from_secs(10);

/// How long a link accepted, by a host or a server, waits for the other end's hello, before it's
/// lost, so that a connection that never says one doesn't hold a joiner's place.
const HELLO: Duration = Duration::from_secs(5);

/// A ticket for an edit or a patch a peer offered, which its result comes back with, as
/// [`Lockstep::done`] gives it.
pub(crate) type Ticket = u64;

/// What applies before a tick, beside the layers' commands: an edit, an author's, or, in a
/// development build, a scene file's save, its author and its text, which each peer turns into the
/// operations that take its world from the file's text as last applied.
pub(crate) enum Outside {
    Edit(Edit),
    Scene { author: String, text: String },
}

/// One peer's inputs for a tick, each value as [`text::write`] writes it: each layer's commands, by
/// the layer's index; what applies before the tick, with the ticket of each this peer offered; and
/// the host's patches that apply before it, by their numbers.
#[derive(Default)]
struct Inputs {
    commands: Vec<(usize, String)>,
    edits: Vec<(Option<Ticket>, String)>,
    scenes: Vec<(Option<Ticket>, String)>,
    patches: Vec<usize>,
    /// The other peer's: whether they've come whole.
    whole: bool,
    /// This peer's: whether they hold an edit, from outside the layers or of a layer whose
    /// commands edit, or a patch, which apply only at a tick run on both peers' inputs; and whether
    /// they went on the stream, rather than in the packets.
    authored: bool,
    streamed: bool,
}

impl Inputs {
    /// Whether they hold anything: the other peer's, predicted to hold nothing, missed if they do.
    fn any(&self) -> bool {
        !(self.commands.is_empty()
            && self.edits.is_empty()
            && self.scenes.is_empty()
            && self.patches.is_empty())
    }
}

/// What a tick runs on, both peers' inputs merged, the host's first: what applies before it, each
/// with its ticket if this peer offered it, and each layer's commands, as text.
#[derive(Default)]
pub(crate) struct Merged {
    pub(crate) inputs: Vec<(Option<Ticket>, Outside)>,
    commands: Vec<Queued>,
    /// The patches that apply before the tick, in order, with their tickets.
    #[cfg(feature = "dev")]
    pub(crate) patches: Vec<(Ticket, subsecond::JumpTable)>,
}

/// A layer's command for a tick, as text, and whose it is.
struct Queued {
    layer: usize,
    /// The player whose command it is, `None` for a server's own.
    player: Option<u8>,
    /// Whether it's a server's client's, which the server took or the client predicts: its text may
    /// name only the assets loaded, as [`unloaded`] says, so that a client predicts what its server
    /// refuses as refused. A lockstep peer's and a server's own name assets as a run that follows
    /// another does, loading what they name, as each peer loads alike what both peers' inputs name,
    /// and a replay what its recording's name.
    remote: bool,
    text: String,
}

impl Queued {
    /// The commands `commands` of player `player`, each with its layer's index, `remote` as
    /// [`Queued::remote`] says.
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
    /// Empties `queue`, layer `layer`'s, of the type `name` names, and fills it with the layer's
    /// commands for the tick, read against `cx`'s world: a client's of a server's as names alone,
    /// those it names that the server hasn't loaded refused, and the others once the assets they
    /// name are loaded, as a run that follows another loads them.
    ///
    /// A command the layer refuses, or that isn't one, is refused alone, and the commands after it
    /// go on; those refused are logged once a layer and tick, as `net.bad_command`, with how many
    /// and the first's player and why, as a client may send thousands a packet.
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

/// Fails with `net.bad_command` if `command`, of player `player`'s inputs, names another player:
/// a variant or a struct whose field `player` holds an integer, as Shards' commands name the
/// player they steer, and a run's layers queue commands for its own player alone. So no client of a
/// server, nor either peer of a session, steers another's player; peers refuse alike, as each knows
/// whose inputs are whose. A server's own commands, which steer any player, aren't checked.
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

/// Fails with `net.bad_command` if `text`, a command a server's client sent, or its own as it
/// predicts it, names a texture, model, font or prefab that `world` hasn't loaded: a server's
/// client names only those its server has, as its state does, so that it can't have its server
/// load or stand in for whatever it names. A name the server would find in the game, but hasn't
/// loaded, is refused too: once loaded, the server's states would name it, and a client that
/// hasn't loaded it can't read them.
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

/// A run's stalls: the runs of its frames that waited, ticks owed, for what the next tick needs
/// from the other end of its link, as a lockstep peer's do for the other's inputs, and a client's
/// past its window for the server's states: how many, their time together and the longest, which
/// `net.delay` and `net.corrected` log; and when the one going on started.
#[derive(Default)]
struct Stalls {
    count: u64,
    total: Duration,
    longest: Duration,
    since: Option<Instant>,
}

impl Stalls {
    /// Notes that a tick waited at `now`, which starts a stall, unless one has.
    fn waited(&mut self, now: Instant) {
        self.since.get_or_insert(now);
    }

    /// Ends the stall going on at `now`, if one is: as a tick runs, or the link is lost.
    fn end(&mut self, now: Instant) {
        if let Some(since) = self.since.take() {
            let stall = now.saturating_duration_since(since);
            self.count += 1;
            self.total += stall;
            self.longest = self.longest.max(stall);
        }
    }

    /// How many, and their time together and the longest, in milliseconds.
    fn logged(&self) -> (u64, u64, u64) {
        let millis = |time: Duration| time.as_millis() as u64;
        (self.count, millis(self.total), millis(self.longest))
    }
}

/// What a peer, a server or a client must do between ticks, as [`Lockstep::turn`],
/// [`Server::turn`] and [`Client::turn`] say.
pub(crate) enum Turn {
    None,
    /// The host owes its joiner its world, or the server a client its own.
    Send,
    /// The joiner has its host's world, or the client its server's: the tick it's at and its text.
    Restore((u64, String)),
}

/// A run's session as a game's own UI shows it, as [`Context::session`] gives it: its code, the
/// run's player and where it stands.
#[derive(Clone)]
pub struct SessionStatus {
    code: Arc<str>,
    player: u8,
    phase: SessionPhase,
}

impl SessionStatus {
    /// A session's status, as a run's would be: for tests, through
    /// [`Context::set_session`].
    #[doc(hidden)]
    pub fn new(code: &str, player: u8, phase: SessionPhase) -> Self {
        Self {
            code: code.into(),
            player,
            phase,
        }
    }

    /// The session's code, written in fours, as `7KQ2-9XMB-4TZD-P0WH`: for the host's UI to show,
    /// for the player to give the one who joins. It's the session's key, which the engine logs
    /// nowhere, nor writes in this type's `Debug`.
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

/// Where a session stands for a run, as [`SessionStatus::phase`] says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    /// The host's, with no one joined, or the one who joined lost: it plays on alone.
    Waiting,
    /// The joiner's, while no run hosts its code at the relay: it asks again each second.
    Unhosted,
    /// The joiner's, connected or connecting, until the host's world takes its own's place.
    Joining,
    /// Both runs play.
    Playing,
    /// The joiner's, refused by its host, as a run of another build is, saying why: it tries again
    /// in 5 s.
    Refused { why: Arc<str> },
    /// The relay can't be reached, refused the session's room or lost it, saying why: the run
    /// tries again, the host every 5 s and the joiner every second.
    Unreachable { why: Arc<str> },
    /// The joiner's, whose host is lost: frozen, it connects again, and the host's world takes its
    /// place again once it does.
    Lost,
}

/// The networked session a run takes part in, as `--host`, `--join`, `--serve` or `--connect` has
/// it: a peer of a lockstep session, a server that owns the world, or a client of a server; each
/// boxed, as their sizes are far apart, and a run has one.
pub(crate) enum Session {
    Peer(Box<Lockstep>),
    Server(Box<Server>),
    Client(Box<Client>),
}

impl Session {
    /// Starts the session `net` says, keyed by `code`, for app `app` of build `build`, whose layers
    /// are `layers`, from the bottom up, as [`Lockstep::start`], [`Server::start`] and
    /// [`Client::start`] say. A host or a server given no code makes one, which it prints to
    /// stdout, `code 7KQ2-9XMB-4TZD-P0WH`, for the runs that join it, and not to the log, as it's
    /// the session's key, which the log says only it made (`net.code`).
    ///
    /// Fails with `net.build` if the run's build can't be read, as [`Build::name`] says, and with
    /// `net.start` if a host or a server can't listen at its address, or make or print its code,
    /// or a joiner or a client is given none. A relay that can't be reached isn't a failure: a host
    /// or a server registers at it once it can, and a joiner or a client comes for its room once
    /// it can, as [`listen`] and [`connect`] say.
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

    /// Starts the session `net` says, as [`start`](Self::start) does, its peers predicting each
    /// other's inputs if `rollback` says so, as [`predict`](Self::predict) has them, and its links
    /// doing to what comes as `link` says, as [`condition`](Self::condition) has them.
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

    /// Has a peer predict the other's inputs, as [`Lockstep::predict`] says: before its first turn.
    pub(crate) fn predict(&mut self) {
        if let Self::Peer(net) = self {
            net.predict();
        }
    }

    /// Has the session's links do to what comes as `conditions` say, as `--link` asks.
    pub(crate) fn condition(&mut self, conditions: Conditions) {
        let random = RandomState::new().hash_one(0);
        match self {
            Self::Peer(net) => net.condition(conditions),
            Self::Server(server) => (server.conditions, server.random) = (Some(conditions), random),
            Self::Client(client) => (client.conditions, client.random) = (Some(conditions), random),
        }
    }

    /// The session as a game's own UI shows it, as [`SessionStatus`] says: a peer's; `None` for a
    /// server or a client, whose sessions a game's own UI doesn't start.
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

    /// The app the session is of.
    pub(crate) fn app(&self) -> &'static str {
        match self {
            Self::Peer(net) => net.app(),
            Self::Server(server) => server.app,
            Self::Client(client) => client.app,
        }
    }

    /// The player the run plays: a peer's, or a client's, once its server's world has come; none
    /// for a server, which plays none.
    pub(crate) fn player(&self) -> Option<u8> {
        match self {
            Self::Peer(net) => Some(net.player()),
            Self::Server(_) => None,
            Self::Client(client) => client.player,
        }
    }

    /// Whether the run goes as another's world goes, as a joiner goes as its host's, and a client
    /// as its server's, whose worlds take its own's place, whose assets name its own.
    pub(crate) fn follows(&self) -> bool {
        match self {
            Self::Peer(net) => !net.hosts(),
            Self::Server(_) => false,
            Self::Client(_) => true,
        }
    }

    /// Whether tick `tick` can run, as [`Lockstep::ready`] and [`Client::ready`] say: a server's
    /// always can.
    pub(crate) fn ready(&self, tick: u64) -> bool {
        match self {
            Self::Peer(net) => net.ready(tick),
            Self::Server(_) => true,
            Self::Client(client) => client.ready(tick),
        }
    }

    /// Counts tick `tick`, which isn't [`ready`](Self::ready), as a peer's or a client's tick that
    /// waited, as [`Lockstep::waited`] and [`Client::waited`] say.
    pub(crate) fn waited(&mut self, tick: u64) {
        match self {
            Self::Peer(net) => net.waited(tick),
            Self::Server(_) => {}
            Self::Client(client) => client.waited(),
        }
    }

    /// The time a frame `dt` after the last owes the run's ticks: a peer's a little less, as
    /// [`Lockstep::pace`] says, a client's a little more or less, or more while it catches up, as
    /// [`Client::pace`] says, and a server's `dt`.
    pub(crate) fn pace(&mut self, dt: Duration) -> Duration {
        match self {
            Self::Peer(net) => net.pace(dt),
            Self::Server(_) => dt,
            Self::Client(client) => client.pace(dt),
        }
    }

    /// As tick `tick` starts, once it's [`ready`](Self::ready): takes the queues' commands as this
    /// run's inputs, as [`Lockstep::start_tick`], [`Server::start_tick`] and
    /// [`Client::start_tick`] say.
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

    /// What tick `tick` runs on, `world` as it starts: a peer's, as [`Lockstep::inputs`] says,
    /// which keeps a copy of the world as a tick it runs on the other's inputs predicted starts, as
    /// [`Lockstep::keep`] says; a server's, as [`Server::inputs`] says; and a client's, as
    /// [`Client::inputs`] says.
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

    /// Keeps what the edit offered with `ticket` did, as it applied before tick `tick`: a peer's,
    /// the only session whose edits a tick applies.
    pub(crate) fn did(&mut self, ticket: Ticket, tick: u64, done: Result<Done>) {
        if let Self::Peer(net) = self {
            net.did(ticket, tick, done);
        }
    }

    /// The sounds the runs of tick `tick` have played, if the tick may run again: a rollback peer's
    /// tick run on the other's inputs predicted, and a client's, which a server's state may correct.
    pub(crate) fn played(&mut self, tick: u64) -> Option<&mut Vec<Sound>> {
        match self {
            Self::Peer(net) => net.played(tick),
            Self::Server(_) => None,
            Self::Client(client) => Some(client.played.entry(tick).or_default()),
        }
    }

    /// Once tick `tick` has run, leaving `world`: a peer ends it if it ran on both peers' inputs,
    /// as [`Lockstep::end_tick`] says; a server sends its clients their states, and moves the
    /// world's change tick on, as [`Server::end_tick`] says; and a client keeps its world's hash,
    /// as [`Client::end_tick`] says.
    pub(crate) fn ran(&mut self, tick: u64, world: &mut World) {
        match self {
            Self::Peer(net) if net.settled(tick) => net.end_tick(tick, world),
            Self::Peer(_) => {}
            Self::Server(server) => server.end_tick(tick, world),
            Self::Client(client) => client.end_tick(tick, world),
        }
    }

    /// Between ticks, as a frame starts, the next to run being `next`: what the session must do of
    /// what came, as [`Lockstep::turn`], [`Server::turn`] and [`Client::turn`] say.
    pub(crate) fn turn(&mut self, next: u64) -> Turn {
        match self {
            Self::Peer(net) => net.turn(next),
            Self::Server(server) => server.turn(next),
            Self::Client(client) => client.turn(),
        }
    }

    /// Sends `world` to the joiner or the clients owed it, at tick `next`, as `write` writes it, a
    /// snapshot file's text: whole for a joiner, as [`Lockstep::send_world`] says, and of what each
    /// client sees, as [`Server::send_world`] says. `write` gives nothing for a world that can't
    /// be written, which stays owed.
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

    /// Once the world of a joiner's host, or a client's server, at tick `tick`, is `world`, in its
    /// own's place: says so, as [`Lockstep::restored`] and [`Client::restored`] say, and returns
    /// the tick the run goes on from.
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

    /// Lets go of the link a world that can't be restored came by, as `why` says.
    pub(crate) fn lose(&mut self, why: &str) {
        match self {
            Self::Peer(net) => net.lose(why),
            Self::Server(_) => {}
            Self::Client(client) => client.lose(why),
        }
    }

    /// Between ticks, the next to run being `next`, `world` as the last left it: the first tick to
    /// run again from, if any, from where [`start_of`](Self::start_of) says, as
    /// [`Lockstep::settle`] and [`Client::settle`] say.
    pub(crate) fn settle(&mut self, next: u64, world: &World) -> Option<u64> {
        match self {
            Self::Peer(net) => net.settle(next, world),
            Self::Server(_) => None,
            Self::Client(client) => client.settle(next, world),
        }
    }

    /// The world as tick `tick`, which [`settle`](Self::settle) gave, starts.
    pub(crate) fn start_of(&self, tick: u64) -> &World {
        match self {
            Self::Peer(net) => net.start_of(tick),
            Self::Server(_) => unreachable!("a server runs no tick again"),
            Self::Client(client) => client.start_of(tick),
        }
    }

    /// Counts the ticks from `from` to `next` run again, as [`settle`](Self::settle) had them.
    pub(crate) fn rolled_back(&mut self, from: u64, next: u64) {
        match self {
            Self::Peer(net) => net.rolled_back(from, next),
            Self::Server(_) => {}
            Self::Client(client) => client.rolled_back(from, next),
        }
    }

    /// Logs what the session did, as the run ends: how a rollback peer rolled back, what a server
    /// sent, and how a client was corrected.
    pub(crate) fn ended(&self) {
        match self {
            Self::Peer(net) => net.ended(),
            Self::Server(server) => server.ended(),
            Self::Client(client) => client.ended(),
        }
    }
}

/// The tick `text` names, as a line or a packet names one; `None` if it isn't a number fewer than
/// [`TICKS`].
fn tick_of(text: &str) -> Option<u64> {
    text.parse().ok().filter(|&tick| tick < TICKS)
}

/// The number `text` is, or none, written `-`, as a packet's header has a number it may not have
/// yet; `None` if it's neither.
fn optional<T: std::str::FromStr>(text: &str) -> Option<Option<T>> {
    if text == "-" {
        Some(None)
    } else {
        text.parse().ok().map(Some)
    }
}

/// Reads a packet's inputs, `lines`, as a peer's and a client's packets carry them: each tick's
/// commands, `c` lines of its tick, then the `t` that says they're whole, which gives the tick and
/// its commands, each with its layer's index, to `each`; `None` at a line that isn't one, those
/// before given.
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

/// Writes `inputs`, a peer's for tick `tick`, after `out`, as their lines, the `t` that says
/// they're whole last.
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
