//! Lockstep between two peers, as [Networking](../../../ARCHITECTURE.md#networking) describes: a
//! host, player 0, that listens, and a joiner, player 1, that connects and starts from the host's
//! world, and runs its host's scripts, which come with it; each tick's inputs, every layer's
//! commands and the edits that apply before it, a script's reload among them, sent as many ticks
//! ahead as the link needs, from [`DELAY`] until it's timed, and run once both peers' are in, or, in
//! a rollback session (`--rollback`), sent [`ROLLBACK_DELAY`] ahead and run before the other's have
//! come, as far as the link needs, from [`PREDICTED`] ticks on, on the prediction that they hold
//! nothing, the world taken back and the ticks run again from the first whose inputs held
//! something, as [`Lockstep::follow_link`] says; each peer's ticks paced so that neither runs far
//! ahead of the other, as [`Lockstep::pace`] says; the world's hash after each tick that's run on
//! both peers' inputs, compared, each peer keeping a copy of its world after the tick until it is,
//! so that where the hashes part, the host sends its joiner its world there, and the joiner names
//! the field where its own differs; and, in development builds, the patches of the game's code,
//! which the host has apply at a tick of its choosing.
//!
//! A session is kept to the runs given its code (`--code`): each connection opens with a handshake
//! keyed by it, which the end that took it refuses a run of another code at, as [`secure`] says,
//! and which makes the keys the stream's frames and the packets are sealed under, so that what
//! goes between the session's runs can't be read, forged or played again on the way.
//!
//! Runs that can't connect to each other, as two behind their own routers can't, meet through a
//! relay (`--relay`), which `gg-relay` runs: the host registers its session's room there, which
//! the code gives, and its joiner comes for it, as [`listen`] and [`connect`] say; the relay copies
//! their stream each way and forwards their packets, which it can't read or forge, and the packets
//! go direct where the routers between them let them, as [`path`] says.
//!
//! The session's stream is lines of text over TCP, sealed in frames, which a thread of the
//! connection's reads, and another writes, as the main thread sends them, so that a slow link
//! never holds the main thread:
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
mod path;
mod secure;
mod server;
mod states;

pub(crate) use client::Client;
pub(crate) use secure::Code;
#[cfg(test)]
pub(crate) use secure::TESTED;
/// The build the tests' sessions are of, as [`Build::name`] names one.
#[cfg(test)]
pub(crate) const BUILD: &str = "0123456789abcdef-fedcba9876543210";
pub(crate) use server::Server;

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::hash::{BuildHasher, RandomState};
use std::io::{self, BufRead, BufReader, ErrorKind, Read as _, Write as _};
use std::mem;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use gg_core::{Error, Result};
use gg_relay::{Hosting, Joined, Token};
use tracing::{debug, error, info, warn};

use crate::args::{Conditions, Listen, Net, Toward};
use crate::assets::{Asset, Counts};
use crate::cells::Seen;
use crate::edits::{Done, Edit};
use crate::layer::Commands;
use crate::package::Files;
use crate::recording::text::{self, Text};
use crate::reflect::Value;
use crate::{Context, Sound, Tick, World, WorldHash};
use path::{Change, Path};
use secure::{Keys, Opened, Replays, Sealer};

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

/// The hello's first part: the protocol and its version, which a change to its lines or its packets
/// takes to the next, a lockstep session's or a rollback session's, which both peers' must be.
const LOCKSTEP: &str = "gg-lockstep 9";
const ROLLBACK: &str = "gg-rollback 9";

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

/// How long a joiner waits between tries at connecting to its host.
const RETRY: Duration = Duration::from_millis(250);

/// How long a joiner its host refused waits before trying it again, as a host of another build
/// refuses it until one of them is rebuilt.
const REFUSED: Duration = Duration::from_secs(5);

/// How long a joiner through a relay waits between tries at finding its host there, as one that
/// hasn't registered yet isn't; and how long a host waits before registering again at a relay it
/// couldn't register at, or lost.
const UNHOSTED: Duration = Duration::from_secs(1);
const REGISTER: Duration = Duration::from_secs(5);

/// How long a link through a relay waits for the relay to answer its socket's bind before it binds
/// again; and, once answered, how long before it binds again anyway, as its router's mapping of
/// the socket may change, and as that keeps the mapping while the link sends nothing else.
const BIND: Duration = Duration::from_millis(250);
const BOUND: Duration = Duration::from_secs(1);

/// How long a link's writing thread goes with nothing to write before it writes `k`, a line that
/// says only that this end is there.
const KEEP: Duration = Duration::from_secs(1);

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

/// A tick run on the other peer's inputs predicted, which rollback runs again from if they hold
/// something: the world as it started, and the sounds its runs played, which a run again doesn't
/// play again.
struct Predicted {
    start: World,
    played: Vec<Sound>,
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

/// A connection to the other peer: what its threads write to it and read from it, each a thread of
/// its own, so that a slow link never holds the main thread; and the socket its packets come and go
/// by.
struct Link {
    /// What the writing thread writes, in order, each text whole; and why it stopped, if it did.
    to: Sender<String>,
    unwritten: Receiver<io::Error>,
    from: Receiver<String>,
    /// Where the connection comes from.
    peer: Option<SocketAddr>,
    /// When something last came by the link, a line or a packet, past [`Link::silence`] after
    /// which it's lost; and, while the other end's hello hasn't come on a link accepted, when the
    /// link was made, past [`HELLO`] after which it's lost.
    heard: Instant,
    unhello: Option<Instant>,
    silence: Duration,
    /// Set once the link counts: for the host, once the joiner has said it joined, before which
    /// what it sends was sent against a world it no longer has; for the joiner, once it has the
    /// host's world.
    joined: bool,
    /// The socket, of the address the connection is from, at a port the OS picks; and where the
    /// other peer's is, once its hello or its world has said, which alone its packets are taken
    /// from: on a link through a relay, the relay's, from the start.
    udp: UdpSocket,
    other: Option<SocketAddr>,
    /// A link's through a relay: the relay and the token its socket binds there by; where the
    /// relay last said the bind came from, the socket as its router maps it, and where this end
    /// last told the other its socket is, on the stream (`at`); when this end last sent a bind;
    /// and what one is written into.
    relayed: Option<Relayed>,
    reflexive: Option<SocketAddr>,
    said: Option<SocketAddr>,
    bind_sent: Option<Instant>,
    bind: Vec<u8>,
    /// A link's through a relay: which way its packets go, as [`Path`] chooses it.
    path: Path,
    /// What the link sent and took, which its changes of way and its end log.
    traffic: Traffic,
    /// The keys the link's handshake made, which its threads seal and open the stream's frames
    /// under, and the main thread its packets; the nonce of this end's next packet, from
    /// [`PACKETS`](secure::PACKETS); the nonces of those taken from the other end; what a packet's
    /// text goes into, after the byte that says whether this end hears the other direct, and what
    /// it's sealed into; and whether one from the other end's address was dropped, as forged or
    /// come before, which is warned of once a link.
    keys: Arc<Keys>,
    nonce: u64,
    replays: Replays,
    plain: Vec<u8>,
    sealed: Vec<u8>,
    forged: bool,
    /// What came, each held until it's due, as `--link` says, or at once.
    lines: VecDeque<(Instant, String)>,
    packets: Vec<(Instant, Vec<u8>)>,
    /// When this peer last sent its packets, if it has.
    sent: Option<Instant>,
    /// Whether a packet couldn't be sent, which is warned of once a link.
    unsent: bool,
    /// When the link was made, which this end's packets' clock counts from; the other end's clock
    /// as its latest packet gave it, and when that came, which this end's packets echo, so that
    /// the other times the way there and back by them; and the way there and back, as the other's
    /// packets' echoes of this end's clock time it.
    made: Instant,
    echo: Option<(u64, Instant)>,
    trip: Trip,
    /// The shortest time the packets taken this turn timed the way at, which alone is taken: those
    /// that waited to be taken while this end's frames stopped timed the wait as well.
    fastest: Option<Duration>,
    /// A peer's: how many ticks this one was past the other's next as the other's latest packet
    /// came, which this one's packets tell the other; and how many it's ahead of the other, its
    /// own count less the other's, halved, as GGPO's time sync has it, so that the way's length
    /// cancels out, a slow link's or an uneven one's, smoothed, as [`Lockstep::pace`] slows it.
    past: Option<i64>,
    ahead: f64,
}

impl Link {
    /// Starts reading `stream`'s lines, in the frames its handshake's `keys` open, on a thread of
    /// their own, which ends at a line that isn't one, as one past [`MAX_LINE`] isn't, at a frame
    /// that doesn't open, as the connection does, and at twice [`LOST`] with nothing come; starts
    /// writing what's sent on another, as [`write_from`] says; and binds the link's socket, which
    /// the main thread reads as each frame starts, without waiting. A link `accepted` is lost
    /// unless the other end's hello comes within [`HELLO`], as [`hello_came`](Self::hello_came)
    /// says. A link `relayed` sends its packets through the relay, its socket bound there, as
    /// [`take`](Self::take) binds it.
    fn new(
        stream: TcpStream,
        keys: Keys,
        accepted: bool,
        relayed: Option<Relayed>,
    ) -> std::io::Result<Self> {
        let udp = UdpSocket::bind(SocketAddr::new(stream.local_addr()?.ip(), 0))?;
        udp.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(LOST))?;
        // Past the link's silence, so that the link is lost for that first; and so that the thread
        // reading a link dropped, which reads on until the other end closes, ends though it never
        // does.
        stream.set_read_timeout(Some(LOST * 2))?;
        let peer = stream.peer_addr().ok();
        let keys = Arc::new(keys);
        let traffic = Traffic::default();
        let written = Arc::clone(&traffic.written);
        let (to, unwritten) = write_from(stream.try_clone()?, Arc::clone(&keys), written)?;
        let read = Counted {
            stream,
            bytes: Arc::clone(&traffic.read),
        };
        let mut reader = Opened::new(BufReader::new(read), Arc::clone(&keys));
        let (lines, from) = mpsc::channel();
        thread::Builder::new()
            .name("gg-net".to_owned())
            .spawn(move || {
                loop {
                    match read_line(&mut reader) {
                        Ok(Some(line)) => {
                            if lines.send(line).is_err() {
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            if error.kind() == ErrorKind::InvalidData {
                                warn!(
                                    code = "net.bad_line",
                                    "Dropping the lockstep peer's connection: {error}"
                                );
                            }
                            break;
                        }
                    }
                }
            })?;
        let now = Instant::now();
        Ok(Self {
            to,
            unwritten,
            from,
            peer,
            heard: now,
            unhello: accepted.then_some(now),
            silence: LOST,
            joined: false,
            udp,
            other: relayed.map(|relayed| relayed.relay),
            relayed,
            reflexive: None,
            said: None,
            bind_sent: None,
            bind: Vec::new(),
            path: Path::default(),
            traffic,
            keys,
            nonce: secure::PACKETS,
            replays: Replays::default(),
            plain: Vec::with_capacity(MAX_PACKET),
            sealed: Vec::with_capacity(MAX_PACKET),
            forged: false,
            lines: VecDeque::new(),
            packets: Vec::new(),
            sent: None,
            unsent: false,
            made: now,
            echo: None,
            trip: Trip::default(),
            fastest: None,
            past: None,
            ahead: 0.0,
        })
    }

    /// Sends `text`, whole lines, which the writing thread writes after what was sent before it,
    /// failing once that thread has stopped, as the connection has closed or a write failed.
    fn send(&mut self, text: String) -> std::io::Result<()> {
        self.to
            .send(text)
            .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "the link no longer writes to it"))
    }

    /// Has the other peer's packets be taken from its socket's `port`, at the address the
    /// connection comes from; on a link through a relay, from where they come from there, as they
    /// already are.
    fn packets_from(&mut self, port: u16) {
        if self.relayed.is_none() {
            self.other = self.peer.map(|at| SocketAddr::new(at.ip(), port));
        }
    }

    /// On a link through a relay, at `now`: binds its socket there, as [`gg_relay::bind`] says,
    /// every [`BIND`] until the relay answers, and every [`BOUND`] after.
    fn bind(&mut self, now: Instant) {
        let Some(relayed) = self.relayed else { return };
        let every = if self.reflexive.is_some() {
            BOUND
        } else {
            BIND
        };
        if self
            .bind_sent
            .is_some_and(|sent| now.saturating_duration_since(sent) < every)
        {
            return;
        }
        self.bind_sent = Some(now);
        gg_relay::bind(&relayed.token, &mut self.bind);
        match self.udp.send_to(&self.bind, relayed.relay) {
            Ok(_) => self.traffic.sent[RELAYED].add(self.bind.len()),
            Err(error) if !mem::replace(&mut self.unsent, true) => warn!(
                code = "net.link",
                "Can't bind the link's socket at the relay, as its packets go: {error}"
            ),
            Err(_) => {}
        }
    }

    /// On a link through a relay: tells the other end where its socket is, `at <reflexive>
    /// <local>`, as the relay last said it saw it, and as it's bound, each time the relay's answer
    /// changes, so that the other can send it packets direct, as [`Path`] says.
    fn say_where(&mut self) {
        let Some(reflexive) = self.reflexive.filter(|&at| self.said != Some(at)) else {
            return;
        };
        let Ok(local) = self.udp.local_addr() else {
            return;
        };
        self.said = Some(reflexive);
        // A failure loses the link at its next take, as the writing thread has stopped.
        let _ = self.send(format!("at {reflexive} {local}\n"));
    }

    /// Takes the other end's `at` line's words, `rest`: where its socket is, which a link through
    /// a relay then probes, as [`Path::at`] says.
    ///
    /// Fails, saying why, if it isn't one.
    fn at(&mut self, rest: &str) -> std::result::Result<(), String> {
        let mut words = rest.split(' ').map(str::parse::<SocketAddr>);
        let (Some(Ok(reflexive)), Some(Ok(local)), None) =
            (words.next(), words.next(), words.next())
        else {
            return Err(format!("the other end's `at` isn't one: at {rest}"));
        };
        #[cfg(test)]
        if tests::UNPUNCHED.get() {
            return Ok(());
        }
        if self.relayed.is_some()
            && let Ok(ours) = self.udp.local_addr()
        {
            self.path.at([reflexive, local], ours);
        }
        Ok(())
    }

    /// Logs `change`, a link's way changing, as [`Path::step`] gives it, with the packets the link
    /// has sent and taken each way, through the relay and direct, and its way there and back so
    /// far, as [`Trip::millis`] gives it, if it has been timed.
    fn log_way(&self, change: &Change) {
        let [relayed_sent, direct_sent] = self.traffic.sent.map(|count| count.packets);
        let [relayed_came, direct_came] = self.traffic.taken.map(|count| count.packets);
        let trip_ms = self.trip.millis().map(|[mean, _]| mean);
        match change {
            Change::Direct(at) => info!(
                code = "net.direct",
                address = %at,
                relayed_sent,
                direct_sent,
                relayed_came,
                direct_came,
                trip_ms,
                "The link's packets go direct to the other end, at {at}, past the relay"
            ),
            Change::Stopped => info!(
                code = "net.relayed",
                relayed_sent,
                direct_sent,
                relayed_came,
                direct_came,
                trip_ms,
                "The link's packets go through the relay again: none came direct for {} ms, or the \
                 other end had none; probing again for {} s",
                path::QUIET.as_millis(),
                path::PUNCH.as_secs()
            ),
            Change::Unpunched => info!(
                code = "net.relayed",
                relayed_sent,
                direct_sent,
                relayed_came,
                direct_came,
                trip_ms,
                "The link's packets go through the relay: no way direct to the other end worked \
                 both ways within {} s, as routers that map each destination their own way have it",
                path::PUNCH.as_secs()
            ),
        }
    }

    /// Notes that the other end's hello has come, after which the link isn't lost for want of one.
    fn hello_came(&mut self) {
        self.unhello = None;
    }

    /// The port this peer's packets come from.
    fn port(&self) -> u16 {
        self.udp.local_addr().map_or(0, |at| at.port())
    }

    /// This end's clock at `now`: the milliseconds since the link was made.
    fn clock(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.made).as_millis() as u64
    }

    /// Writes after `out` what a packet sent at `now` says of the times: this end's clock, and the
    /// other end's latest, with the milliseconds since it came, or `-` for each while none has.
    fn write_times(&self, out: &mut String, now: Instant) {
        let clock = self.clock(now);
        let written = "a string takes any text";
        match self.echo {
            Some((echo, came)) => {
                let held = now.saturating_duration_since(came).as_millis();
                write!(out, "{clock} {echo} {held}").expect(written);
            }
            None => write!(out, "{clock} - -").expect(written),
        }
    }

    /// Takes the times a packet of the other end's says, as [`write_times`](Self::write_times)
    /// wrote them, the packet having come at `now`: its clock, `clock`, which this end's packets
    /// echo from then on, unless a later one has come; and, if it echoes this end's, `echoed`,
    /// that clock and the milliseconds the other end held it, the time since, less those, which
    /// the way there and back took, as [`took`](Self::took) takes it.
    fn timed(&mut self, clock: u64, echoed: Option<(u64, u64)>, now: Instant) {
        if self.echo.is_none_or(|(latest, _)| clock >= latest) {
            self.echo = Some((clock, now));
        }
        if let Some((echo, held)) = echoed {
            let trip = self.clock(now).saturating_sub(echo).saturating_sub(held);
            let trip = Duration::from_millis(trip);
            self.fastest = Some(self.fastest.map_or(trip, |fastest| fastest.min(trip)));
        }
    }

    /// Once a turn has taken its packets: has the way there and back the shortest time they
    /// timed, as [`Trip::sample`] takes it.
    fn took(&mut self) {
        if let Some(fastest) = self.fastest.take() {
            self.trip.sample(fastest);
        }
    }

    /// Takes the lines read and the packets come since the last take, once what `conditions` hold
    /// is due from `now`, as [`receive`](Self::receive) holds them, the writing thread's `k`s among
    /// the lines, and why the link is lost, if it is, once what came before has been taken: its
    /// connection closed, or a write to it failed; nothing came by it for [`Link::silence`]; or
    /// the other end's hello hasn't come within [`HELLO`] of a link accepted.
    fn take(
        &mut self,
        buffer: &mut [u8],
        conditions: Option<Conditions>,
        random: &mut u64,
        now: Instant,
    ) -> (Vec<String>, Vec<Vec<u8>>, Option<String>) {
        let (mut lines, mut packets) = (Vec::new(), Vec::new());
        let mut closed = false;
        let latency = conditions.map_or(Duration::ZERO, |link| link.latency);
        loop {
            match self.from.try_recv() {
                Ok(line) => {
                    self.heard = now;
                    self.lines.push_back((now + latency, line));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    closed = true;
                    break;
                }
            }
        }
        while self.lines.front().is_some_and(|(due, _)| *due <= now) {
            lines.extend(self.lines.pop_front().map(|(_, line)| line));
        }
        self.bind(now);
        self.receive(buffer, conditions, random, now);
        self.traffic.count(self.clock(now) / 1000);
        self.say_where();
        self.packets.sort_by_key(|(due, _)| *due);
        let due = self.packets.partition_point(|(due, _)| *due <= now);
        packets.extend(self.packets.drain(..due).map(|(_, packet)| packet));
        // Lost once what came before has been read.
        let lost = if !self.lines.is_empty() {
            None
        } else if let Ok(error) = self.unwritten.try_recv() {
            Some(format!("can't write to it: {error}"))
        } else if closed {
            Some("the connection closed".to_owned())
        } else if now.saturating_duration_since(self.heard) >= self.silence {
            let silence = self.silence.as_secs_f64();
            Some(format!("nothing came from it for {silence} s"))
        } else if self
            .unhello
            .is_some_and(|made| now.saturating_duration_since(made) >= HELLO)
        {
            Some(format!("it said no hello within {} s", HELLO.as_secs()))
        } else {
            None
        };
        (lines, packets, lost)
    }

    /// Takes the packets that came from the other peer's socket, read through `buffer`, each's
    /// text, once it has opened and its nonce hasn't come before, as [`Replays`] says, held from
    /// `now` as `conditions` say, or lost, drawing on `random`. On a link through a relay, takes
    /// the relay's answers to its binds, and the other's packets that came direct, from wherever
    /// they came, as only the other end seals them so: those that don't open dropped unsaid, as
    /// anyone can send the socket anything; and has its [`Path`] take each.
    fn receive(
        &mut self,
        buffer: &mut [u8],
        conditions: Option<Conditions>,
        random: &mut u64,
        now: Instant,
    ) {
        loop {
            match self.udp.recv_from(buffer) {
                Ok((size, from)) => {
                    let relayed = self.relayed.is_some();
                    let usual = Some(from) == self.other;
                    if !usual && !relayed {
                        continue;
                    }
                    if relayed
                        && usual
                        && let Some(at) = gg_relay::bound(&buffer[..size])
                    {
                        self.reflexive = Some(at);
                        self.traffic.taken[RELAYED].add(size);
                        continue;
                    }
                    let opened = secure::open_packet(&self.keys, &buffer[..size]);
                    let fresh = opened.filter(|(nonce, _)| self.replays.fresh(*nonce));
                    // Each packet's text follows its byte.
                    let Some((_, mut text)) = fresh.filter(|(_, text)| !text.is_empty()) else {
                        if usual && !mem::replace(&mut self.forged, true) {
                            warn!(
                                code = "net.forged",
                                "Dropped a packet from the other end's address that doesn't \
                                 open under the link's keys, or came before: forged, changed on \
                                 the way or played again; those after are dropped unsaid"
                            );
                        }
                        continue;
                    };
                    let hears_direct = text.remove(0) == 1;
                    let mut due = now;
                    if let Some(conditions) = conditions {
                        if unit(random) < conditions.loss {
                            continue;
                        }
                        due += conditions.latency + conditions.jitter.mul_f64(unit(random));
                    }
                    if relayed {
                        let direct = (!usual).then_some(from);
                        self.path.came(direct, hears_direct, now);
                    }
                    let way = if relayed && usual { RELAYED } else { DIRECT };
                    self.traffic.taken[way].add(size);
                    self.heard = now;
                    self.packets.push((due, text));
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                // Windows' answer to a packet sent to a port nothing listens at, as the other
                // peer's after it has gone.
                Err(error) if error.kind() == ErrorKind::ConnectionReset => {}
                Err(error) => {
                    warn!(code = "net.link", "Can't read the peer's packets: {error}");
                    break;
                }
            }
        }
    }

    /// Sends `packet`, its text, at most [`MAX_PACKET_TEXT`] bytes, after a byte that says whether
    /// this end hears the other direct, sealed, to the other peer's socket, warning once a link of
    /// one that can't be sent, which is lost, as a packet may be. On a link through a relay, it goes
    /// the ways [`Path::step`] gives, each copy sealed under a nonce of its own, a change of way
    /// logged.
    fn send_packet(&mut self, packet: &[u8]) {
        let Some(other) = self.other else { return };
        let now = Instant::now();
        self.plain.clear();
        self.plain.push(u8::from(self.path.hears_direct(now)));
        self.plain.extend_from_slice(packet);
        if self.relayed.is_none() {
            self.send_sealed(other, DIRECT, true);
            return;
        }
        let (ways, change) = self.path.step(now);
        if let Some(change) = change {
            self.log_way(&change);
        }
        if ways.relay {
            self.send_sealed(other, RELAYED, true);
        }
        for to in ways.direct {
            // A probe to an address that turns out not to reach the other is unsaid.
            self.send_sealed(to, DIRECT, !ways.relay);
        }
    }

    /// Seals what [`send_packet`](Self::send_packet) wrote under the next nonce and sends it to
    /// `to`, counting it as gone `way`, [`RELAYED`] or [`DIRECT`], warning once a link of one that
    /// can't be sent if `warned`.
    fn send_sealed(&mut self, to: SocketAddr, way: usize, warned: bool) {
        secure::seal_packet(&self.keys, self.nonce, &self.plain, &mut self.sealed);
        self.nonce += 1;
        match self.udp.send_to(&self.sealed, to) {
            Ok(_) => self.traffic.sent[way].add(self.sealed.len()),
            Err(error) if warned && !mem::replace(&mut self.unsent, true) => warn!(
                code = "net.link",
                "Can't send the peer a packet, which is lost, as those after may be: {error}"
            ),
            Err(_) => {}
        }
    }

    /// Logs what the link sent and took (`net.traffic`), as it ends, `whose` it was, as
    /// [`Traffic`] counts it: for its whole time, and each way the bytes, their mean a second and
    /// the most in one, the stream's, and the packets' and how many, and how many of those went
    /// through the relay; and its way there and back as it ended, smoothed, and its deviation, as
    /// [`Trip::millis`] gives them, if it was timed. IP's and UDP's headers aren't counted: a
    /// packet's count gives them, some 28 bytes a packet over IPv4.
    fn ended(&self, whose: &str) {
        let seconds = self.made.elapsed().as_secs_f64();
        let traffic = &self.traffic;
        // Each counter read once, as the link's threads may count on meanwhile.
        let [stream_sent, stream_taken] =
            [&traffic.written, &traffic.read].map(|bytes| bytes.load(Ordering::Relaxed));
        let [packet_bytes_sent, packet_bytes_taken] =
            [traffic.sent, traffic.taken].map(|[relayed, direct]| relayed.bytes + direct.bytes);
        let [packets_sent, packets_taken] =
            [traffic.sent, traffic.taken].map(|[relayed, direct]| relayed.packets + direct.packets);
        let [relayed_sent, relayed_taken] =
            [traffic.sent, traffic.taken].map(|[relayed, _]| relayed.packets);
        let (bytes_sent, bytes_taken) = (
            stream_sent + packet_bytes_sent,
            stream_taken + packet_bytes_taken,
        );
        let [busiest_sent, busiest_taken] = traffic.busiest([bytes_sent, bytes_taken]);
        let a_second = |bytes: u64| {
            if seconds > 0.0 {
                (bytes as f64 / seconds).round() as u64
            } else {
                0
            }
        };
        let (per_second_sent, per_second_taken) = (a_second(bytes_sent), a_second(bytes_taken));
        let trip = self.trip.millis();
        let (trip_ms, trip_deviation_ms) = (trip.map(|[mean, _]| mean), trip.map(|[_, dev]| dev));
        info!(
            code = "net.traffic",
            seconds,
            bytes_sent,
            bytes_taken,
            per_second_sent,
            per_second_taken,
            busiest_sent,
            busiest_taken,
            stream_sent,
            stream_taken,
            packet_bytes_sent,
            packet_bytes_taken,
            packets_sent,
            packets_taken,
            relayed_sent,
            relayed_taken,
            trip_ms,
            trip_deviation_ms,
            "The link to {whose} ended after {seconds:.1} s. It sent {bytes_sent} bytes, \
             {per_second_sent} a second on average and {busiest_sent} in its busiest second: \
             {stream_sent} on the stream, and {packet_bytes_sent} in {packets_sent} packets, \
             {relayed_sent} of them through the relay. It took {bytes_taken} bytes, \
             {per_second_taken} a second on average and {busiest_taken} in its busiest second: \
             {stream_taken} on the stream, and {packet_bytes_taken} in {packets_taken} packets, \
             {relayed_taken} of them through the relay"
        );
    }
}

/// Starts the thread that writes to `stream` what's sent through the channel it returns, each text
/// whole and in order, sealed under `keys` as [`Sealer`] seals it, and `k` after [`KEEP`] with
/// nothing else, so that the other end hears from this one while its main thread is busy, as
/// restoring a world or stopped at a script's breakpoint, adding the bytes it writes to `written`;
/// and returns, with the channel, where the error comes that stopped it, if one did.
///
/// Once the channel closes, as its link is dropped, and what was sent before is written, it shuts
/// the connection's sending side: the other end reads what was sent, as a refusal's why, and then
/// its end, and closes its side, which ends the thread reading this one, which has read all that
/// came. Shutting the receiving side too would have Windows reset the connection at what came that
/// wasn't read, as a joiner's hello the host refuses unread, and the other end lose what was sent.
/// At a write that fails, as one that can't go on for [`LOST`] does, it shuts both sides.
fn write_from(
    stream: TcpStream,
    keys: Arc<Keys>,
    written: Arc<AtomicU64>,
) -> io::Result<(Sender<String>, Receiver<io::Error>)> {
    let (to, texts) = mpsc::channel::<String>();
    let (failed, unwritten) = mpsc::channel();
    thread::Builder::new()
        .name("gg-net-write".to_owned())
        .spawn(move || {
            let mut sealer = Sealer::new(keys);
            let mut out = Counted {
                stream,
                bytes: written,
            };
            loop {
                let wrote = match texts.recv_timeout(KEEP) {
                    Ok(text) => sealer.write(&mut out, text.as_bytes()),
                    Err(RecvTimeoutError::Timeout) => sealer.write(&mut out, b"k\n"),
                    Err(RecvTimeoutError::Disconnected) => {
                        let _ = out.stream.shutdown(Shutdown::Write);
                        return;
                    }
                };
                if let Err(error) = wrote {
                    let _ = failed.send(error);
                    let _ = out.stream.shutdown(Shutdown::Both);
                    return;
                }
            }
        })?;
    Ok((to, unwritten))
}

/// A stream that adds the bytes read from it, or written to it, to `bytes`: a link's, whose
/// threads count what its stream carries, sealed.
struct Counted<S> {
    stream: S,
    bytes: Arc<AtomicU64>,
}

impl<S: io::Read> io::Read for Counted<S> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let size = self.stream.read(out)?;
        self.bytes.fetch_add(size as u64, Ordering::Relaxed);
        Ok(size)
    }
}

impl<S: io::Write> io::Write for Counted<S> {
    fn write(&mut self, text: &[u8]) -> io::Result<usize> {
        let size = self.stream.write(text)?;
        self.bytes.fetch_add(size as u64, Ordering::Relaxed);
        Ok(size)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// The indices of what a link's packets went through, in [`Traffic`]'s counts: the relay, or
/// direct to the other end, as every packet of a link not through a relay goes.
const RELAYED: usize = 0;
const DIRECT: usize = 1;

/// What a link sent and took, which `net.traffic` logs as it ends: its stream's bytes, sealed in
/// their frames, as its threads write and read them, `k`s among them; its packets and their bytes,
/// sealed, each way, by what they went through, as the main thread sends them and takes them from
/// the other end, the binds and the relay's answers among those through the relay, and those
/// `--link` loses not taken; and the most bytes in a second of the link, each way, as the main
/// thread counts them as it takes what came, a frame's at a time, so that a second its frames
/// stopped for counts toward the next.
#[derive(Default)]
struct Traffic {
    written: Arc<AtomicU64>,
    read: Arc<AtomicU64>,
    /// By [`RELAYED`] and [`DIRECT`].
    sent: [Count; 2],
    taken: [Count; 2],
    /// The second of the link counted toward, how many bytes each way had gone as it started,
    /// and the most in one of those before it, each way.
    second: u64,
    at_second: [u64; 2],
    most: [u64; 2],
}

/// Packets and their bytes.
#[derive(Clone, Copy, Default)]
struct Count {
    packets: u64,
    bytes: u64,
}

impl Count {
    fn add(&mut self, bytes: usize) {
        self.packets += 1;
        self.bytes += bytes as u64;
    }
}

impl Traffic {
    /// The bytes sent and taken so far, the stream's and the packets'.
    fn bytes(&self) -> [u64; 2] {
        let packets = |[relayed, direct]: [Count; 2]| relayed.bytes + direct.bytes;
        [
            self.written.load(Ordering::Relaxed) + packets(self.sent),
            self.read.load(Ordering::Relaxed) + packets(self.taken),
        ]
    }

    /// Counts the bytes since the last second started toward it, once the link's second is
    /// `second`, past it, and starts that one.
    fn count(&mut self, second: u64) {
        if second != self.second {
            let bytes = self.bytes();
            self.most = self.busiest(bytes);
            self.at_second = bytes;
            self.second = second;
        }
    }

    /// The most bytes in a second of the link, each way, the one going on among them, `bytes`
    /// having gone so far, as [`bytes`](Self::bytes) gave them.
    fn busiest(&self, bytes: [u64; 2]) -> [u64; 2] {
        [0, 1].map(|way| self.most[way].max(bytes[way] - self.at_second[way]))
    }
}

/// A link's way there and back, smoothed as TCP smooths its own (RFC 6298's SRTT and RTTVAR): its
/// mean and its mean deviation, in seconds, from each time a packet's echo gives; none before the
/// first.
#[derive(Default)]
struct Trip {
    mean: Option<f64>,
    deviation: f64,
}

impl Trip {
    /// Takes a time the way there and back took, `trip`.
    fn sample(&mut self, trip: Duration) {
        let trip = trip.as_secs_f64();
        match self.mean {
            Some(mean) => {
                self.deviation += ((mean - trip).abs() - self.deviation) / 4.0;
                self.mean = Some(mean + (trip - mean) / 8.0);
            }
            None => (self.mean, self.deviation) = (Some(trip), trip / 2.0),
        }
    }

    /// The ticks the way one way takes, half the way there and back, and [`MARGIN`] deviations,
    /// rounded up: within which what's sent comes, but seldom; none before the first time is taken.
    fn ticks(&self) -> Option<u64> {
        let way = self.mean? / 2.0 + MARGIN * self.deviation;
        Some((way / Tick::DT.as_secs_f64()).ceil() as u64)
    }

    /// The mean and the mean deviation in milliseconds, to a tenth, as the logs give them; none
    /// before the first time is taken.
    fn millis(&self) -> Option<[f64; 2]> {
        let millis = |seconds: f64| (seconds * 10_000.0).round() / 10.0;
        Some([millis(self.mean?), millis(self.deviation)])
    }
}

/// How many of its deviations past the way one way a link's delay allows: two, where TCP allows
/// four for its retransmissions. The deviation is the way there and back's, some 1.4 times one
/// way's where the two ways' jitter is its own, so two allow nearly three of one way's; and inputs
/// that come late cost a stall, which `net.delay` counts, where four had the delay most of the way
/// there and back.
const MARGIN: f64 = 2.0;

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

/// How a peer finds the other: the host by the connections its listener accepts and its relay
/// pairs it with, the joiner by connecting to its host's address or coming for its room at its
/// relay, again each time it loses it, by the session's code.
enum Find {
    Host(Accepted),
    Join {
        toward: Toward,
        code: Code,
        connecting: Option<Receiver<Handshaken>>,
        looking: Looking,
    },
}

impl Find {
    /// Has a joiner connect to its host again after [`REFUSED`], as after a refusal.
    fn again(&mut self) {
        if let Self::Join {
            toward,
            code,
            connecting,
            looking,
        } = self
        {
            let host = "the lockstep host";
            *connecting = Some(connect(toward.clone(), host, REFUSED, *code, looking));
        }
    }

    /// What the session's relay last answered, as [`Heard`] says.
    fn heard(&self) -> Answer {
        match self {
            Self::Host(accepted) => accepted.heard.get(),
            Self::Join { looking, .. } => looking.heard.get(),
        }
    }
}

/// A peer of a lockstep session.
pub(crate) struct Lockstep {
    find: Find,
    /// Where the host listens, as the OS has it: for tests, which listen where it picks.
    #[cfg(test)]
    listening: Option<SocketAddr>,
    /// The session's code, written as [`Code`]'s `Display` writes it, for [`SessionStatus`]: the
    /// session's key, logged nowhere.
    code: Arc<str>,
    /// The joiner's: why its host last refused it, until the host's world comes.
    refused: Option<Arc<str>>,
    /// The joiner's: whether its host's world has come, so that a link lost after is its host lost.
    played: bool,
    app: &'static str,
    /// The run's build, as [`Build::name`] names it, which both peers' must be: a test's own, to
    /// stand for another's.
    build: String,
    /// The layers' names, from the bottom up, which both peers' must be.
    layers: String,
    link: Option<Link>,
    /// The joiner's world as it comes, the tick it's at and its lines so far.
    coming: Option<(u64, String)>,
    /// Set while the host owes its joiner its world, as it joins or after a desync.
    owed: bool,
    /// Set from a desync until the peers' worlds are made the same again, while their hashes
    /// aren't compared.
    parted: bool,
    /// How many ticks ahead this peer sends its inputs, and the most it runs ahead of the other's
    /// inputs, on them predicted: lockstep's, none, or a rollback session's. A lockstep peer's
    /// delay and a rollback peer's window follow the link, as [`follow_link`](Self::follow_link)
    /// says.
    delay: u64,
    window: u64,
    /// The delays or windows the link has needed in the last [`FALLS`], as a sliding window's
    /// maximum keeps them: each with when it was needed, each needed less than those before it and
    /// later, so that the first is the most, and the values from the least to the most each appear
    /// once at most, 15 of them.
    needs: VecDeque<(Instant, u64)>,
    /// The least and the most delay or window this peer has had with the other there; how many
    /// ticks waited for the other's inputs, and the last that did; and its stalls as they waited:
    /// as the run ends, `net.delay` logs them.
    ranged: Option<[u64; 2]>,
    waits: u64,
    waited: Option<u64>,
    stalls: Stalls,
    /// What `--link` has the link do to what comes, and the state of the random numbers it draws.
    conditions: Option<Conditions>,
    random: u64,
    /// The next tick this peer runs, as its packets tell the other.
    next: u64,
    /// The first tick this peer hasn't made its inputs for, those for each tick before having gone
    /// out; and, while its delay falls, a tick's it holds back, which go out with the next tick's.
    tagged: u64,
    held: Option<Inputs>,
    /// This peer's inputs, by tick, kept until the other peer has them and the tick has run on
    /// both peers'.
    ours: BTreeMap<u64, Inputs>,
    /// The other peer's, by tick, kept until the tick has run on them.
    theirs: BTreeMap<u64, Inputs>,
    /// The tick from which the other peer's inputs are needed: before it, they're empty; `None`
    /// while there's no other peer, for the host, which plays on alone.
    from: Option<u64>,
    /// The first tick from `from` whose inputs from the other peer haven't come whole, all before
    /// it having come; and the first of this peer's that the other hasn't said it has.
    whole: u64,
    acked: u64,
    /// The tick the peers' worlds were last made the same at, as the host's came, which their
    /// packets name, so that those sent before, which may come after, are dropped.
    since: u64,
    /// The ticks run on the other peer's inputs predicted, which haven't run on them since, by
    /// tick.
    predicted: BTreeMap<u64, Predicted>,
    /// How many times this peer rolled back, how many ticks it ran again, and the most at once.
    rolled: [u64; 3],
    /// What a packet is read into.
    buffer: Vec<u8>,
    /// What this peer offered since its last tick, with the ticket of each that's answered, which
    /// goes out with its inputs for the tick its delay on from its next.
    offered: Vec<(Option<Ticket>, Outside)>,
    /// The last ticket given.
    #[cfg(feature = "dev")]
    tickets: Ticket,
    /// What the edits and patches this peer offered did, by ticket, with the tick they applied
    /// before.
    done: Vec<(Ticket, u64, Result<Done>)>,
    /// The world's hash after each tick, this peer's and the other's, until they're compared.
    hashes: [BTreeMap<u64, WorldHash>; 2],
    /// A copy of this peer's world after each tick whose hashes haven't been compared, by tick,
    /// so that a desync names where the worlds parted: the joiner's after the tick they parted at
    /// kept until its host's comes. And copies kept from before, which they and the predicted
    /// ticks' are taken into again.
    worlds: BTreeMap<u64, World>,
    spares: Vec<World>,
    /// The joiner's: its host's world after the tick their hashes parted at, as it comes, the tick
    /// and its lines so far.
    parting: Option<(u64, String)>,
    #[cfg(feature = "dev")]
    patches: Patches,
    /// The host's: how many textures, models and fonts there were as it last told its joiner their
    /// names, the engine's white texture among them.
    #[cfg(feature = "dev")]
    named: Counts,
    /// The joiner's: the names its host gave the textures it added, each with the path it names,
    /// as they came since they were last taken.
    #[cfg(feature = "dev")]
    names: Vec<(Arc<str>, Arc<str>)>,
}

/// The patches `gg run` gave a peer since its process started, which it gives both peers of a
/// session alike and in the same order, each built from every edit since the build, so that one
/// holds those before it.
#[cfg(feature = "dev")]
#[derive(Default)]
struct Patches {
    /// Each patch, numbered from 1 by its place, with its ticket, until it applies.
    got: Vec<Option<(subsecond::JumpTable, Ticket)>>,
    /// How many have applied, from the first: the code the peer runs.
    applied: usize,
    /// Each patch that's to apply, by its number, with the tick it applies before: the host's
    /// choice, which its joiner takes, [`UNSENT`] until the host's inputs that carry it go out.
    due: Vec<(usize, u64)>,
    /// The joiner's: how many its host had applied, as its host's world comes.
    hosts: usize,
}

/// The tick a patch the host was given applies before until the host's inputs that carry it go
/// out, which none reaches.
#[cfg(feature = "dev")]
const UNSENT: u64 = u64::MAX;

impl Lockstep {
    /// Starts the session `net` says, keyed by `code`, for app `app` of build `build`, as
    /// [`Build::name`] names it, whose layers are `layers`, from the bottom up: the host listening
    /// at its address, the joiner trying to connect to its host's, in the background, until it
    /// does.
    ///
    /// Fails with `net.start` if the host can't listen at its address.
    pub(crate) fn start(
        net: &Net,
        code: Code,
        app: &'static str,
        build: &str,
        layers: &[&str],
    ) -> Result<Self> {
        #[cfg(test)]
        let mut listening = None;
        let find = match net {
            Net::Host(at) => {
                let (find, at) = listen(at, "a lockstep peer", code)?;
                #[cfg(test)]
                {
                    listening = at;
                }
                match at {
                    Some(at) => info!(address = %at, "Hosting a lockstep session"),
                    None => info!("Hosting a lockstep session at its relay alone"),
                }
                Find::Host(find)
            }
            Net::Join(toward) => Find::Join {
                toward: toward.clone(),
                code,
                connecting: None,
                looking: Looking::default(),
            },
            Net::Serve(_) | Net::Connect(_) => {
                unreachable!(
                    "a server's session and a client's start as `Session::start` starts them"
                )
            }
        };
        let host = matches!(find, Find::Host(_));
        Ok(Self {
            find,
            #[cfg(test)]
            listening,
            code: code.to_string().into(),
            refused: None,
            played: false,
            app,
            build: build.to_owned(),
            layers: layers.join(" "),
            link: None,
            coming: None,
            owed: false,
            parted: false,
            delay: DELAY,
            window: 0,
            needs: VecDeque::with_capacity((MOST_DELAY - LEAST_DELAY + 1) as usize),
            ranged: None,
            waits: 0,
            waited: None,
            stalls: Stalls::default(),
            conditions: None,
            random: 0,
            next: 0,
            tagged: 0,
            held: None,
            ours: BTreeMap::new(),
            theirs: BTreeMap::new(),
            from: (!host).then_some(0),
            whole: 0,
            acked: 0,
            since: 0,
            predicted: BTreeMap::new(),
            rolled: [0; 3],
            buffer: vec![0; 1 << 16],
            offered: Vec::new(),
            #[cfg(feature = "dev")]
            tickets: 0,
            done: Vec::new(),
            hashes: Default::default(),
            worlds: BTreeMap::new(),
            spares: Vec::new(),
            parting: None,
            #[cfg(feature = "dev")]
            patches: Patches::default(),
            #[cfg(feature = "dev")]
            named: (0, 0, 0, 0),
            #[cfg(feature = "dev")]
            names: Vec::new(),
        })
    }

    /// Has the peer predict the other's inputs, as a rollback session's peers do (`--rollback`):
    /// before its first turn.
    pub(crate) fn predict(&mut self) {
        self.delay = ROLLBACK_DELAY;
        self.window = PREDICTED;
    }

    /// Has the link do to what comes as `conditions` say, as `--link` asks.
    pub(crate) fn condition(&mut self, conditions: Conditions) {
        self.conditions = Some(conditions);
        self.random = RandomState::new().hash_one(0);
    }

    /// The protocol the peer speaks, which the other's must.
    fn protocol(&self) -> &'static str {
        if self.window > 0 { ROLLBACK } else { LOCKSTEP }
    }

    /// The app the session is of.
    pub(crate) fn app(&self) -> &'static str {
        self.app
    }

    /// This peer's player: 0 for the host, 1 for the joiner.
    pub(crate) fn player(&self) -> u8 {
        match self.find {
            Find::Host(_) => 0,
            Find::Join { .. } => 1,
        }
    }

    /// Whether this peer is the host.
    pub(crate) fn hosts(&self) -> bool {
        self.player() == 0
    }

    /// Where the session stands for this peer, as [`SessionPhase`] says.
    fn phase(&self) -> SessionPhase {
        let heard = self.find.heard();
        if self.hosts() {
            return match (&self.link, heard) {
                (Some(_), _) => SessionPhase::Playing,
                (None, Answer::Unreachable(why)) => SessionPhase::Unreachable { why },
                (None, _) => SessionPhase::Waiting,
            };
        }
        if self.link.is_some() && !self.waiting() {
            return SessionPhase::Playing;
        }
        if let Some(why) = &self.refused {
            return SessionPhase::Refused { why: why.clone() };
        }
        if self.link.is_some() {
            return SessionPhase::Joining;
        }
        if self.played {
            return SessionPhase::Lost;
        }
        match heard {
            Answer::Unhosted => SessionPhase::Unhosted,
            Answer::Unreachable(why) => SessionPhase::Unreachable { why },
            Answer::Nothing | Answer::Hosted => SessionPhase::Joining,
        }
    }

    /// Whether the peer has no world to run: a joiner that hasn't the host's yet, or whose host is
    /// sending it again, whose own the host's takes the place of.
    pub(crate) fn waiting(&self) -> bool {
        !self.hosts()
            && (self.coming.is_some() || !self.link.as_ref().is_some_and(|link| link.joined))
    }

    /// Between ticks, as a frame starts, the next to run being `next`: takes the lines read, the
    /// packets come and the connections made since the last turn, those `--link` holds once they're
    /// due, and returns what the peer must do of them: send its world, if it's the host and owes
    /// one, or restore the host's, if it's the joiner and has one whole. Sends its packets again a
    /// tick's time after it last did, as a peer waiting for the other's inputs, and the other for
    /// its own, sends none otherwise. Logs a connection lost, a peer refused and a desync.
    pub(crate) fn turn(&mut self, next: u64) -> Turn {
        let now = Instant::now();
        self.next = next;
        let (lines, packets, lost) = match &mut self.link {
            Some(link) => link.take(&mut self.buffer, self.conditions, &mut self.random, now),
            None => Default::default(),
        };
        let mut turn = Turn::None;
        for line in lines {
            if let Some(world) = self.read(&line) {
                turn = Turn::Restore(world);
            }
            if self.link.is_none() {
                break;
            }
        }
        for packet in packets {
            if self.link.is_none() {
                break;
            }
            self.read_packet(&packet, now);
        }
        if let Some(link) = &mut self.link {
            link.took();
        }
        if let Some(why) = lost
            && self.link.is_some()
        {
            self.lose(&why);
        }
        // After the link's lines, so that a joiner that closed its link and connects again finds
        // the host's place free, as the host has seen the close.
        self.find_peer();
        let sent = self.link.as_ref().and_then(|link| link.sent);
        if sent.is_some_and(|sent| now.saturating_duration_since(sent) >= Tick::DT) {
            self.send_packets();
        }
        if matches!(turn, Turn::None) && self.owed && self.link.is_some() {
            turn = Turn::Send;
        }
        turn
    }

    /// Takes a connection the host accepted, or the joiner made, if one has come whose handshake
    /// held, logging one whose didn't: refused, the joiner trying again after [`REFUSED`].
    fn find_peer(&mut self) {
        let made = match &mut self.find {
            Find::Host(accepted) => {
                while let Some(handshaken) = accepted.handshaken() {
                    let shaken = match handshaken {
                        Ok(shaken) => shaken,
                        Err(why) => {
                            warn!(code = "net.refused", "Refused a lockstep peer: {why}");
                            continue;
                        }
                    };
                    match Link::new(shaken.stream, shaken.keys, true, shaken.relayed) {
                        Ok(link) if self.link.is_none() => self.link = Some(link),
                        // A second joiner, refused before its hello.
                        Ok(mut link) => {
                            let why = "refused the session has its two players\n";
                            let _ = link.send(why.to_owned());
                        }
                        Err(error) => warn!(code = "net.link", "Can't take a peer: {error}"),
                    }
                }
                return;
            }
            Find::Join { .. } if self.link.is_some() => return,
            Find::Join {
                toward,
                code,
                connecting,
                looking,
            } => {
                let made = connecting.get_or_insert_with(|| {
                    let host = "the lockstep host";
                    connect(toward.clone(), host, Duration::ZERO, *code, looking)
                });
                let Ok(handshaken) = made.try_recv() else {
                    return;
                };
                *connecting = None;
                match handshaken {
                    Ok(shaken) => (shaken, toward.clone()),
                    Err(why) => {
                        error!(
                            code = "net.refused",
                            "The lockstep host refused this peer in the handshake: {why}; it \
                             tries again in {} s",
                            REFUSED.as_secs()
                        );
                        self.refused = Some(why.to_string().into());
                        self.find.again();
                        return;
                    }
                }
            }
        };
        let (shaken, toward) = made;
        let protocol = self.protocol();
        let hello = |link: &Link| hello(protocol, self.app, &self.build, &self.layers, link.port());
        let link = Link::new(shaken.stream, shaken.keys, false, shaken.relayed)
            .and_then(|mut link| link.send(hello(&link)).map(|()| link));
        match link {
            Ok(link) => {
                match toward {
                    Toward::Address(address) => {
                        info!(address = %address, "Connected to the lockstep host");
                    }
                    Toward::Relay(relay) => {
                        info!(relay = %relay, "Connected to the lockstep host through its relay");
                    }
                }
                self.link = Some(link);
            }
            Err(error) => warn!(code = "net.link", "Can't reach the host: {error}"),
        }
    }

    /// Takes `line`, from the other peer; returns the host's world, the tick it's at and its text,
    /// once its last line has come, after which the joiner takes the host's inputs.
    fn read(&mut self, line: &str) -> Option<(u64, String)> {
        let gathering = match (&mut self.coming, &mut self.parting) {
            (Some((_, text)), _) | (None, Some((_, text))) => Some(text),
            (None, None) => None,
        };
        if let Some(text) = gathering {
            if text.len() + line.len() >= MAX_WORLD {
                self.lose(&format!("the host's world is more than {MAX_WORLD} bytes"));
                return None;
            }
            text.push_str(line);
            text.push('\n');
            if line != "end" {
                return None;
            }
            if let Some((tick, text)) = self.coming.take() {
                self.came(tick);
                return Some((tick, text));
            }
            let (tick, text) = self.parting.take()?;
            self.name_parting(tick, &text);
            return None;
        }
        // The hello's parts are split by tabs, the rest's by spaces.
        let (kind, rest) = line.split_once([' ', '\t']).unwrap_or((line, ""));
        let joined = self.link.as_ref().is_some_and(|link| link.joined);
        match kind {
            // The other end's writing thread, saying it's there.
            "k" => {}
            "at" => {
                let at = self.link.as_mut().map(|link| link.at(rest));
                if let Some(Err(why)) = at {
                    self.lose(&why);
                }
            }
            "hello" if self.hosts() => self.hello(line),
            "refused" if !self.hosts() => {
                error!(
                    code = "net.refused",
                    "The lockstep host refused this peer: {rest}; it tries again in {} s",
                    REFUSED.as_secs()
                );
                self.refused = Some(rest.into());
                self.lose("the host refused it");
                self.find.again();
            }
            "world" if !self.hosts() => {
                let mut words = rest.split(' ');
                let mut number = || words.next().and_then(tick_of);
                let world = (number(), number(), number(), number());
                // A rollback session's peers' delay is the same; a lockstep session's are their own.
                let most = if self.window > 0 {
                    ROLLBACK_DELAY
                } else {
                    MOST_DELAY
                };
                match world {
                    (Some(tick), Some(from), Some(patches), Some(port))
                        if from >= tick && from - tick <= most =>
                    {
                        let (Ok(patches), Ok(port)) =
                            (usize::try_from(patches), u16::try_from(port))
                        else {
                            self.lose("the host's world names no patches or port");
                            return None;
                        };
                        #[cfg(feature = "dev")]
                        {
                            self.patches.hosts = patches;
                        }
                        // Builds without patches are refused by hosts with them.
                        #[cfg(not(feature = "dev"))]
                        let _ = patches;
                        if let Some(link) = &mut self.link {
                            link.packets_from(port);
                        }
                        // This peer's inputs go out from the tick its host needs them from, which
                        // its delay reaches from the host's world's.
                        self.tagged = from;
                        self.delay = self.delay.max(from - tick);
                        self.coming = Some((tick, String::new()));
                    }
                    _ => self.lose(&format!(
                        "the host's world names no tick, the joiner's first, patches or port: \
                         {line}"
                    )),
                }
            }
            "joined" if self.hosts() => {
                if let Some(link) = &mut self.link {
                    link.joined = true;
                    info!(tick = rest, "The joiner joined the lockstep session");
                }
            }
            // Gathered whatever the joiner's world, so that its lines aren't read as others.
            "parted" if !self.hosts() => match tick_of(rest) {
                Some(tick) => self.parting = Some((tick, String::new())),
                None => self.lose(&format!("the peer sent a line that isn't one: {line}")),
            },
            // Sent against a world the joiner no longer has, as the host's comes.
            _ if !joined => {}
            "c" | "e" | "s" => {
                let mut parts = rest.splitn(if kind == "c" { 3 } else { 2 }, ' ');
                let tick = parts.next().and_then(tick_of);
                let layer = if kind == "c" {
                    parts.next().and_then(|layer| layer.parse::<usize>().ok())
                } else {
                    Some(0)
                };
                let value = parts.next().map(str::to_owned);
                match (tick, layer, value) {
                    (Some(tick), Some(layer), Some(value)) if self.near(tick) => {
                        let inputs = self.theirs.entry(tick).or_default();
                        // Come again, as in the world after its packets.
                        if !inputs.whole {
                            match kind {
                                "c" => inputs.commands.push((layer, value)),
                                "e" => inputs.edits.push((None, value)),
                                _ => inputs.scenes.push((None, value)),
                            }
                        }
                    }
                    _ => self.lose(&format!("the peer sent a line that isn't one: {line}")),
                }
            }
            "t" => match tick_of(rest).filter(|&tick| self.near(tick)) {
                Some(tick) => {
                    self.theirs.entry(tick).or_default().whole = true;
                    self.came_whole();
                }
                None => self.lose(&format!("the peer sent a line that isn't one: {line}")),
            },
            "h" => {
                let hash = rest.split_once(' ').and_then(|(tick, hash)| {
                    let (content, layout) = hash.split_once(' ')?;
                    let hex = |text: &str| u64::from_str_radix(text, 16).ok();
                    let hash = WorldHash {
                        content: hex(content)?,
                        layout: hex(layout)?,
                    };
                    Some((tick_of(tick)?, hash))
                });
                match hash {
                    Some((tick, hash)) => {
                        self.hashes[1].insert(tick, hash);
                        self.compare();
                    }
                    None => self.lose(&format!("the peer sent a line that isn't one: {line}")),
                }
            }
            #[cfg(feature = "dev")]
            "p" if !self.hosts() => {
                let due = rest.split_once(' ').and_then(|(number, tick)| {
                    Some((number.parse::<usize>().ok()?, tick_of(tick)?))
                });
                match due {
                    Some((number, tick)) if number > 0 && self.near(tick) => {
                        let inputs = self.theirs.entry(tick).or_default();
                        if !inputs.whole {
                            inputs.patches.push(number);
                            self.patches.due.push((number, tick));
                        }
                    }
                    _ => self.lose(&format!("the peer sent a line that isn't one: {line}")),
                }
            }
            #[cfg(feature = "dev")]
            "n" if !self.hosts() => match text::read_named(rest, "n ".len() + 1) {
                Some(named) => self.names.push(named),
                None => self.lose(&format!("the peer sent a line that isn't one: {line}")),
            },
            _ => self.lose(&format!("the peer sent a line that isn't one: {line}")),
        }
        None
    }

    /// Takes the joiner's hello, `line`, refusing it unless it's of this protocol, app, engine,
    /// build and layers, and says where its packets come from; once taken, the host owes it its
    /// world.
    fn hello(&mut self, line: &str) {
        let engine = engine();
        let ours = [
            self.protocol(),
            self.app,
            &engine,
            &self.build,
            self.layers.as_str(),
        ];
        match port_of(line, ours, "the host") {
            Ok(port) => {
                self.owed = true;
                if let Some(link) = &mut self.link {
                    link.hello_came();
                    link.packets_from(port);
                }
            }
            Err(why) => {
                warn!(code = "net.refused", "Refused a lockstep peer: {why}");
                if let Some(link) = &mut self.link {
                    let _ = link.send(format!("refused {why}\n"));
                }
                self.drop_link();
            }
        }
    }

    /// The host, between ticks, the next to run being `next`: sends the joiner `world`, a snapshot
    /// file's text, which names its assets as the host does, there being `assets` of them, with the
    /// tick the joiner's inputs are needed from, how many patches its code is on and where its
    /// packets come from; and the host's inputs for each tick from `next` to that one, whole, its
    /// patches that are to apply among them, which it has sent before, or has none of within its
    /// delay of its start or restore. The joiner's inputs are needed from the first tick the host
    /// hasn't made its own for, its delay on from `next`, which the joiner's delay reaches, and
    /// those before are none: those it predicted that its world went by, which takes the joiner's
    /// place, as its ticks before `next` did, whatever the joiner's were.
    pub(crate) fn send_world(&mut self, world: &str, assets: Counts, next: u64) {
        self.owed = false;
        #[cfg(feature = "dev")]
        {
            self.named = assets;
        }
        #[cfg(not(feature = "dev"))]
        let _ = assets;
        #[cfg(feature = "dev")]
        let patches = self.patches.applied;
        #[cfg(not(feature = "dev"))]
        let patches = 0;
        let port = self.link.as_ref().map_or(0, Link::port);
        // As far as its inputs reach between ticks, or will once its ticks go on, after a restore.
        let from = next + self.delay;
        self.tagged = from;
        let mut text = format!("world {next} {from} {patches} {port}\n{world}");
        let none = Inputs::default();
        for tick in next..from {
            write_inputs(&mut text, tick, self.ours.get(&tick).unwrap_or(&none));
        }
        self.ours = self.ours.split_off(&next);
        self.theirs.clear();
        self.hashes = Default::default();
        self.release();
        self.parted = false;
        (self.from, self.whole, self.acked, self.since) = (Some(from), from, from, next);
        if let Some(link) = &mut self.link {
            link.joined = false;
            if let Err(error) = link.send(text) {
                self.lose(&format!("can't send the joiner the world: {error}"));
            } else {
                info!(tick = next, "Sent the joiner the world");
            }
        }
    }

    /// The joiner, as the last line of its host's world at tick `tick` comes, which takes its own
    /// world's place: refuses the edits it offered whose ticks it hasn't run, though its host may
    /// have run some of them, as DEFECTS.md has it; and drops the host's inputs and hashes it has,
    /// which its host sends again from `tick` after its world, as it does its patches that are to
    /// apply. The patches its host had applied apply before `tick`, if they haven't here.
    fn came(&mut self, tick: u64) {
        let ours = mem::take(&mut self.ours)
            .into_values()
            .chain(self.held.take());
        let sent = ours.flat_map(|inputs| inputs.edits.into_iter().chain(inputs.scenes));
        let sent = sent.map(|(ticket, _)| ticket);
        let offered = self.offered.drain(..).map(|(ticket, _)| ticket);
        let tickets: Vec<Ticket> = sent.chain(offered).flatten().collect();
        for ticket in tickets {
            let error = Error::new(
                "net.resynced",
                "the host's world took this peer's place before the edit applied",
            );
            self.done.push((ticket, tick, Err(error)));
        }
        self.theirs.clear();
        self.hashes = Default::default();
        self.release();
        self.parted = false;
        (self.from, self.whole, self.acked, self.since) = (Some(tick), tick, tick, tick);
        if let Some(link) = &mut self.link {
            link.joined = true;
        }
        (self.played, self.refused) = (true, None);
        // A joiner ahead of its host may have applied a patch at a tick after `tick`, which it runs
        // again on that code: their worlds may part until the host applies it too, and a resync
        // makes them the same.
        #[cfg(feature = "dev")]
        {
            let patches = &mut self.patches;
            patches.due.clear();
            if patches.hosts > patches.applied {
                patches.due.push((patches.hosts, tick));
            }
        }
    }

    /// The joiner's: the names its host gave the textures it added after its world, each with the
    /// path it names, which came since they were last taken.
    #[cfg(feature = "dev")]
    pub(crate) fn take_names(&mut self) -> Vec<(Arc<str>, Arc<str>)> {
        mem::take(&mut self.names)
    }

    /// The joiner, once it has restored its host's world at tick `tick`: says it has joined.
    pub(crate) fn restored(&mut self, tick: u64) {
        let Some(link) = &mut self.link else { return };
        if let Err(error) = link.send(format!("joined {tick}\n")) {
            self.lose(&format!("can't tell the host it joined: {error}"));
        }
    }

    /// Offers `edit`, which goes out with this peer's inputs for the tick its delay on from its
    /// next, and returns its ticket, which [`done`](Self::done) gives its result with.
    #[cfg(feature = "dev")]
    pub(crate) fn offer(&mut self, edit: Edit) -> Ticket {
        self.tickets += 1;
        self.offered.push((Some(self.tickets), Outside::Edit(edit)));
        self.tickets
    }

    /// Offers `edit` as [`offer`](Self::offer) does, answered to no one, as it logs what it did, as
    /// a script's reload does.
    #[cfg(feature = "dev")]
    pub(crate) fn offer_unanswered(&mut self, edit: Edit) {
        self.offered.push((None, Outside::Edit(edit)));
    }

    /// Offers the save of the scene file whose author is `author`, its text now `text`, as
    /// [`offer`](Self::offer) offers an edit, answered to no one, as it logs what it did.
    #[cfg(feature = "dev")]
    pub(crate) fn offer_save(&mut self, author: String, text: String) {
        self.offered.push((None, Outside::Scene { author, text }));
    }

    /// What the edit offered with `ticket` did, and the tick it applied before, once it has.
    #[cfg(feature = "dev")]
    pub(crate) fn done(&mut self, ticket: Ticket) -> Option<(u64, Result<Done>)> {
        let at = self.done.iter().position(|(of, ..)| *of == ticket)?;
        let (_, tick, done) = self.done.swap_remove(at);
        Some((tick, done))
    }

    /// Whether the edit offered with `ticket` has applied, or been refused.
    #[cfg(feature = "dev")]
    pub(crate) fn is_done(&self, ticket: Ticket) -> bool {
        self.done.iter().any(|(of, ..)| *of == ticket)
    }

    /// Keeps what the edit offered with `ticket` did, as it applied before tick `tick`.
    pub(crate) fn did(&mut self, ticket: Ticket, tick: u64, done: Result<Done>) {
        self.done.push((ticket, tick, done));
    }

    /// Whether tick `tick` can run: it has the patches due before it, which a joiner may have from
    /// its host before it has them from `gg run`; and it has the other peer's inputs, or needs
    /// none, or, in a rollback session, is within its window of the first it hasn't, and applies no
    /// edit or patch of this peer's.
    pub(crate) fn ready(&self, tick: u64) -> bool {
        if self.waiting() {
            return false;
        }
        #[cfg(feature = "dev")]
        {
            let patches = &self.patches;
            let mut due = patches.due.iter();
            if due.any(|&(number, at)| at <= tick && number > patches.got.len()) {
                return false;
            }
        }
        if self.settled(tick) {
            return true;
        }
        let authored = self.ours.get(&tick).is_some_and(|inputs| inputs.authored);
        tick < self.whole + self.window && !authored
    }

    /// Whether tick `tick` runs on both peers' inputs: the other's have come, or it needs none.
    pub(crate) fn settled(&self, tick: u64) -> bool {
        self.from.is_none_or(|_| tick < self.whole)
    }

    /// As a tick starts, at `now`, once the other peer has joined: has a lockstep peer's delay, or
    /// a rollback peer's window, follow the link, once its way there and back has been timed. The
    /// delay is the ticks the way one way takes, as [`Trip::ticks`] says, and one more, as a tick's
    /// inputs go out as it starts, from [`LEAST_DELAY`] to [`MOST_DELAY`]; the window those ticks
    /// and two more, from [`PREDICTED`] to [`MOST_PREDICTED`]. Each rises at once, and falls to
    /// the most the link has needed in the last [`FALLS`], once that's less, the delay or window it
    /// had as the way was first timed counting as needed then, so that a link's jitter doesn't have
    /// it fall at each lull and rise again: a window at once, as it bounds this peer's predictions
    /// alone, and a delay a tick at a time, never two ticks running, as a tick it falls holds its
    /// inputs back for the next's.
    fn follow_link(&mut self, now: Instant) {
        let Some(link) = self.link.as_ref().filter(|link| link.joined) else {
            return;
        };
        let rollback = self.window > 0;
        let had = if rollback { self.window } else { self.delay };
        let mut has = had;
        if let Some(ticks) = link.trip.ticks() {
            let wanted = if rollback {
                (ticks + 2).clamp(PREDICTED, MOST_PREDICTED)
            } else {
                (ticks + 1).clamp(LEAST_DELAY, MOST_DELAY)
            };
            if self.needs.is_empty() {
                self.needs.push_back((now, had));
            }
            // A need no more than this one can't be the most again while this one counts.
            while self.needs.back().is_some_and(|&(_, need)| need <= wanted) {
                self.needs.pop_back();
            }
            self.needs.push_back((now, wanted));
            while self
                .needs
                .front()
                .is_some_and(|&(when, _)| now.saturating_duration_since(when) > FALLS)
            {
                self.needs.pop_front();
            }
            // The latest, just pushed, still counts.
            let needed = self.needs[0].1;
            has = match (rollback, &self.held) {
                _ if needed >= had => needed,
                (true, _) => needed,
                (false, None) => had - 1,
                (false, Some(_)) => had,
            };
        }
        let what = if rollback { "window" } else { "delay" };
        if has != had {
            debug!(
                ticks = has,
                "The {what} went from {had} to {has} ticks, as the link's way there and back has it"
            );
        }
        if rollback {
            self.window = has;
        } else {
            self.delay = has;
        }
        let [least, most] = self.ranged.get_or_insert([has, has]);
        (*least, *most) = ((*least).min(has), (*most).max(has));
    }

    /// The time a frame `dt` after the last owes the peer's ticks, once the other peer has joined:
    /// less by [`PACE`] for each tick it's ahead of the other past the first, as its link has it,
    /// at most [`MOST_PACE`] less, so that neither runs far ahead of the other, as one that started
    /// first, or whose clock runs faster, would. A rollback peer ahead predicts further than the
    /// other, and a lockstep peer ahead waits for the other's inputs at a link's every jitter, as it
    /// takes the ticks owed meanwhile at once after.
    pub(crate) fn pace(&self, dt: Duration) -> Duration {
        let Some(link) = self.link.as_ref().filter(|link| link.joined) else {
            return dt;
        };
        let slower = ((link.ahead - 1.0) * PACE).clamp(0.0, MOST_PACE);
        dt.mul_f64(1.0 - slower)
    }

    /// Counts tick `tick` among those that waited for the other peer's inputs, once, if the other
    /// has joined: as it can't run, as [`ready`](Self::ready) says; the first starts a stall,
    /// which the next tick that runs ends.
    pub(crate) fn waited(&mut self, tick: u64) {
        if self.link.as_ref().is_some_and(|link| link.joined) && self.waited != Some(tick) {
            self.waits += 1;
            self.waited = Some(tick);
            self.stalls.waited(Instant::now());
        }
    }

    /// As tick `tick` starts, once it's [`ready`](Self::ready): has its delay or window follow the
    /// link, as [`follow_link`](Self::follow_link) says, and sends this peer's inputs for the tick
    /// its delay on, `queues`' commands, each layer's by its index, written against `world`, what
    /// it offered since its last tick, emptying them, and, the host's, the patches offered since it
    /// last sent its inputs, which apply before that tick; and lets go of the inputs it no longer
    /// needs. As the delay rises, the ticks it passes over go out holding none of this peer's
    /// inputs; as it falls, which it does a tick at a time, this tick's inputs are held back, as
    /// that tick's have gone, and go out with the next tick's.
    pub(crate) fn start_tick<'q>(
        &mut self,
        tick: u64,
        queues: impl Iterator<Item = (usize, &'q mut dyn Commands)>,
        world: &World,
    ) {
        self.next = tick;
        let now = Instant::now();
        self.stalls.end(now);
        self.follow_link(now);
        let ahead = tick + self.delay;
        // As the run starts, or its ticks jump, as a restore has them: none of this peer's inputs
        // went out for the ticks before.
        if !(tick..=ahead + 1).contains(&self.tagged) {
            self.tagged = ahead;
        }
        let holds = self.tagged > ahead;
        let mut inputs = self.held.take().unwrap_or_default();
        for (layer, queue) in queues {
            inputs.authored |= queue.edits() && !queue.is_empty();
            for i in 0..queue.len() {
                let mut value = String::new();
                text::write(&mut value, &queue.value(i), world);
                inputs.commands.push((layer, value));
            }
            queue.clear();
        }
        for (ticket, input) in mem::take(&mut self.offered) {
            inputs.authored = true;
            let mut value = String::new();
            match input {
                Outside::Edit(edit) => {
                    text::write(&mut value, &edit.to_value(), world);
                    inputs.edits.push((ticket, value));
                }
                Outside::Scene {
                    author,
                    text: scene,
                } => {
                    let strings = Value::List(vec![Value::String(author), Value::String(scene)]);
                    text::write(&mut value, &strings, world);
                    inputs.scenes.push((ticket, value));
                }
            }
        }
        #[cfg(feature = "dev")]
        if self.hosts() && !holds {
            for (number, at) in &mut self.patches.due {
                if *at == UNSENT {
                    *at = ahead;
                    inputs.patches.push(*number);
                }
            }
            inputs.authored |= !inputs.patches.is_empty();
        }
        // This peer's inputs are kept until the other has them and their tick has run on both
        // peers', and the other's until then.
        let settled = self
            .predicted
            .first_key_value()
            .map_or(tick, |(&first, _)| first);
        let had = if self.from.is_some() {
            self.acked
        } else {
            tick
        };
        self.ours = self.ours.split_off(&settled.min(had));
        self.theirs = self.theirs.split_off(&settled);
        if holds {
            self.held = Some(inputs);
            self.send_packets();
            return;
        }
        for skipped in self.tagged..ahead {
            self.ours.insert(skipped, Inputs::default());
        }
        self.tagged = ahead + 1;
        if self.from.is_some() {
            let mut text = String::new();
            // The names of the assets the host added since it last told them, before the inputs
            // that may name them.
            #[cfg(feature = "dev")]
            if self.hosts() {
                for (name, path) in world.assets().named_from(self.named) {
                    text.push_str("n ");
                    text::write_named(&mut text, name, path);
                    text.push('\n');
                }
                self.named = world.assets().counts();
            }
            // Each line, its tick and its layer, and the packet's first and last, at most.
            let lines = inputs
                .commands
                .iter()
                .map(|(_, command)| command.len() + 32);
            let size = 64 + lines.sum::<usize>();
            inputs.streamed = inputs.authored || !text.is_empty() || size > MAX_PACKET_TEXT;
            if inputs.streamed {
                write_inputs(&mut text, ahead, &inputs);
                self.send(text);
            }
        }
        self.ours.insert(ahead, inputs);
        self.send_packets();
    }

    /// What tick `tick` runs on: both peers' inputs, the host's first, the other's predicted to
    /// hold nothing until they've come; and, once they have, as the tick runs on both peers' inputs,
    /// once, what applies before it, the edits read once the textures they name are loaded, as a
    /// run that follows another loads them, and the patches due before it.
    pub(crate) fn inputs(&mut self, tick: u64, cx: &mut Context) -> Merged {
        let (settled, hosts) = (self.settled(tick), self.hosts());
        let ours = self.ours.get_mut(&tick);
        let theirs = self.theirs.get_mut(&tick).filter(|_| settled);
        let peers = if hosts {
            [ours, theirs]
        } else {
            [theirs, ours]
        };
        let mut merged = Merged::default();
        for (player, peer) in (0..).zip(peers) {
            let Some(peer) = peer else {
                continue;
            };
            merged.commands.extend(Queued::of(
                peer.commands.iter().cloned(),
                Some(player),
                false,
            ));
            if !settled {
                continue;
            }
            for (ticket, edit) in mem::take(&mut peer.edits) {
                let read = Text::parse(&edit, 1).and_then(|text| {
                    cx.load_followed(&text);
                    Edit::from_value(&text.value(cx.world()))
                });
                match read {
                    Ok(edit) => merged.inputs.push((ticket, Outside::Edit(edit))),
                    Err(error) => {
                        warn!(code = "net.bad_command", "A peer's edit isn't one: {error}");
                        if let Some(ticket) = ticket {
                            self.done.push((ticket, tick, Err(error)));
                        }
                    }
                }
            }
            for (ticket, scene) in mem::take(&mut peer.scenes) {
                let read = Text::parse(&scene, 1).map(|text| text.value(cx.world()));
                let saved = match &read {
                    Ok(Value::List(strings)) => match &strings[..] {
                        [Value::String(author), Value::String(text)] => {
                            Some((author.clone(), text.clone()))
                        }
                        _ => None,
                    },
                    _ => None,
                };
                match saved {
                    Some((author, text)) => {
                        let save = Outside::Scene { author, text };
                        merged.inputs.push((ticket, save));
                    }
                    None => warn!(
                        code = "net.bad_command",
                        "A peer's scene file isn't one: {scene}"
                    ),
                }
            }
        }
        #[cfg(feature = "dev")]
        if settled {
            self.due_patches(tick, &mut merged);
        }
        merged
    }

    /// Keeps a copy of `world` as tick `tick` starts on the other peer's inputs predicted, which
    /// rollback runs it again from if theirs hold something, in the memory of the copy kept before.
    pub(crate) fn keep(&mut self, tick: u64, world: &World) {
        let spares = &mut self.spares;
        let predicted = self.predicted.entry(tick).or_insert_with(|| Predicted {
            start: spares.pop().unwrap_or_default(),
            played: Vec::new(),
        });
        predicted.start.clone_from(world);
    }

    /// The sounds the runs of tick `tick` on the other peer's inputs predicted have played, while
    /// it hasn't run on theirs.
    pub(crate) fn played(&mut self, tick: u64) -> Option<&mut Vec<Sound>> {
        let predicted = self.predicted.get_mut(&tick)?;
        Some(&mut predicted.played)
    }

    /// Between ticks, the next to run being `next`, `world` as the last left it: takes the ticks run
    /// on the other peer's inputs predicted whose inputs have come since, in turn. One theirs held
    /// nothing for, as predicted, ran as it would have on them, and ends as a tick run on them
    /// does, as [`end_tick`](Self::end_tick) says. The first theirs held something for is returned,
    /// for the world to be taken back to where it started, [`start_of`](Self::start_of), and it
    /// and the ticks after run again: once the patches due before `next` have come from `gg run`.
    pub(crate) fn settle(&mut self, next: u64, world: &World) -> Option<u64> {
        #[cfg(feature = "dev")]
        {
            let patches = &self.patches;
            let mut due = patches.due.iter();
            if due.any(|&(number, at)| at < next && number > patches.got.len()) {
                return None;
            }
        }
        #[cfg(not(feature = "dev"))]
        let _ = next;
        while let Some((&tick, _)) = self.predicted.first_key_value()
            && tick < self.whole
        {
            if self.theirs.get(&tick).is_some_and(Inputs::any) {
                return Some(tick);
            }
            let predicted = self.predicted.remove(&tick).expect("it's the first");
            self.spares.push(predicted.start);
            // The world after it: as the tick after started, or as it is, that tick yet to run.
            let after = self.predicted.remove(&(tick + 1));
            self.finish(tick, after.as_ref().map_or(world, |after| &after.start));
            match after {
                Some(after) if self.link.is_some() => {
                    self.predicted.insert(tick + 1, after);
                }
                _ => {}
            }
            if self.link.is_none() {
                break;
            }
        }
        None
    }

    /// The world as the first tick run on the other peer's inputs predicted, whose inputs haven't
    /// come, started: the world after the last tick that's final, which no rollback takes back;
    /// `None` while no tick runs on them, the world as it stands the one.
    pub(crate) fn final_world(&self) -> Option<&World> {
        let (_, first) = self.predicted.first_key_value()?;
        Some(&first.start)
    }

    /// The world as tick `tick`, run on the other peer's inputs predicted, started.
    ///
    /// # Panics
    ///
    /// If it hasn't run on them, or has run on theirs since.
    pub(crate) fn start_of(&self, tick: u64) -> &World {
        &self.predicted[&tick].start
    }

    /// Counts a rollback, from tick `from` to `next`, the next to run, the ticks between run again.
    pub(crate) fn rolled_back(&mut self, from: u64, next: u64) {
        let ticks = next - from;
        let [rollbacks, again, most] = &mut self.rolled;
        *rollbacks += 1;
        *again += ticks;
        *most = (*most).max(ticks);
        debug!(tick = from, ticks, "Rolled back");
    }

    /// Logs, as the run ends, what its link to the other peer sent and took, if it has one, as
    /// [`Link::ended`] says; if the other peer joined, the least and the most its delay or its
    /// window were, how many ticks waited for the other's inputs, and its stalls as they did, as
    /// [`Stalls`] says (`net.delay`); and, in a rollback session, how many times the peer rolled
    /// back, how many ticks it ran again, and the most at once (`net.rollback`).
    pub(crate) fn ended(&self) {
        if let Some(link) = &self.link {
            link.ended("the other peer");
        }
        if let Some([least, most]) = self.ranged {
            let (what, waits) = (if self.window > 0 { "window" } else { "delay" }, self.waits);
            let (stalls, stalled_ms, longest_stall_ms) = self.stalls.logged();
            info!(
                code = "net.delay",
                least,
                most,
                waits,
                stalls,
                stalled_ms,
                longest_stall_ms,
                "The {what} ranged from {least} to {most} ticks as the link needed, and {waits} \
                 ticks waited for the other peer's inputs, in {stalls} stalls of {stalled_ms} ms \
                 together, the longest {longest_stall_ms} ms"
            );
        }
        if self.window == 0 {
            return;
        }
        let [rollbacks, ticks, most] = self.rolled;
        info!(
            code = "net.rollback",
            rollbacks,
            ticks,
            most,
            "Rolled back {rollbacks} times, as the other peer's inputs held what was predicted they \
             wouldn't, running {ticks} ticks again, at most {most} at once"
        );
    }

    /// Once tick `tick` has run on both peers' inputs, leaving `world`, as it does once: lets go of
    /// its runs on the other's predicted, and ends it, as [`finish`](Self::finish) says.
    pub(crate) fn end_tick(&mut self, tick: u64, world: &World) {
        if let Some(predicted) = self.predicted.remove(&tick) {
            self.spares.push(predicted.start);
        }
        self.finish(tick, world);
    }

    /// Ends tick `tick`, which has run on both peers' inputs, leaving `world`: sends its hash, if
    /// there's another peer, keeping a copy of the world until the hash is compared with theirs,
    /// unless the peers have parted, and compares it.
    fn finish(&mut self, tick: u64, world: &World) {
        if self.from.is_none() {
            return;
        }
        if !self.parted {
            let mut kept = self.spares.pop().unwrap_or_default();
            kept.clone_from(world);
            self.worlds.insert(tick, kept);
        }
        let hash = world.hash();
        self.send(format!("h {tick} {hash}\n"));
        self.hashes[0].insert(tick, hash);
        self.compare();
    }

    /// Compares the hashes both peers have after the same tick, dropping them and the worlds kept
    /// with this peer's, unless the peers have parted; a pair that differs logs `net.desync`, and
    /// has the host send its joiner its world after that tick, which the joiner keeps its own after
    /// until it comes, then its world again.
    fn compare(&mut self) {
        if self.parted {
            self.hashes = Default::default();
            return;
        }
        let hosts = self.hosts();
        let mut parted = None;
        let [ours, theirs] = &mut self.hashes;
        while let (Some((&a, _)), Some((&b, _))) =
            (ours.first_key_value(), theirs.first_key_value())
        {
            if a != b {
                // One peer's hashes run ahead of the other's: drop the older, unmatched.
                if a < b {
                    ours.pop_first();
                } else {
                    theirs.pop_first();
                }
                continue;
            }
            let (_, mine) = ours.pop_first().expect("it has a first");
            let (_, other) = theirs.pop_first().expect("it has a first");
            if mine != other {
                error!(
                    code = "net.desync",
                    tick = a,
                    "The peers' worlds went apart at tick {a}: this one's hashes {mine}, the \
                     other's {other}; the host sends its world again"
                );
                self.owed = hosts;
                self.parted = true;
                ours.clear();
                theirs.clear();
                parted = Some(a);
                break;
            }
        }
        // The worlds kept with the hashes dropped, but the one after the tick the peers parted at.
        let first = self.hashes[0].first_key_value().map(|(&tick, _)| tick);
        let mut kept = None;
        while let Some(world) = self.worlds.first_entry()
            && first.is_none_or(|first| *world.key() < first)
        {
            let (tick, world) = world.remove_entry();
            if parted == Some(tick) {
                kept = Some(world);
            } else {
                self.spares.push(world);
            }
        }
        if let (Some(tick), Some(world)) = (parted, kept) {
            if hosts {
                // Dropped after, as sending it may lose the joiner, its spares with it.
                self.send_parted(tick, &world);
            } else {
                self.worlds.insert(tick, world);
            }
        }
    }

    /// The host, as its hashes part from its joiner's after tick `tick`: sends the joiner `world`,
    /// its own after that tick, which the joiner names where they part by.
    fn send_parted(&mut self, tick: u64, world: &World) {
        let mut text = format!("parted {tick}\n");
        match world.save_whole(&mut text) {
            Ok(()) => {
                text.push_str("end\n");
                self.send(text);
            }
            Err(error) => warn!(
                code = error.code(),
                "Can't send the joiner the world where theirs parted from it: {error:#}"
            ),
        }
    }

    /// The joiner, as its host's world after tick `tick`, where their hashes parted, has come whole
    /// as `text`: logs the first entity, component and field where its own world after that tick
    /// differs, as [`World::difference`] finds it (`net.desync_field`); and, where a cell's file
    /// came in from another file than the host's, as [`World::cell_file_parted`] finds it, names
    /// it (`net.cell_file`) and leaves the session, to try again later, as a refused hello does.
    fn name_parting(&mut self, tick: u64, text: &str) {
        // None after a tick the joiner hasn't kept a world after, as one it didn't part at.
        let Some(ours) = self.worlds.remove(&tick) else {
            return;
        };
        // Into a copy of its own, of the same types and assets. What's opaque comes back as the
        // host's world names it, which a world never drawn and compared as the same takes.
        let mut theirs = ours.clone();
        // Numbered as lines of the message, after its `parted`.
        let lines = (2..).zip(text.lines().take_while(|line| *line != "end"));
        match theirs.load_whole(lines, true) {
            Ok(_) => {
                log_parting(tick, &ours, &theirs);
                if let Some((file, joiner, host)) = ours.cell_file_parted(&theirs) {
                    error!(
                        code = "net.cell_file",
                        tick,
                        file = &*file,
                        joiner = %format_args!("{joiner:016x}"),
                        host = %format_args!("{host:016x}"),
                        "The peers' worlds parted after tick {tick} as the cell file {file} came \
                         in, which isn't the host's here: the joiner leaves, and tries again in {} \
                         s; give both peers the same files",
                        REFUSED.as_secs()
                    );
                    self.lose(&format!("its cell file {file} isn't the host's"));
                    self.find.again();
                }
            }
            Err(error) => warn!(
                code = error.code(),
                tick,
                "Can't read the host's world after tick {tick}, where the peers parted: {error:#}"
            ),
        }
        self.spares.push(ours);
    }

    /// Puts the worlds kept among the spares, as the hashes they were kept for are dropped, and
    /// those of the ticks run on the other peer's inputs predicted, as the world the peers go on
    /// from takes their place.
    fn release(&mut self) {
        let kept = mem::take(&mut self.worlds).into_values();
        self.spares.extend(kept);
        let predicted = mem::take(&mut self.predicted).into_values();
        self.spares
            .extend(predicted.map(|predicted| predicted.start));
    }

    /// Whether the other peer's inputs may be for tick `tick`, as [`AHEAD`] says.
    fn near(&self, tick: u64) -> bool {
        tick < self.whole + AHEAD
    }

    /// Moves on past the ticks whose inputs from the other peer have come whole, as their last
    /// line or their packet has.
    fn came_whole(&mut self) {
        while self
            .theirs
            .get(&self.whole)
            .is_some_and(|inputs| inputs.whole)
        {
            self.whole += 1;
        }
    }

    /// Takes `packet`, from the other peer, which came at `now`, once the link counts: the times
    /// it says, which time the way there and back; how far the other is from this peer in their
    /// ticks; how far it has this peer's inputs, and its own for each tick it holds that haven't
    /// come whole. Loses the peer for a packet that isn't one.
    fn read_packet(&mut self, packet: &[u8], now: Instant) {
        if !self.link.as_ref().is_some_and(|link| link.joined) {
            return;
        }
        if self.read_ticks(packet, now).is_none() {
            self.lose("the peer sent a packet that isn't one");
        }
        self.came_whole();
    }

    /// Reads `packet`'s lines, as [`read_packet`](Self::read_packet) takes them, failing at one
    /// that isn't one, or at a tick's inputs past those the other peer may send, as [`AHEAD`]
    /// says, those before taken.
    fn read_ticks(&mut self, packet: &[u8], now: Instant) -> Option<()> {
        let mut lines = std::str::from_utf8(packet).ok()?.lines();
        let mut words = lines.next()?.strip_prefix("a ")?.split(' ');
        let (since, acked, next) = (
            tick_of(words.next()?)?,
            tick_of(words.next()?)?,
            tick_of(words.next()?)?,
        );
        let past = optional::<i64>(words.next()?)?;
        let clock = words.next()?.parse::<u64>().ok()?;
        let (echo, held) = (optional(words.next()?)?, optional(words.next()?)?);
        if words.next().is_some() {
            return None;
        }
        if let Some(link) = &mut self.link {
            link.timed(clock, echo.zip(held), now);
            // Each peer's count is the ticks it's ahead and the way's: their difference is twice
            // the ticks it's ahead.
            let ours = self.next as i64 - next as i64;
            link.past = Some(ours);
            if let Some(theirs) = past {
                let ahead = ours.saturating_sub(theirs) as f64 / 2.0;
                link.ahead += (ahead - link.ahead) / 16.0;
            }
        }
        // Sent before the peers' worlds were last made the same.
        if since != self.since {
            return Some(());
        }
        self.acked = self.acked.max(acked);
        let (whole, theirs) = (self.whole, &mut self.theirs);
        let mut near = true;
        read_batches(lines, |tick, commands| {
            near &= tick < whole + AHEAD;
            // Come before, or whole on the stream; what came of them there is the same.
            if tick >= whole && near {
                let inputs = theirs.entry(tick).or_default();
                if !inputs.whole {
                    *inputs = Inputs {
                        commands,
                        whole: true,
                        ..Inputs::default()
                    };
                }
            }
        })?;
        near.then_some(())
    }

    /// Sends the other peer this peer's packets, once there is one and it has said where its
    /// packets come from: how far this peer has its inputs, its next tick and how far it was past
    /// the other's as the other's latest packet came, and the times, as [`Link::write_times`]
    /// says; and this peer's inputs for each tick from the first the other hasn't said it has that
    /// didn't go on the stream, oldest first, as many to a packet as fit [`MAX_PACKET_TEXT`], a
    /// tick's in one.
    fn send_packets(&mut self) {
        if self.from.is_none() {
            return;
        }
        let Some(link) = &mut self.link else { return };
        let now = Instant::now();
        let mut header = format!("a {} {} {} ", self.since, self.whole, self.next);
        match link.past {
            Some(past) => write!(header, "{past} ").expect("a string takes any text"),
            None => header.push_str("- "),
        }
        link.write_times(&mut header, now);
        header.push('\n');
        let (mut packet, mut lines) = (header.clone(), String::new());
        let sent = (self.ours.range(self.acked..)).filter(|(_, inputs)| !inputs.streamed);
        for (&tick, inputs) in sent {
            lines.clear();
            write_inputs(&mut lines, tick, inputs);
            if packet.len() + lines.len() > MAX_PACKET_TEXT && packet.len() > header.len() {
                link.send_packet(packet.as_bytes());
                packet.clone_from(&header);
            }
            packet.push_str(&lines);
        }
        // The last, or the header alone, which says how far this peer has the other's inputs.
        link.send_packet(packet.as_bytes());
        link.sent = Some(now);
    }

    /// Sends `text` to the other peer, losing it if that fails.
    fn send(&mut self, text: String) {
        let Some(link) = &mut self.link else { return };
        if let Err(error) = link.send(text) {
            self.lose(&format!("can't send to the peer: {error}"));
        }
    }

    /// Loses the other peer, as `why` says: the host plays on alone, and the joiner waits for its
    /// host's world again.
    pub(crate) fn lose(&mut self, why: &str) {
        if self.link.is_some() {
            warn!(code = "net.lost", "Lost the lockstep peer: {why}");
        }
        self.drop_link();
    }

    /// Lets go of the link to the other peer, logging what it sent and took, as [`Link::ended`]
    /// says, and ending the stall going on, if one is, and what came by it.
    fn drop_link(&mut self) {
        if let Some(link) = self.link.take() {
            link.ended("the other peer");
        }
        self.stalls.end(Instant::now());
        self.coming = None;
        self.parting = None;
        self.owed = false;
        self.parted = false;
        self.theirs.clear();
        self.hashes = Default::default();
        // Their memory too, as a host may play on alone, its ticks on the joiner's inputs
        // predicted going on as they ran.
        self.worlds.clear();
        self.spares.clear();
        self.predicted.clear();
        if self.hosts() {
            self.from = None;
        }
    }
}

#[cfg(feature = "dev")]
impl Lockstep {
    /// Offers the patch `table` names, `gg run`'s next for this process, and returns its ticket,
    /// which [`done`](Self::done) gives its result with. The host has it apply before the tick its
    /// next inputs go out for, its delay on, which neither peer has run, among those inputs, which
    /// tell its joiner, which has it apply then too; a joiner has it apply as its host says, or
    /// before the first tick of its host's world, if its host had applied it.
    pub(crate) fn offer_patch(&mut self, table: subsecond::JumpTable) -> Ticket {
        self.tickets += 1;
        self.patches.got.push(Some((table, self.tickets)));
        if self.hosts() {
            self.patches.due.push((self.patches.got.len(), UNSENT));
        }
        self.tickets
    }

    /// Moves the patches due before tick `tick` into `merged`, each in turn, answering those before
    /// it that haven't applied as applied, as it holds them.
    fn due_patches(&mut self, tick: u64, merged: &mut Merged) {
        let patches = &mut self.patches;
        while let Some(&(number, at)) = patches.due.first()
            && at <= tick
            && number <= patches.got.len()
        {
            patches.due.remove(0);
            if number <= patches.applied {
                continue;
            }
            for held in &mut patches.got[patches.applied..number - 1] {
                if let Some((_, ticket)) = held.take() {
                    self.done
                        .push((ticket, tick, Ok(Done::Applied(Vec::new()))));
                }
            }
            if let Some((table, ticket)) = patches.got[number - 1].take() {
                merged.patches.push((ticket, table));
            }
            patches.applied = number;
        }
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
            Self::Client(client) => client.corrected(from, next),
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

/// What a session's hello names the run's build by, so that a host or a server refuses a peer or a
/// client of another build, whose simulation may go otherwise, as one rebuilt with an edit's
/// would: the source its code is compiled from, as [`App::source`](crate::App::source) names it,
/// and the package it reads.
#[derive(Clone, Default)]
pub(crate) struct Build {
    /// The source's digest, as `App::source` names it.
    pub(crate) source: Option<&'static str>,
    /// Where the run reads its files: its package, if it reads one.
    pub(crate) files: Files,
}

impl Build {
    /// The build, as the hello names it: for an app that names its source, the source's digest
    /// and the run's package's, as [`Package::digest`](crate::package::Package::digest) has it,
    /// each 16 hex digits, `-` between them, as in `5c7cd8abeeed5f44-641dce7a72446a9c`, the
    /// package's `-` for a run that reads its files from the disk, which the hello doesn't name;
    /// for an app that names none, its executable's digest, as [`executable`] says, which builds
    /// of one source on other OSes or machines don't share.
    ///
    /// Fails with `net.build` if the package or the executable can't be read.
    pub(crate) fn name(&self) -> Result<String> {
        let Some(source) = self.source else {
            return executable().map(str::to_owned);
        };
        let package = match self.files.packaged() {
            Some(package) => {
                let digest = package.digest().map_err(|error| {
                    let why = "can't digest the run's package, which the hello names its build by";
                    Error::new("net.build", why).with_source(error)
                })?;
                format!("{digest:016x}")
            }
            None => "-".to_owned(),
        };
        Ok(format!("{source}-{package}"))
    }
}

/// The digest of the run's executable's bytes, as 16 hex digits, read once a process, which names
/// the build of an app that names no source, as [`Build::name`] says. Builds of one source on two
/// OSes differ, and on two machines, as rustc writes the paths of the sources and the debug info's
/// into the executable.
///
/// Fails with `net.build` if the executable can't be read.
fn executable() -> Result<&'static str> {
    static BUILD: OnceLock<std::result::Result<String, String>> = OnceLock::new();
    let build = BUILD.get_or_init(|| {
        let _zone = gg_core::zone!();
        // Linux's link to the executable, which reads even once its file is deleted, as a run's
        // can be while it runs.
        let path = if cfg!(target_os = "linux") {
            PathBuf::from("/proc/self/exe")
        } else {
            std::env::current_exe().map_err(|error| error.to_string())?
        };
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("can't read {}: {error}", path.display()))?;
        Ok(format!("{:016x}", crate::ecs::bytes_digest(&bytes)))
    });
    build.as_deref().map_err(|why| {
        Error::new(
            "net.build",
            format!("can't digest the run's executable, which the hello names its build by: {why}"),
        )
    })
}

/// What the hello says the engine is: its version and profile, and whether it's a development
/// build, which applies patches and scene files' saves, as other builds don't.
fn engine() -> String {
    let engine = crate::recording::engine();
    if cfg!(feature = "dev") {
        format!("{engine} dev")
    } else {
        engine
    }
}

/// The hello of a peer or a client that speaks `protocol`, of app `app`, build `build` and layers
/// `layers`, whose packets come from its socket's `port`: as the stream's lines name it.
fn hello(protocol: &str, app: &str, build: &str, layers: &str, port: u16) -> String {
    format!(
        "hello\t{protocol}\t{app}\t{}\t{build}\t{layers}\t{port}\n",
        engine()
    )
}

/// Takes a hello, `line`, which [`hello`] writes: the port its packets come from, if it's of the
/// protocol, app, engine, build and layers `ours` are, which `who` speaks for; or else why it's
/// refused.
fn port_of(line: &str, ours: [&str; 5], who: &str) -> std::result::Result<u16, String> {
    let parts: Vec<&str> = line.split('\t').collect();
    let port = parts.get(6).and_then(|port| port.parse::<u16>().ok());
    match (parts.get(1..6), port) {
        (Some(theirs), Some(port)) if parts.len() == 7 && theirs == &ours[..] => Ok(port),
        (Some([protocol, ..]), _) if *protocol != ours[0] => {
            Err(format!("it speaks {protocol}, and {who} {}", ours[0]))
        }
        (Some([_, app, engine, build, layers]), Some(_)) if parts.len() == 7 => Err(format!(
            "it's app {app}, engine {engine}, build {build}, layers `{layers}`, and {who} app {}, \
             engine {}, build {}, layers `{}`",
            ours[1], ours[2], ours[3], ours[4]
        )),
        _ => Err("its hello isn't one".to_owned()),
    }
}

/// Takes `what` where `at` says: listening at its address, on a thread of its own that accepts
/// each connection, and registering at its relay, on another, as [`register`] says; returns where
/// they come, each through its handshake keyed by `code`, as [`Accepted`] says, and the address
/// listened at as the OS has it, which a test's port 0 picks.
///
/// Fails with `net.start` if it can't listen there, or start either thread.
fn listen(at: &Listen, what: &'static str, code: Code) -> Result<(Accepted, Option<SocketAddr>)> {
    let (shaken, handshaken) = mpsc::channel();
    let session = Arc::new(());
    let heard = Heard::default();
    let started = |error: io::Error| {
        let message = format!("can't start the thread that takes {what}");
        Error::new("net.start", message).with_source(error)
    };
    let mut listening = None;
    if let Some(address) = &at.address {
        let failed = |error: io::Error| {
            let message = format!("can't listen at {address} for {what}");
            Error::new("net.start", message).with_source(error)
        };
        let listener = TcpListener::bind(address).map_err(failed)?;
        listening = Some(listener.local_addr().map_err(failed)?);
        let (shaken, kept) = (shaken.clone(), Arc::downgrade(&session));
        thread::Builder::new()
            .name("gg-net-accept".to_owned())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    if kept.strong_count() == 0 {
                        break;
                    }
                    let shaken = shaken.clone();
                    handshake_on_its_own(move || {
                        let _ = shaken.send(shake(stream, &code, true, None));
                    });
                }
            })
            .map_err(started)?;
    }
    if let Some(relay) = &at.relay {
        let (relay, kept, heard) = (relay.clone(), Arc::downgrade(&session), heard.clone());
        thread::Builder::new()
            .name("gg-net-relay".to_owned())
            .spawn(move || register(&relay, what, code, &shaken, &kept, &heard))
            .map_err(started)?;
    }
    let accepted = Accepted {
        handshaken,
        heard,
        _session: session,
    };
    Ok((accepted, listening))
}

/// Runs `handshake` on a thread of its own, so that a connection slow to say its part holds no
/// other.
fn handshake_on_its_own(handshake: impl FnOnce() + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("gg-net-handshake".to_owned())
        .spawn(handshake);
    if let Err(error) = spawned {
        warn!(
            code = "net.link",
            "Can't start a connection's handshake: {error}"
        );
    }
}

/// Registers the session's room, which `code` gives, at the relay at `relay`, for `what`, and takes
/// each that comes for it, each through its handshake on a thread of its own, into `shaken`, until
/// the session, which `kept` holds, is gone. Registers again [`REGISTER`] after the relay can't be
/// reached, refuses it, as it does while a host it lost holds the room, or is lost: logged once
/// each time, `net.relay`, until it's registered again. Notes in `heard` whether the room is
/// registered, or why not.
fn register(
    relay: &str,
    what: &str,
    code: Code,
    shaken: &Sender<Handshaken>,
    kept: &Weak<()>,
    heard: &Heard,
) {
    let room = code.room();
    let mut warned = false;
    while kept.strong_count() > 0 {
        let registered = resolve(relay).and_then(|at| Ok((at, Hosting::start(at, room)?)));
        let lost = match registered {
            Ok((at, mut hosting)) => {
                info!(relay, "Registered the session at its relay, for {what}");
                heard.set(Answer::Hosted);
                warned = false;
                loop {
                    if kept.strong_count() == 0 {
                        return;
                    }
                    match hosting.joiner() {
                        Ok(None) => {}
                        Ok(Some(ticket)) => {
                            let shaken = shaken.clone();
                            handshake_on_its_own(move || {
                                let made = gg_relay::take(at, &ticket).and_then(|paired| {
                                    let relayed = Relayed {
                                        relay: at,
                                        token: paired.token,
                                    };
                                    shake(paired.stream, &code, true, Some(relayed))
                                });
                                let _ = shaken.send(made);
                            });
                        }
                        Err(error) => break format!("lost it: {error}"),
                    }
                }
            }
            Err(error) => format!("can't register there: {error}"),
        };
        heard.set(Answer::Unreachable(lost.as_str().into()));
        if !mem::replace(&mut warned, true) {
            let again = REGISTER.as_secs();
            warn!(
                code = "net.relay",
                relay, "The session's relay {relay}: {lost}; registering again every {again} s"
            );
        }
        sleep_while_kept(REGISTER, kept);
    }
}

/// Sleeps `time`, in steps of 100 ms, while `kept`'s session lasts; returns whether it still does.
fn sleep_while_kept(time: Duration, kept: &Weak<()>) -> bool {
    let wake = Instant::now() + time;
    while kept.strong_count() > 0 && Instant::now() < wake {
        let left = wake.saturating_duration_since(Instant::now());
        thread::sleep(left.min(Duration::from_millis(100)));
    }
    kept.strong_count() > 0
}

/// What a session's relay last answered the thread that registers its room there, or comes for
/// it, shared with that thread, for the session's status to say, as [`SessionPhase`] does.
#[derive(Clone, Default)]
pub(crate) struct Heard(Arc<Mutex<Answer>>);

impl Heard {
    fn set(&self, answer: Answer) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = answer;
    }

    fn get(&self) -> Answer {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// What a relay answered, as [`Heard`] keeps it.
#[derive(Clone, Default)]
pub(crate) enum Answer {
    /// Nothing yet, or the session has no relay.
    #[default]
    Nothing,
    /// The host's room is registered; or the joiner's, which its host holds, paired it.
    Hosted,
    /// The joiner's room, which no host holds yet.
    Unhosted,
    /// The relay can't be reached, refused the room or lost it, saying why.
    Unreachable(Arc<str>),
}

/// What a joiner or a client keeps while it looks for its host or server, as [`connect`] does:
/// what tells the threads that connect it that the session is gone, which they end at, and what
/// the relay last answered them.
#[derive(Default)]
pub(crate) struct Looking {
    session: Arc<()>,
    heard: Heard,
}

/// The address `address` names, as `host:port` or `ip:port`, as [`preferred`] picks it.
///
/// Fails if it names none.
fn resolve(address: &str) -> io::Result<SocketAddr> {
    let addresses: Vec<SocketAddr> = address.to_socket_addrs()?.collect();
    preferred(&addresses).ok_or_else(|| {
        let why = format!("{address} names no address");
        io::Error::new(ErrorKind::NotFound, why)
    })
}

/// The first IPv4 address of `addresses`, else the first: a relay listens on IPv4 by default, and
/// a link's socket follows its connection's family, so that a host name with an IPv6 address too,
/// which a resolver may list first, would otherwise reach none.
fn preferred(addresses: &[SocketAddr]) -> Option<SocketAddr> {
    let v4 = addresses.iter().find(|address| address.is_ipv4());
    v4.or(addresses.first()).copied()
}

/// A connection whose handshake held: its stream, the keys it made, and, for one made through a
/// relay, where the link's socket binds there; or why its handshake failed.
type Handshaken = io::Result<Shaken>;

struct Shaken {
    stream: TcpStream,
    keys: Keys,
    relayed: Option<Relayed>,
}

/// Where a link through a relay binds its socket: the relay's address, and the token the relay
/// gave its end of the pair.
#[derive(Clone, Copy)]
struct Relayed {
    relay: SocketAddr,
    token: Token,
}

/// The connections a host or a server accepts, as [`listen`] gives them, each once its handshake
/// has ended, which a thread of the connection's own runs, as the thread that accepts it starts
/// it: so that one slow to say its part holds neither the main thread, nor another, nor a
/// joiner's place before it has shown it holds the code, and a main thread busy awhile holds no
/// joiner's handshake. And what tells the thread that accepts them that the session is gone, which
/// it ends at as the next connection comes.
struct Accepted {
    handshaken: Receiver<Handshaken>,
    /// What the relay last answered the thread that registers the session's room, if it has one.
    heard: Heard,
    _session: Arc<()>,
}

impl Accepted {
    /// The next connection whose handshake has ended, if one has.
    fn handshaken(&self) -> Option<Handshaken> {
        self.handshaken.try_recv().ok()
    }
}

/// Runs the handshake on `stream`, keyed by `code`, as the end that made the connection or,
/// `accepted`, the end that took it, as [`secure::handshake`] says, waiting at most [`HELLO`]
/// for the other end's part; returns the stream with the keys it made, and where it's `relayed`.
fn shake(
    mut stream: TcpStream,
    code: &Code,
    accepted: bool,
    relayed: Option<Relayed>,
) -> Handshaken {
    stream.set_read_timeout(Some(HELLO))?;
    stream.set_write_timeout(Some(HELLO))?;
    match secure::handshake(&mut stream, code, accepted) {
        Ok(keys) => Ok(Shaken {
            stream,
            keys,
            relayed,
        }),
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            let secs = HELLO.as_secs();
            let why = format!("its part of the handshake didn't come within {secs} s");
            Err(io::Error::new(error.kind(), why))
        }
        Err(error) => Err(error),
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

/// Connects to `what` where `toward` says, on a thread of its own, after `after`: to its address,
/// trying again every [`RETRY`] until it does, or through its relay, coming for the room `code`
/// gives there, again every [`UNHOSTED`] until its host takes it, as [`gg_relay::join`] says,
/// noting each answer in `looking`'s [`Heard`], and logging as it starts through a relay, so that
/// a join's time reads from its log. Then runs the handshake keyed by `code` on the connection, as
/// [`shake`] does; returns where the connection comes, with its keys, or why its handshake failed.
/// The thread ends, connecting no more, once `looking` is gone, as its session is.
fn connect(
    toward: Toward,
    what: &'static str,
    after: Duration,
    code: Code,
    looking: &Looking,
) -> Receiver<Handshaken> {
    let (made, connection) = mpsc::channel();
    let (kept, heard) = (Arc::downgrade(&looking.session), looking.heard.clone());
    let spawned = thread::Builder::new()
        .name("gg-net-connect".to_owned())
        .spawn(move || {
            if !sleep_while_kept(after, &kept) {
                return;
            }
            if let Toward::Relay(relay) = &toward {
                info!(relay = %relay, "Looking for {what} through its relay");
            }
            let mut warned = false;
            loop {
                let (shaken, wait) = match &toward {
                    Toward::Address(address) => {
                        let stream = address.to_socket_addrs().ok().and_then(|mut addresses| {
                            addresses.find_map(|at| TcpStream::connect(at).ok())
                        });
                        let shaken = stream.map(|stream| shake(stream, &code, false, None));
                        if shaken.is_none() && !mem::replace(&mut warned, true) {
                            info!(address = %address, "Waiting for {what}");
                        }
                        (shaken, RETRY)
                    }
                    Toward::Relay(relay) => {
                        let joined = resolve(relay)
                            .and_then(|at| Ok((at, gg_relay::join(at, code.room())?)));
                        let shaken = match joined {
                            Ok((at, Joined::Paired(paired))) => {
                                heard.set(Answer::Hosted);
                                let relayed = Relayed {
                                    relay: at,
                                    token: paired.token,
                                };
                                Some(shake(paired.stream, &code, false, Some(relayed)))
                            }
                            Ok((_, Joined::Unhosted)) => {
                                heard.set(Answer::Unhosted);
                                if !mem::replace(&mut warned, true) {
                                    info!(relay = %relay, "Waiting for {what} at the relay");
                                }
                                None
                            }
                            Err(error) => {
                                heard.set(Answer::Unreachable(error.to_string().into()));
                                if !mem::replace(&mut warned, true) {
                                    warn!(
                                        code = "net.relay",
                                        relay = %relay,
                                        "Can't find {what} at the relay {relay}: {error}; trying \
                                         again every {} s",
                                        UNHOSTED.as_secs()
                                    );
                                }
                                None
                            }
                        };
                        (shaken, UNHOSTED)
                    }
                };
                if let Some(shaken) = shaken {
                    let _ = made.send(shaken);
                    return;
                }
                if !sleep_while_kept(wait, &kept) {
                    return;
                }
            }
        });
    if let Err(error) = spawned {
        warn!(
            code = "net.link",
            "Can't start the thread that connects: {error}"
        );
    }
    connection
}

/// Reads a line, without its line break, `\n` or `\r\n`: `None` once the stream ends, or ends
/// partway through a line, as a connection cut short does. As `gg_api::read_line` reads the tool
/// API's, which builds without `dev` haven't.
///
/// Fails with `InvalidData` for a line of more than [`MAX_LINE`] bytes, or one that isn't UTF-8.
fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    let limit = MAX_LINE as u64 + 1;
    reader.by_ref().take(limit).read_until(b'\n', &mut line)?;
    if line.pop() != Some(b'\n') {
        if line.len() >= MAX_LINE {
            let message = format!("a line of more than {MAX_LINE} bytes");
            return Err(io::Error::new(ErrorKind::InvalidData, message));
        }
        return Ok(None);
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
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

/// The next of `random`'s numbers, SplitMix64's, from 0 up to 1, in steps of 2⁻⁵³: for what a link
/// does to what comes, which nothing replays.
fn unit(random: &mut u64) -> f64 {
    *random = random.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *random;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// Logs where `ours`, the joiner's world after tick `tick`, first differs from `theirs`, its host's
/// (`net.desync_field`): the entity, and the component's field, with each world's value there,
/// as recordings write values, or `none` where a world hasn't the component; `alive` or `none`
/// for an entity one world alone has, its field empty.
fn log_parting(tick: u64, ours: &World, theirs: &World) {
    let Some(difference) = ours.difference(theirs) else {
        error!(
            code = "net.desync_field",
            tick,
            "After tick {tick} the peers' worlds hold the same content, so their layouts part, \
             where they hold their entities or their streams of IDs"
        );
        return;
    };
    let written = difference.written(ours, theirs);
    let (at, joiner, host) = (&written.at, &written.ours, &written.theirs);
    error!(
        code = "net.desync_field",
        tick,
        entity = %difference.entity,
        field = written.field.as_str(),
        joiner = joiner.as_str(),
        host = host.as_str(),
        "After tick {tick} the peers' worlds first differ at {at}: the joiner's {joiner}, the \
         host's {host}"
    );
}

#[cfg(test)]
mod tests;
