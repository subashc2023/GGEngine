//! Lockstep between two peers, as [Networking](../../../ARCHITECTURE.md#networking) describes: a
//! host, player 0, that listens, and a joiner, player 1, that connects and starts from the host's
//! world, and runs its host's scripts, which come with it; each tick's inputs, every layer's
//! commands and the edits that apply before it, a script's reload among them, sent [`DELAY`] ticks
//! ahead and run once both peers' are in, or, in a rollback session (`--rollback`), sent
//! [`ROLLBACK_DELAY`] ahead and run before the other's have come, as far as [`PREDICTED`] ticks on,
//! on the prediction that they hold nothing, the world taken back and the ticks run again from the
//! first whose inputs held something; the world's hash after each tick that's run on both peers'
//! inputs, compared, each peer keeping a copy of its world after the tick until it is, so that where
//! the hashes part, the host sends its joiner its world there, and the joiner names the field where
//! its own differs; and, in development builds, the patches of the game's code, which the host has
//! apply at a tick of its choosing.
//!
//! The session's stream is lines of text over TCP, which a thread of the connection's reads, and
//! another writes, as the main thread sends them, so that a slow link never holds the main thread:
//!
//! ```text
//! hello\tgg-lockstep 6\tShards\t0.1.0 debug dev\t3f8a6c2e9b1d4057\tShards Inspector\t50123
//!                                                    the joiner, as it connects: its app, engine,
//!                                                      build and layers, and the port its packets
//!                                                      come from, as `gg-rollback 6` for a
//!                                                      rollback session's
//! refused <why>                                      the host, refusing it, before it closes
//! k                                                  either, after a second with nothing else to
//!                                                      write, to say it's there
//! world 1234 2 50124                                 the host's world, at tick 1234, its code
//! gg-snapshot 2                                        2 patches on, and the port its packets
//! ...                                                  come from, as a snapshot file's text, to
//! end                                                  its `end`
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
//! UDP's, sent at each tick and again a tick's time after the last, which carry this peer's inputs
//! for each tick from the first the other hasn't said it has, as many as fit [`MAX_PACKET`], so
//! that a packet lost loses none, the ticks' lines as [`read_batches`] reads them:
//!
//! ```text
//! a 1234 1236                                        the tick the peers' worlds were last made
//!                                                      the same at, the host's world's, a packet
//!                                                      of another dropped; and the other's inputs
//!                                                      whole before tick 1236
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
mod server;

pub(crate) use client::Client;
pub(crate) use server::Server;

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::hash::{BuildHasher, RandomState};
use std::io::{self, BufRead, BufReader, ErrorKind, Read as _, Write as _};
use std::mem;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
#[cfg(feature = "dev")]
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use gg_core::{Error, Result};
use tracing::{debug, error, info, warn};

use crate::args::{Conditions, Net};
use crate::assets::{Asset, Counts};
use crate::cells::Seen;
use crate::edits::{Done, Edit};
use crate::layer::Commands;
use crate::recording::text::{self, Text};
use crate::reflect::Value;
use crate::{Context, Sound, Tick, World, WorldHash};

/// How many ticks ahead a lockstep peer sends its inputs: 62.5 ms, so that they reach the other
/// peer before it needs them across a LAN, at the cost of as long between a press and what it does.
pub(crate) const DELAY: u64 = 4;

/// How many ticks ahead a rollback peer sends its inputs: 31.25 ms between a press and what it
/// does, the other peer predicting them for as long as they take past that.
pub(crate) const ROLLBACK_DELAY: u64 = 2;

/// The most ticks a rollback peer runs on the other's inputs predicted, 125 ms of them, after which
/// it waits for them, as a lockstep peer waits for each; and so the most it runs again at once.
pub(crate) const PREDICTED: u64 = 8;

/// The hello's first part: the protocol and its version, which a change to its lines or its packets
/// takes to the next, a lockstep session's or a rollback session's, which both peers' must be.
const LOCKSTEP: &str = "gg-lockstep 6";
const ROLLBACK: &str = "gg-rollback 6";

/// The most bytes a packet may have: what a path of the internet's least MTU carries whole, as
/// QUIC's packets are kept to. A tick's inputs that wouldn't fit go on the stream.
const MAX_PACKET: usize = 1200;

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
/// the inputs kept for later are few: a peer runs no more than its delay and [`PREDICTED`] ticks
/// past the other, and a client no more than half a second past its server's latest state.
const AHEAD: u64 = 64;

/// How long a joiner waits between tries at connecting to its host.
const RETRY: Duration = Duration::from_millis(250);

/// How long a joiner its host refused waits before trying it again, as a host of another build
/// refuses it until one of them is rebuilt.
const REFUSED: Duration = Duration::from_secs(5);

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
/// a variant or a struct whose field `player` holds an integer, as the demos' commands name the
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
    /// from.
    udp: UdpSocket,
    other: Option<SocketAddr>,
    /// What came, each held until it's due, as `--link` says, or at once.
    lines: VecDeque<(Instant, String)>,
    packets: Vec<(Instant, Vec<u8>)>,
    /// When this peer last sent its packets, if it has.
    sent: Option<Instant>,
    /// Whether a packet couldn't be sent, which is warned of once a link.
    unsent: bool,
}

impl Link {
    /// Starts reading `stream`'s lines on a thread of their own, which ends at a line that isn't
    /// one, as one past [`MAX_LINE`] isn't, as the connection does, and at twice [`LOST`] with
    /// nothing come; starts writing what's sent on another, as [`write_from`] says; and binds the
    /// link's socket, which the main thread reads as each frame starts, without waiting. A link
    /// `accepted` is lost unless the other end's hello comes within [`HELLO`], as
    /// [`hello_came`](Self::hello_came) says.
    fn new(stream: TcpStream, accepted: bool) -> std::io::Result<Self> {
        let udp = UdpSocket::bind(SocketAddr::new(stream.local_addr()?.ip(), 0))?;
        udp.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(LOST))?;
        // Past the link's silence, so that the link is lost for that first; and so that the thread
        // reading a link dropped, which reads on until the other end closes, ends though it never
        // does.
        stream.set_read_timeout(Some(LOST * 2))?;
        let peer = stream.peer_addr().ok();
        let (to, unwritten) = write_from(stream.try_clone()?)?;
        let mut reader = BufReader::new(stream);
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
            other: None,
            lines: VecDeque::new(),
            packets: Vec::new(),
            sent: None,
            unsent: false,
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
    /// connection comes from.
    fn packets_from(&mut self, port: u16) {
        self.other = self.peer.map(|at| SocketAddr::new(at.ip(), port));
    }

    /// Notes that the other end's hello has come, after which the link isn't lost for want of one.
    fn hello_came(&mut self) {
        self.unhello = None;
    }

    /// The port this peer's packets come from.
    fn port(&self) -> u16 {
        self.udp.local_addr().map_or(0, |at| at.port())
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
        self.receive(buffer, conditions, random, now);
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

    /// Takes the packets that came from the other peer's socket, read through `buffer`, each held
    /// from `now` as `conditions` say, or lost, drawing on `random`.
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
                    if Some(from) != self.other {
                        continue;
                    }
                    let mut due = now;
                    if let Some(conditions) = conditions {
                        if unit(random) < conditions.loss {
                            continue;
                        }
                        due += conditions.latency + conditions.jitter.mul_f64(unit(random));
                    }
                    self.heard = now;
                    self.packets.push((due, buffer[..size].to_vec()));
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

    /// Sends `packet` to the other peer's socket, warning once a link of one that can't be sent,
    /// which is lost, as a packet may be.
    fn send_packet(&mut self, packet: &[u8]) {
        let Some(other) = self.other else { return };
        if let Err(error) = self.udp.send_to(packet, other)
            && !mem::replace(&mut self.unsent, true)
        {
            warn!(
                code = "net.link",
                "Can't send the peer a packet, which is lost, as those after may be: {error}"
            );
        }
    }
}

/// Starts the thread that writes to `stream` what's sent through the channel it returns, each text
/// whole and in order, and `k` after [`KEEP`] with nothing else, so that the other end hears from
/// this one while its main thread is busy, as restoring a world or stopped at a script's
/// breakpoint; and returns, with the channel, where the error comes that stopped it, if one did.
///
/// Once the channel closes, as its link is dropped, and what was sent before is written, it shuts
/// the connection's sending side: the other end reads what was sent, as a refusal's why, and then
/// its end, and closes its side, which ends the thread reading this one, which has read all that
/// came. Shutting the receiving side too would have Windows reset the connection at what came that
/// wasn't read, as a joiner's hello the host refuses unread, and the other end lose what was sent.
/// At a write that fails, as one that can't go on for [`LOST`] does, it shuts both sides.
fn write_from(mut stream: TcpStream) -> io::Result<(Sender<String>, Receiver<io::Error>)> {
    let (to, texts) = mpsc::channel::<String>();
    let (failed, unwritten) = mpsc::channel();
    thread::Builder::new()
        .name("gg-net-write".to_owned())
        .spawn(move || {
            loop {
                let written = match texts.recv_timeout(KEEP) {
                    Ok(text) => stream.write_all(text.as_bytes()),
                    Err(RecvTimeoutError::Timeout) => stream.write_all(b"k\n"),
                    Err(RecvTimeoutError::Disconnected) => {
                        let _ = stream.shutdown(Shutdown::Write);
                        return;
                    }
                };
                if let Err(error) = written {
                    let _ = failed.send(error);
                    let _ = stream.shutdown(Shutdown::Both);
                    return;
                }
            }
        })?;
    Ok((to, unwritten))
}

/// How a peer finds the other: the host by the connections its listener accepts, the joiner by
/// connecting to its host's address, again each time it loses it.
enum Find {
    Host(Receiver<TcpStream>),
    Join {
        address: String,
        connecting: Option<Receiver<TcpStream>>,
    },
}

/// A peer of a lockstep session.
pub(crate) struct Lockstep {
    find: Find,
    /// Where the host listens, as the OS has it: for tests, which listen where it picks.
    #[cfg(test)]
    listening: Option<SocketAddr>,
    app: &'static str,
    /// The run's build, as [`build`] says, which both peers' must be: a test's own, to stand for
    /// another's.
    build: &'static str,
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
    /// inputs, on them predicted: lockstep's, none, or a rollback session's.
    delay: u64,
    window: u64,
    /// What `--link` has the link do to what comes, and the state of the random numbers it draws.
    conditions: Option<Conditions>,
    random: u64,
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
    /// choice, which its joiner takes.
    due: Vec<(usize, u64)>,
    /// The joiner's: how many its host had applied, as its host's world comes.
    hosts: usize,
}

impl Lockstep {
    /// Starts the session `net` says, for app `app`, whose layers are `layers`, from the bottom up:
    /// the host listening at its address, the joiner trying to connect to its host's, in the
    /// background, until it does.
    ///
    /// Fails with `net.start` if the host can't listen at its address, and with `net.build` if the
    /// run's build can't be read, as [`build`] says.
    pub(crate) fn start(net: &Net, app: &'static str, layers: &[&str]) -> Result<Self> {
        let build = build()?;
        #[cfg(test)]
        let mut listening = None;
        let find = match net {
            Net::Host(address) => {
                let (find, at) = listen(address, "a lockstep peer")?;
                #[cfg(test)]
                {
                    listening = Some(at);
                }
                info!(address = %at, "Hosting a lockstep session");
                Find::Host(find)
            }
            Net::Join(address) => Find::Join {
                address: address.clone(),
                connecting: None,
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
            app,
            build,
            layers: layers.join(" "),
            link: None,
            coming: None,
            owed: false,
            parted: false,
            delay: DELAY,
            window: 0,
            conditions: None,
            random: 0,
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

    /// Whether the peer has no world to run: a joiner that hasn't the host's yet, or whose host is
    /// sending it again, whose own the host's takes the place of.
    pub(crate) fn waiting(&self) -> bool {
        !self.hosts()
            && (self.coming.is_some() || !self.link.as_ref().is_some_and(|link| link.joined))
    }

    /// Between ticks, as a frame starts: takes the lines read, the packets come and the
    /// connections made since the last turn, those `--link` holds once they're due, and returns
    /// what the peer must do of them: send its world, if it's the host and owes one, or restore the
    /// host's, if it's the joiner and has one whole. Sends its packets again a tick's time after it
    /// last did, as a peer waiting for the other's inputs, and the other for its own, sends none
    /// otherwise. Logs a connection lost, a peer refused and a desync.
    pub(crate) fn turn(&mut self) -> Turn {
        let now = Instant::now();
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
            self.read_packet(&packet);
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

    /// Takes a connection the host accepted, or the joiner made, if one has come.
    fn find_peer(&mut self) {
        let made = match &mut self.find {
            Find::Host(accepted) => {
                while let Ok(stream) = accepted.try_recv() {
                    match Link::new(stream, true) {
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
                address,
                connecting,
            } => {
                let made = connecting.get_or_insert_with(|| {
                    connect(address.clone(), "the lockstep host", Duration::ZERO)
                });
                let Ok(stream) = made.try_recv() else { return };
                *connecting = None;
                (stream, address.clone())
            }
        };
        let (stream, address) = made;
        let protocol = self.protocol();
        let hello = |link: &Link| hello(protocol, self.app, self.build, &self.layers, link.port());
        let link =
            Link::new(stream, false).and_then(|mut link| link.send(hello(&link)).map(|()| link));
        match link {
            Ok(link) => {
                info!(address = %address, "Connected to the lockstep host");
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
            "hello" if self.hosts() => self.hello(line),
            "refused" if !self.hosts() => {
                error!(
                    code = "net.refused",
                    "The lockstep host refused this peer: {rest}; it tries again in {} s",
                    REFUSED.as_secs()
                );
                self.lose("the host refused it");
                if let Find::Join {
                    address,
                    connecting,
                } = &mut self.find
                {
                    *connecting = Some(connect(address.clone(), "the lockstep host", REFUSED));
                }
            }
            "world" if !self.hosts() => {
                let world = rest.split_once(' ').and_then(|(tick, rest)| {
                    let (patches, port) = rest.split_once(' ')?;
                    let patches = patches.parse::<usize>().ok()?;
                    Some((tick_of(tick)?, patches, port.parse::<u16>().ok()?))
                });
                match world {
                    Some((tick, patches, port)) => {
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
                        self.coming = Some((tick, String::new()));
                    }
                    None => self.lose("the host's world names no tick, patches or port"),
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
            self.build,
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
    /// file's text, which names its assets as the host does, there being `assets` of them,
    /// with how many patches its code is on and where its packets come from; and its inputs for
    /// each tick from `next` to the tick its delay on, whole, its patches that are to apply among
    /// them, which it has sent before, or has none of within its delay of its start or restore. The
    /// joiner's inputs are needed from the tick its delay on from `next`, the first it can reach,
    /// and those before are none: those it predicted that its world went by, which takes the
    /// joiner's place, as its ticks before `next` did, whatever the joiner's were.
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
        let mut text = format!("world {next} {patches} {port}\n{world}");
        let none = Inputs::default();
        for tick in next..next + self.delay {
            write_inputs(&mut text, tick, self.ours.get(&tick).unwrap_or(&none));
        }
        self.ours = self.ours.split_off(&next);
        self.theirs.clear();
        self.hashes = Default::default();
        self.release();
        self.parted = false;
        let from = next + self.delay;
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
        let ours = mem::take(&mut self.ours).into_values();
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

    /// Offers `edit`, which goes out with this peer's inputs for the tick [`DELAY`] on from its
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
    /// none, or, in a rollback session, is within [`PREDICTED`] ticks of the first it hasn't, and
    /// applies no edit or patch of this peer's.
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

    /// As tick `tick` starts, once it's [`ready`](Self::ready): sends this peer's inputs for the
    /// tick its delay on, `queues`' commands, each layer's by its index, written against `world`,
    /// what it offered since its last tick, emptying them, and, the host's, the patches due before
    /// that tick; and lets go of the inputs it no longer needs.
    pub(crate) fn start_tick<'q>(
        &mut self,
        tick: u64,
        queues: impl Iterator<Item = (usize, &'q mut dyn Commands)>,
        world: &World,
    ) {
        let ahead = tick + self.delay;
        let mut inputs = Inputs::default();
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
        if self.hosts() {
            let due = self.patches.due.iter().filter(|&&(_, at)| at == ahead);
            inputs.patches.extend(due.map(|&(number, _)| number));
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
            inputs.streamed = inputs.authored || !text.is_empty() || size > MAX_PACKET;
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

    /// Logs, in a rollback session, how many times the peer rolled back, how many ticks it ran
    /// again, and the most at once (`net.rollback`), as the run ends.
    pub(crate) fn ended(&self) {
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
                    if let Find::Join {
                        address,
                        connecting,
                    } = &mut self.find
                    {
                        *connecting = Some(connect(address.clone(), "the lockstep host", REFUSED));
                    }
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

    /// Takes `packet`, from the other peer, once the link counts: how far it has this peer's
    /// inputs, and its own for each tick it holds that haven't come whole. Loses the peer for a
    /// packet that isn't one.
    fn read_packet(&mut self, packet: &[u8]) {
        if !self.link.as_ref().is_some_and(|link| link.joined) {
            return;
        }
        if self.read_ticks(packet).is_none() {
            self.lose("the peer sent a packet that isn't one");
        }
        self.came_whole();
    }

    /// Reads `packet`'s lines, as [`read_packet`](Self::read_packet) takes them, failing at one
    /// that isn't one, or at a tick's inputs past those the other peer may send, as [`AHEAD`]
    /// says, those before taken.
    fn read_ticks(&mut self, packet: &[u8]) -> Option<()> {
        let mut lines = std::str::from_utf8(packet).ok()?.lines();
        let (since, acked) = lines.next()?.strip_prefix("a ")?.split_once(' ')?;
        let (since, acked) = (tick_of(since)?, tick_of(acked)?);
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
    /// packets come from: how far this peer has its inputs, and this peer's inputs for each tick
    /// from the first it hasn't said it has that didn't go on the stream, oldest first, as many to
    /// a packet as fit [`MAX_PACKET`], a tick's in one.
    fn send_packets(&mut self) {
        if self.from.is_none() {
            return;
        }
        let Some(link) = &mut self.link else { return };
        let header = format!("a {} {}\n", self.since, self.whole);
        let (mut packet, mut lines) = (header.clone(), String::new());
        let sent = (self.ours.range(self.acked..)).filter(|(_, inputs)| !inputs.streamed);
        for (&tick, inputs) in sent {
            lines.clear();
            write_inputs(&mut lines, tick, inputs);
            if packet.len() + lines.len() > MAX_PACKET && packet.len() > header.len() {
                link.send_packet(packet.as_bytes());
                packet.clone_from(&header);
            }
            packet.push_str(&lines);
        }
        // The last, or the header alone, which says how far this peer has the other's inputs.
        link.send_packet(packet.as_bytes());
        link.sent = Some(Instant::now());
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

    fn drop_link(&mut self) {
        self.link = None;
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
    /// Offers the patch `table` names, `gg run`'s next for this process, the next tick to run
    /// being `next`, and returns its ticket, which [`done`](Self::done) gives its result with. The
    /// host has it apply before the tick its delay on, which neither peer has run, among its
    /// inputs for that tick, which tell its joiner, which has it apply then too; a joiner has it
    /// apply as its host says, or before the first tick of its host's world, if its host had
    /// applied it.
    pub(crate) fn offer_patch(&mut self, table: subsecond::JumpTable, next: u64) -> Ticket {
        self.tickets += 1;
        self.patches.got.push(Some((table, self.tickets)));
        if self.hosts() {
            let (number, at) = (self.patches.got.len(), next + self.delay);
            self.patches.due.push((number, at));
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

/// The networked session a run takes part in, as `--host`, `--join`, `--serve` or `--connect` has
/// it: a peer of a lockstep session, a server that owns the world, or a client of a server; each
/// boxed, as their sizes are far apart, and a run has one.
pub(crate) enum Session {
    Peer(Box<Lockstep>),
    Server(Box<Server>),
    Client(Box<Client>),
}

impl Session {
    /// Starts the session `net` says, for app `app`, whose layers are `layers`, from the bottom
    /// up, as [`Lockstep::start`], [`Server::start`] and [`Client::start`] say.
    ///
    /// Fails with `net.start` if a host or a server can't listen at its address.
    pub(crate) fn start(net: &Net, app: &'static str, layers: &[&str]) -> Result<Self> {
        Ok(match net {
            Net::Host(_) | Net::Join(_) => Self::Peer(Box::new(Lockstep::start(net, app, layers)?)),
            Net::Serve(address) => Self::Server(Box::new(Server::start(address, app, layers)?)),
            Net::Connect(address) => Self::Client(Box::new(Client::start(address, app, layers)?)),
        })
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
    /// as [`Lockstep::end_tick`] says; a server sends its clients their states, as
    /// [`Server::end_tick`] says; and a client keeps its world's hash, as [`Client::end_tick`] says.
    pub(crate) fn ran(&mut self, tick: u64, world: &World) {
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
            Self::Peer(net) => net.turn(),
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

/// What the hello says the run's build is: the digest of its executable's bytes, as 16 hex digits,
/// read once a process, so that a host or a server refuses a peer or a client of another build,
/// whose simulation may go otherwise, as one rebuilt with an edit's would. Builds of one source on
/// two machines differ too, as rustc writes the paths of the sources and the debug info's into the
/// executable: peers share one build, as a game's players do.
///
/// Fails with `net.build` if the executable can't be read.
fn build() -> Result<&'static str> {
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

/// Listens at `address` for `what`, on a thread of its own that accepts each connection; returns
/// where they come, and the address as the OS has it, which a test's port 0 picks.
///
/// Fails with `net.start` if it can't listen there.
fn listen(address: &str, what: &str) -> Result<(Receiver<TcpStream>, SocketAddr)> {
    let failed = |error: io::Error| {
        let message = format!("can't listen at {address} for {what}");
        Error::new("net.start", message).with_source(error)
    };
    let listener = TcpListener::bind(address).map_err(failed)?;
    let at = listener.local_addr().map_err(failed)?;
    let (accepted, find) = mpsc::channel();
    thread::Builder::new()
        .name("gg-net-accept".to_owned())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if accepted.send(stream).is_err() {
                    break;
                }
            }
        })
        .map_err(|error| {
            let message = format!("can't start the thread that accepts {what}");
            Error::new("net.start", message).with_source(error)
        })?;
    Ok((find, at))
}

/// The tick `text` names, as a line or a packet names one; `None` if it isn't a number fewer than
/// [`TICKS`].
fn tick_of(text: &str) -> Option<u64> {
    text.parse().ok().filter(|&tick| tick < TICKS)
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

/// Connects to `address`, `what`'s, on a thread of its own, after `after`, trying again every
/// [`RETRY`] until it does, and returns where the connection comes.
fn connect(address: String, what: &'static str, after: Duration) -> Receiver<TcpStream> {
    let (made, connection) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("gg-net-connect".to_owned())
        .spawn(move || {
            thread::sleep(after);
            let mut warned = false;
            loop {
                let stream = address
                    .to_socket_addrs()
                    .ok()
                    .and_then(|mut addresses| addresses.find_map(|at| TcpStream::connect(at).ok()));
                if let Some(stream) = stream {
                    let _ = made.send(stream);
                    return;
                }
                if !warned {
                    info!(address = %address, "Waiting for {what}");
                    warned = true;
                }
                thread::sleep(RETRY);
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
mod tests {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    use std::rc::Rc;

    use tracing_subscriber::layer::SubscriberExt;

    #[cfg(feature = "dev")]
    use gg_core::math::DVec3;

    use super::*;
    use crate::layer::Stack;
    use crate::testing::Codes;
    use crate::{Component, Context, Layer, Operation, Reflect, Tick};

    #[derive(Clone, Component)]
    pub(super) struct Score {
        pub(super) points: [u32; 2],
    }

    #[derive(Reflect)]
    pub(super) enum Command {
        Add { player: u8, points: u32 },
    }

    /// A game of two players, whose commands add to their scores, and which loads the scene file at
    /// `scene`, if it's given one, and the script at `script`, whose component its score's entity
    /// has.
    #[derive(Default)]
    pub(super) struct Game {
        commands: Vec<Command>,
        scene: Option<PathBuf>,
        script: Option<(crate::Scripts, PathBuf)>,
        /// Whether it runs the run's mods, and the mods once it has loaded them.
        modded: bool,
        mods: Option<crate::Mods>,
        /// Whether its session's peers predict each other's inputs, as a rollback session's do,
        /// and what its link does to what comes.
        rollback: bool,
        link: Option<Conditions>,
        /// Whether its run was networked as it attached or picked up from a world, each time, in
        /// order.
        networked: Rc<RefCell<Vec<(&'static str, bool)>>>,
    }

    impl Layer for Game {
        fn load(&mut self, cx: &mut Context) {
            if let Some((scripts, path)) = &mut self.script {
                scripts.load(cx, path).expect("loaded");
            }
            if self.modded {
                // Before the mods, which ride on it.
                cx.world_mut().register::<Score>().expect("described");
                self.mods = Some(crate::Mods::load(cx, crate::Grant::all()));
            }
        }

        fn attach(&mut self, cx: &mut Context) {
            let networked = cx.networked();
            self.networked.borrow_mut().push(("attach", networked));
            let score = cx.world_mut().spawn(Score { points: [0; 2] });
            if let Some(scene) = &self.scene {
                cx.load_scene_file(scene).expect("loaded");
            }
            if self.script.is_some() {
                cx.world_mut().add_default(score, "Spin").expect("added");
            }
        }

        fn restored(&mut self, cx: &mut Context) {
            let networked = cx.networked();
            self.networked.borrow_mut().push(("restored", networked));
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            for Command::Add { player, points } in self.commands.drain(..) {
                for score in world.query_mut::<&mut Score>() {
                    score.points[usize::from(player)] += points;
                }
            }
            if let Some((scripts, _)) = &mut self.script {
                scripts.run(world);
            }
            if let Some(mods) = &mut self.mods {
                mods.run(world);
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.commands)
        }
    }

    /// A peer: its stack, its context and the next tick it runs.
    struct Peer {
        stack: Stack,
        cx: Context,
        next: u64,
    }

    impl Peer {
        /// A peer of `net`'s session, its game attached in `cx`.
        fn new(net: &Net, cx: Context, game: Game) -> Self {
            let (rollback, link) = (game.rollback, game.link);
            let mut stack = Stack::default();
            stack.push_layer(game);
            Self::of(stack, net, cx, rollback, link)
        }

        /// A peer of `net`'s session, `stack`'s layers attached in `cx`, which predicts the other's
        /// inputs if `rollback` says so, over a link that does what `link` says.
        fn of(
            mut stack: Stack,
            net: &Net,
            mut cx: Context,
            rollback: bool,
            link: Option<Conditions>,
        ) -> Self {
            let names = stack.names().to_vec();
            let mut lockstep = Lockstep::start(net, "Tested", &names).expect("started");
            if rollback {
                lockstep.predict();
            }
            if let Some(link) = link {
                lockstep.condition(link);
            }
            stack.lockstep(lockstep);
            stack.attach(&mut cx);
            Self { stack, cx, next: 0 }
        }

        /// A host, its game attached in `cx`, which plays alone for `alone` ticks.
        fn host(cx: Context, game: Game, alone: u64) -> Self {
            let mut host = Self::new(&Net::Host("127.0.0.1:0".to_owned()), cx, game);
            for _ in 0..alone {
                host.turn();
            }
            assert_eq!(host.next, alone);
            host
        }

        /// A joiner of `host`'s session, which hasn't connected yet, over a link like its host's.
        fn joiner(host: &mut Self) -> Self {
            let net = host.net();
            let address = net.listening.expect("listening").to_string();
            let game = Game {
                rollback: net.window > 0,
                link: net.conditions,
                ..Game::default()
            };
            let joiner = Self::new(&Net::Join(address), Context::seeded(2), game);
            assert_eq!(joiner.cx.player(), Some(1));
            joiner
        }

        /// A frame's turn and the tick it runs, if the other peer's inputs have come.
        fn turn(&mut self) {
            self.wait();
            if self.stack.tick(&mut self.cx, Tick { number: self.next }) {
                self.next += 1;
            }
        }

        /// A frame's turn that runs no tick.
        fn wait(&mut self) {
            if let Some(ticks) = self.stack.net_turn(&mut self.cx, self.next) {
                self.next = ticks;
            }
        }

        fn net(&mut self) -> &mut Lockstep {
            self.stack.net_mut().expect("a peer")
        }

        /// Sets player `player`'s points to `points`, as an edit of `author`'s, applied now.
        fn set(&mut self, author: &str, player: usize, points: u32) -> Edit {
            let world = self.cx.world();
            let score = world.query::<(crate::Entity, &Score)>().next();
            let id = world.id(score.expect("a score").0).expect("an ID");
            let set = Operation::Set {
                entity: id,
                component: "Score".into(),
                path: format!("points[{player}]"),
                value: Value::UInt(u64::from(points)),
            };
            Edit::Apply {
                author: author.to_owned(),
                operations: vec![set],
            }
        }

        fn score(&self) -> [u32; 2] {
            let mut scores = self.cx.world().query::<&Score>();
            scores.next().expect("a score").points
        }
    }

    /// Has `joiner` join `host`'s session, the host not ticking until it has sent its world, which
    /// the joiner starts from at the host's next tick.
    fn join(host: &mut Peer, joiner: &mut Peer) {
        let at = host.next;
        let start = Instant::now();
        while host.net().from.is_none() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the joiner never connected"
            );
            host.stack.net_turn(&mut host.cx, host.next);
            joiner.turn();
            thread::sleep(Duration::from_millis(1));
        }
        until(host, joiner, |_, joiner| !joiner.net().waiting());
        assert!(joiner.next >= at, "{} {at}", joiner.next);
    }

    /// Turns both peers until `done` holds, failing after 10 seconds.
    fn until(
        host: &mut Peer,
        joiner: &mut Peer,
        mut done: impl FnMut(&mut Peer, &mut Peer) -> bool,
    ) {
        let start = Instant::now();
        while !done(host, joiner) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the peers never got there"
            );
            host.turn();
            joiner.turn();
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Turns both peers until both are at tick `at`, the one ahead waiting there, its frames
    /// turning on without ticking, as they take and send packets.
    fn meet(host: &mut Peer, joiner: &mut Peer, at: u64) {
        let start = Instant::now();
        while host.next < at || joiner.next < at {
            if start.elapsed() >= Duration::from_secs(10) {
                let state = |peer: &mut Peer| {
                    let next = peer.next;
                    let net = peer.net();
                    let (whole, acked) = (net.whole, net.acked);
                    format!("at {next}, the other's inputs to {whole}, its own taken to {acked}")
                };
                panic!(
                    "the peers never met at {at}: the host {}; the joiner {}",
                    state(host),
                    state(joiner)
                );
            }
            for peer in [&mut *host, &mut *joiner] {
                if peer.next < at {
                    peer.turn();
                } else {
                    peer.wait();
                }
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Has both peers meet at tick `at`, as [`meet`] does, and wait there until each has the other's
    /// inputs for every tick before it, and has run each on them, rolling back where they held
    /// what it didn't predict: so that their worlds are those both peers' inputs make.
    fn settle(host: &mut Peer, joiner: &mut Peer, at: u64) {
        meet(host, joiner, at);
        let start = Instant::now();
        let settled = |peer: &mut Peer| {
            let net = peer.net();
            net.whole >= at && net.predicted.is_empty()
        };
        while !(settled(host) && settled(joiner)) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the peers never settled"
            );
            host.wait();
            joiner.wait();
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// What a slow link does in the tests: holds each packet 20 ms, and up to 10 more, and loses a
    /// fifth of them.
    pub(super) const SLOW: Conditions = Conditions {
        latency: Duration::from_millis(20),
        jitter: Duration::from_millis(10),
        loss: 0.2,
    };

    /// Rollback peers over a slow link run ahead of each other's inputs, on them predicted to hold
    /// nothing, and roll back to the first tick whose inputs held something, running it and the
    /// ticks after again: each player's commands apply once, though they come late, lost and out
    /// of order, and their worlds hash alike at each tick run on both peers' inputs.
    #[test]
    fn rollback_peers_over_a_slow_link_apply_each_command_once_and_hash_alike() {
        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let game = Game {
            rollback: true,
            link: Some(SLOW),
            ..Game::default()
        };
        let mut host = Peer::host(Context::seeded(1), game, 1);
        let mut joiner = Peer::joiner(&mut host);
        join(&mut host, &mut joiner);
        // The host's player at each turn, the joiner's at every third, as keys change.
        let mut queued = [0; 2];
        for turn in 0..300 {
            host.stack_commands(Command::Add {
                player: 0,
                points: 1,
            });
            queued[0] += 1;
            if turn % 3 == 0 {
                joiner.stack_commands(Command::Add {
                    player: 1,
                    points: 10,
                });
                queued[1] += 10;
            }
            host.turn();
            joiner.turn();
            thread::sleep(Duration::from_millis(1));
        }
        // Past the ticks the last commands went out for.
        let at = host.next.max(joiner.next) + ROLLBACK_DELAY + 1;
        settle(&mut host, &mut joiner, at);
        assert_eq!(host.score(), queued);
        assert_eq!(joiner.score(), queued);
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
        let rolled = |peer: &mut Peer| peer.net().rolled;
        let ([rollbacks, again, most], [theirs, ..]) = (rolled(&mut host), rolled(&mut joiner));
        assert!(rollbacks > 0 && theirs > 0, "{rollbacks} {theirs}");
        assert!(again >= rollbacks && most <= PREDICTED, "{again} {most}");
        let codes: Vec<String> = logged.try_iter().collect();
        let parted = ["net.desync", "net.bad_command", "net.lost"];
        assert!(
            !codes.iter().any(|code| parted.contains(&code.as_str())),
            "{codes:?}"
        );
    }

    /// The commands of [`Editor`]'s queue, which edit.
    #[derive(Default)]
    struct Edits(Vec<Command>);

    impl Commands for Edits {
        fn len(&self) -> usize {
            self.0.len()
        }

        fn value(&self, i: usize) -> Value {
            self.0[i].to_value()
        }

        fn push(&mut self, value: &Value) -> Result<()> {
            self.0.push(Command::from_value(value)?);
            Ok(())
        }

        fn edits(&self) -> bool {
            true
        }

        fn clear(&mut self) {
            self.0.clear();
        }
    }

    /// Adds its commands' points to the score it attaches, and counts them as it applies them,
    /// as the Inspector keeps a log of its edits: a tick that ran them again would count them
    /// again.
    struct Editor {
        queued: Edits,
        applied: Rc<Cell<u32>>,
    }

    impl Layer for Editor {
        fn attach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn(Score { points: [0; 2] });
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            for Command::Add { player, points } in self.queued.0.drain(..) {
                for score in world.query_mut::<&mut Score>() {
                    score.points[usize::from(player)] += points;
                }
                self.applied.set(self.applied.get() + 1);
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.queued)
        }
    }

    /// In a rollback session, a tick that applies a command of a queue that edits, as the
    /// Inspector's, runs only once it has both peers' inputs, the peer whose it is waiting for the
    /// other's, and the other rolling back to it: so that each applies once on each peer, as a
    /// layer that keeps what it did, as the Inspector's log, counts it.
    #[test]
    fn rollback_peers_apply_each_edit_once() {
        let peer = |net: &Net, cx: Context| {
            let applied = Rc::new(Cell::new(0));
            let mut stack = Stack::default();
            stack.push_layer(Editor {
                queued: Edits::default(),
                applied: Rc::clone(&applied),
            });
            (Peer::of(stack, net, cx, true, Some(SLOW)), applied)
        };
        let (mut host, by_host) = peer(&Net::Host("127.0.0.1:0".to_owned()), Context::seeded(1));
        host.turn();
        let address = host.net().listening.expect("listening").to_string();
        let (mut joiner, by_joiner) = peer(&Net::Join(address), Context::seeded(2));
        join(&mut host, &mut joiner);
        let edit = |peer: &mut Peer, player: u8| {
            let queue = peer
                .stack
                .commands_of(0)
                .expect("the editor queues commands");
            let command = Command::Add { player, points: 1 };
            queue.push(&command.to_value()).expect("queued");
        };
        let mut queued = [0; 2];
        for turn in 0..200 {
            if turn % 7 == 0 {
                edit(&mut host, 0);
                queued[0] += 1;
            }
            if turn % 5 == 0 {
                edit(&mut joiner, 1);
                queued[1] += 1;
            }
            host.turn();
            joiner.turn();
            thread::sleep(Duration::from_millis(1));
        }
        let at = host.next.max(joiner.next) + ROLLBACK_DELAY + 1;
        settle(&mut host, &mut joiner, at);
        let all = queued[0] + queued[1];
        assert_eq!((by_host.get(), by_joiner.get()), (all, all));
        assert_eq!(host.score(), queued);
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    }

    /// A command of [`Stager`]'s: asks for the scene of this path from the asset root.
    #[derive(Reflect)]
    struct Stage(String);

    /// Loads `scenes/menu.ron` as it attaches, and asks at each tick for the scenes its commands
    /// name, as a game goes from its menu to a level.
    #[derive(Default)]
    struct Stager(Vec<Stage>);

    impl Layer for Stager {
        fn attach(&mut self, cx: &mut Context) {
            let root = cx.root.clone().expect("a root");
            cx.load_scene_file(root.join("scenes/menu.ron"))
                .expect("loaded");
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            for Stage(scene) in self.0.drain(..) {
                world.change_scene(&scene);
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.0)
        }
    }

    /// Lockstep peers, predicting each other's inputs and rolling back if `rollback`, each put
    /// in a scene the other's command asked for at the same tick, loading the texture it names
    /// on both, and hash alike, without a desync.
    fn peers_change_scenes_in_step(rollback: bool) {
        use std::fs;

        use crate::dirs::tests::Scratch;

        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let dir = Scratch::new(&format!("net-scene-change-{rollback}"));
        let root = dir.0.clone();
        fs::create_dir_all(root.join("scenes")).expect("a directory");
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        fs::write(root.join("moss.png"), png).expect("a PNG");
        let sprite = |id: u64, texture: &str| {
            format!(
                "        \"{id:016x}\": (\n            Sprite: (\n                texture: \
                 {texture},\n            ),\n        ),\n"
            )
        };
        let scene = |entities: &str| format!("(\n    entities: {{\n{entities}    }},\n)\n");
        let menu = scene(&sprite(1, "None"));
        let level = scene(&(sprite(2, "Some(\"moss.png\")") + &sprite(3, "None")));
        fs::write(root.join("scenes/menu.ron"), menu).expect("the menu");
        fs::write(root.join("scenes/level.ron"), level).expect("the level");
        let link = rollback.then_some(SLOW);
        let peer = |net: &Net, seed: u64| {
            let mut stack = Stack::default();
            stack.push_layer(Stager::default());
            Peer::of(
                stack,
                net,
                Context::seeded(seed).root(&root),
                rollback,
                link,
            )
        };
        let mut host = peer(&Net::Host("127.0.0.1:0".to_owned()), 1);
        host.turn();
        let address = host.net().listening.expect("listening").to_string();
        let mut joiner = peer(&Net::Join(address), 2);
        join(&mut host, &mut joiner);
        let stage = |peer: &mut Peer, scene: &str| {
            let queue = peer
                .stack
                .commands_of(0)
                .expect("the stager queues commands");
            let command = Stage(scene.to_owned()).to_value();
            queue.push(&command).expect("queued");
        };
        let ids = |peer: &Peer| {
            let world = peer.cx.world();
            let mut ids: Vec<u64> = (world.query::<crate::Entity>())
                .map(|entity| world.id(entity).expect("alive").get())
                .collect();
            ids.sort_unstable();
            ids
        };
        for (by_joiner, scene, entities) in [
            (true, "scenes/level.ron", vec![2, 3]),
            (false, "scenes/menu.ron", vec![1]),
            (false, "scenes/level.ron", vec![2, 3]),
        ] {
            stage(if by_joiner { &mut joiner } else { &mut host }, scene);
            for _ in 0..20 {
                host.turn();
                joiner.turn();
                thread::sleep(Duration::from_millis(1));
            }
            let at = host.next.max(joiner.next) + DELAY.max(ROLLBACK_DELAY) + 1;
            settle(&mut host, &mut joiner, at);
            assert_eq!((ids(&host), ids(&joiner)), (entities.clone(), entities));
            assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
        }
        for peer in [&host, &joiner] {
            assert!(peer.cx.world().assets().texture("moss.png").is_some());
        }
        // Each takes the last as final as the ticks after run, as a rollback peer may have taken
        // it as predicted, its run on both peers' inputs alike.
        let at = host.next.max(joiner.next) + 2;
        settle(&mut host, &mut joiner, at);
        let codes: Vec<String> = logged.try_iter().collect();
        let replaced = (codes.iter())
            .filter(|code| *code == "scene.replaced")
            .count();
        assert_eq!(replaced, 6, "three on each peer: {codes:?}");
        let parted = [
            "net.desync",
            "net.bad_command",
            "net.lost",
            "asset.stand_in",
        ];
        assert!(
            !codes.iter().any(|code| parted.contains(&code.as_str())),
            "{codes:?}"
        );
    }

    /// Whether a [`Vetoer`]'s scene changes are vetoed: a resource, which rollback takes back with
    /// the world.
    #[derive(Clone)]
    struct Vetoed;

    /// A command of [`Vetoer`]'s.
    #[derive(Reflect)]
    enum Stagehand {
        /// Vetoes every scene change from then on.
        Veto,
        /// Asks for the scene of this path from the asset root, unless vetoed.
        Go(String),
    }

    /// Loads `scenes/menu.ron` as it attaches, and applies its commands at each tick.
    #[derive(Default)]
    struct Vetoer(Vec<Stagehand>);

    impl Layer for Vetoer {
        fn attach(&mut self, cx: &mut Context) {
            let root = cx.root.clone().expect("a root");
            cx.load_scene_file(root.join("scenes/menu.ron"))
                .expect("loaded");
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            for command in self.0.drain(..) {
                match command {
                    Stagehand::Veto => {
                        world.insert_resource(Vetoed);
                    }
                    Stagehand::Go(scene) if world.resource::<Vetoed>().is_none() => {
                        world.change_scene(&scene);
                    }
                    Stagehand::Go(_) => {}
                }
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.0)
        }
    }

    /// A rollback peer that changed the scene at a tick run on the other's inputs predicted, which
    /// held a veto of it, takes the change back as it rolls back to that tick: the world, the scene
    /// file in it and the logs keep nothing of it, both peers hashing alike, and no peer logs
    /// `scene.replaced` for it.
    #[test]
    fn a_scene_change_rollback_takes_back_leaves_nothing_of_it() {
        use std::fs;

        use crate::dirs::tests::Scratch;

        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let dir = Scratch::new("net-scene-change-vetoed");
        let root = dir.0.clone();
        fs::create_dir_all(root.join("scenes")).expect("a directory");
        let scene = |id: u64| {
            format!(
                "(\n    entities: {{\n        \"{id:016x}\": (\n            Sprite: (),\n        \
                 ),\n    }},\n)\n"
            )
        };
        fs::write(root.join("scenes/menu.ron"), scene(1)).expect("the menu");
        fs::write(root.join("scenes/level.ron"), scene(2)).expect("the level");
        // Slower than the ticks the peers predict ahead of each other's inputs, so that the
        // joiner runs the tick of its change before the host's veto of it comes.
        let link = Conditions {
            latency: Duration::from_millis(80),
            jitter: Duration::ZERO,
            loss: 0.0,
        };
        let peer = |net: &Net, seed: u64| {
            let mut stack = Stack::default();
            stack.push_layer(Vetoer::default());
            Peer::of(
                stack,
                net,
                Context::seeded(seed).root(&root),
                true,
                Some(link),
            )
        };
        let mut host = peer(&Net::Host("127.0.0.1:0".to_owned()), 1);
        host.turn();
        let address = host.net().listening.expect("listening").to_string();
        let mut joiner = peer(&Net::Join(address), 2);
        join(&mut host, &mut joiner);
        let at = host.next.max(joiner.next) + 1;
        meet(&mut host, &mut joiner, at);
        let queue = |peer: &mut Peer, command: Stagehand| {
            let queue = peer
                .stack
                .commands_of(0)
                .expect("the vetoer queues commands");
            queue.push(&command.to_value()).expect("queued");
        };
        // For the same tick, the host's first.
        queue(&mut host, Stagehand::Veto);
        queue(&mut joiner, Stagehand::Go("scenes/level.ron".to_owned()));
        for _ in 0..40 {
            host.turn();
            joiner.turn();
            thread::sleep(Duration::from_millis(1));
        }
        let settled = host.next.max(joiner.next) + ROLLBACK_DELAY + 1;
        settle(&mut host, &mut joiner, settled);
        for peer in [&host, &joiner] {
            let world = peer.cx.world();
            assert_eq!(world.scene(), None);
            assert!(world.resource::<Vetoed>().is_some());
            let files: Vec<&str> = (peer.cx.scenes.files.iter())
                .map(|file| file.author.as_str())
                .collect();
            assert_eq!(files, ["scenes/menu.ron"]);
        }
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
        let [rollbacks, ..] = joiner.net().rolled;
        assert!(rollbacks > 0, "the joiner rolled back");
        let codes: Vec<String> = logged.try_iter().collect();
        assert!(
            codes.iter().any(|code| code == "scene.entered"),
            "the joiner's prediction changed the scene: {codes:?}"
        );
        let parted = ["scene.replaced", "net.desync", "net.lost"];
        assert!(
            !codes.iter().any(|code| parted.contains(&code.as_str())),
            "{codes:?}"
        );
    }

    /// As [`peers_change_scenes_in_step`] says, in lockstep.
    #[test]
    fn lockstep_peers_change_scenes_in_step() {
        peers_change_scenes_in_step(false);
    }

    /// As [`peers_change_scenes_in_step`] says, rolling back over a slow link.
    #[test]
    fn rollback_peers_change_scenes_in_step() {
        peers_change_scenes_in_step(true);
    }

    /// A peer of `net`'s session in the asset root `root`, of a [`Walker`](crate::cells::tests::Walker)
    /// that builds a world of cells, which predicts the other's inputs and rolls back over a slow
    /// link if `rollback` says so.
    fn walker(net: &Net, root: &std::path::Path, seed: u64, rollback: bool) -> Peer {
        let mut stack = Stack::default();
        stack.push_layer(crate::cells::tests::Walker::default());
        let cx = Context::seeded(seed).root(root);
        Peer::of(stack, net, cx, rollback, rollback.then_some(SLOW))
    }

    /// Queues `walk` on `peer`'s walker.
    fn walk(peer: &mut Peer, walk: crate::cells::tests::Walk) {
        let queue = peer
            .stack
            .commands_of(0)
            .expect("the walker queues commands");
        queue.push(&walk.to_value()).expect("queued");
    }

    /// Lockstep peers, predicting each other's inputs and rolling back if `rollback`, bring the
    /// same cells in and take the same out as each moves the viewer or an entity in turn, cell 1
    /// coming back from what it keeps, their worlds hashing alike once both have run each tick on
    /// both peers' inputs, without a desync: rolling back, each runs ticks again across cells'
    /// entries and exits.
    fn peers_stream_cells_in_step(rollback: bool) {
        use crate::cells::tests::{Walk, id_of, project, xs};
        use crate::dirs::tests::Scratch;

        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let dir = Scratch::new(&format!("net-cells-{rollback}"));
        let root = project(&dir.0);
        let mut host = walker(&Net::Host("127.0.0.1:0".to_owned()), &root, 1, rollback);
        host.turn();
        let address = host.net().listening.expect("listening").to_string();
        let mut joiner = walker(&Net::Join(address), &root, 2, rollback);
        join(&mut host, &mut joiner);
        for (by_joiner, step, cells) in [
            (true, Walk::Viewer(25.0), vec![0, 1, 2, 3]),
            (false, Walk::Crate(12.0), vec![0, 1, 2, 3]),
            (true, Walk::Viewer(100.0), vec![]),
            (false, Walk::Viewer(5.0), vec![0, 1]),
        ] {
            walk(if by_joiner { &mut joiner } else { &mut host }, step);
            for _ in 0..20 {
                host.turn();
                joiner.turn();
                thread::sleep(Duration::from_millis(1));
            }
            let at = host.next.max(joiner.next) + DELAY.max(ROLLBACK_DELAY) + 1;
            settle(&mut host, &mut joiner, at);
            let (ours, theirs) = (host.cx.world(), joiner.cx.world());
            assert_eq!((xs(ours), xs(theirs)), (cells.clone(), cells), "{step:?}");
            assert_eq!(ours.hash(), theirs.hash(), "{step:?}");
        }
        for peer in [&host, &joiner] {
            let world = peer.cx.world();
            let entity = world.entity(id_of(1)).expect("in");
            let at = world.get::<crate::Transform>(entity).expect("placed");
            assert_eq!(at.translation.x, 12.0, "as it was left");
        }
        if rollback {
            for peer in [&mut host, &mut joiner] {
                let [rollbacks, ..] = peer.net().rolled;
                assert!(rollbacks > 0, "rolled back");
            }
        }
        let codes: Vec<String> = logged.try_iter().collect();
        let parted = ["net.desync", "net.bad_command", "net.lost", "cells.refused"];
        assert!(
            !codes.iter().any(|code| parted.contains(&code.as_str())),
            "{codes:?}"
        );
    }

    /// As [`peers_stream_cells_in_step`] says, in lockstep.
    #[test]
    fn lockstep_peers_stream_cells_in_step() {
        peers_stream_cells_in_step(false);
    }

    /// As [`peers_stream_cells_in_step`] says, rolling back over a slow link.
    #[test]
    fn rollback_peers_stream_cells_in_step() {
        peers_stream_cells_in_step(true);
    }

    /// A joiner whose cell file isn't its host's, which their worlds part at as the cell comes in,
    /// names it and leaves the session, to try again later, rather than taking the host's world and
    /// parting again at the next cell.
    #[test]
    fn a_joiner_whose_cell_file_differs_names_it_and_leaves() {
        use crate::cells::tests::{Walk, project};
        use crate::dirs::tests::Scratch;

        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let dirs = ["net-cell-file-host", "net-cell-file-joiner"].map(Scratch::new);
        let [host_root, joiner_root] = [&dirs[0], &dirs[1]].map(|dir| project(&dir.0));
        let two = joiner_root.join("cells/2_0_0.ron");
        let other = std::fs::read_to_string(&two).expect("a cell");
        std::fs::write(&two, other.replace("25.0", "26.0")).expect("written");
        let mut host = walker(&Net::Host("127.0.0.1:0".to_owned()), &host_root, 1, false);
        host.turn();
        let address = host.net().listening.expect("listening").to_string();
        let mut joiner = walker(&Net::Join(address), &joiner_root, 2, false);
        join(&mut host, &mut joiner);
        walk(&mut joiner, Walk::Viewer(25.0));
        let started = Instant::now();
        let mut seen: Vec<String> = Vec::new();
        while !seen.iter().any(|code| code == "net.cell_file") {
            assert!(started.elapsed() < Duration::from_secs(10), "{seen:?}");
            host.turn();
            joiner.turn();
            thread::sleep(Duration::from_millis(1));
            seen.extend(logged.try_iter());
        }
        assert!(seen.iter().any(|code| code == "net.desync"), "{seen:?}");
        assert!(seen.iter().any(|code| code == "net.lost"), "{seen:?}");
    }

    /// A joiner that joins once its host's cells have come and gone starts from the host's world,
    /// what the cells keep among it, and brings them in again alike: cell 1's entity where the host
    /// left it.
    #[test]
    fn a_joiner_takes_what_its_host_s_cells_keep() {
        use crate::cells::tests::{Walk, id_of, project, xs};
        use crate::dirs::tests::Scratch;

        let dir = Scratch::new("net-cells-joiner");
        let root = project(&dir.0);
        let mut host = walker(&Net::Host("127.0.0.1:0".to_owned()), &root, 1, false);
        for step in [Walk::Viewer(25.0), Walk::Crate(12.0), Walk::Viewer(100.0)] {
            walk(&mut host, step);
            // Past the tick its commands go out for, `DELAY` on, as a peer's do alone too.
            for _ in 0..=DELAY {
                host.turn();
            }
        }
        let kept = |peer: &Peer| {
            let world = peer.cx.world();
            world.resource::<crate::cells::CellState>().cloned()
        };
        let held = kept(&host).expect("the host's cells came in");
        // Cell 1 alone changed; the others went out as they came.
        assert_eq!((held.cells_in.len(), held.kept.len()), (0, 1));
        let address = host.net().listening.expect("listening").to_string();
        let mut joiner = walker(&Net::Join(address), &root, 2, false);
        join(&mut host, &mut joiner);
        assert_eq!(kept(&joiner), Some(held), "the host's");
        walk(&mut joiner, Walk::Viewer(5.0));
        for _ in 0..20 {
            host.turn();
            joiner.turn();
            thread::sleep(Duration::from_millis(1));
        }
        let at = host.next.max(joiner.next) + DELAY + 1;
        settle(&mut host, &mut joiner, at);
        for peer in [&host, &joiner] {
            let world = peer.cx.world();
            assert_eq!(xs(world), [0, 1]);
            let entity = world.entity(id_of(1)).expect("in");
            let at = world.get::<crate::Transform>(entity).expect("placed");
            assert_eq!(at.translation.x, 12.0, "where the host left it");
        }
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    }

    /// Queues a command for each player on its peer, and turns both, `turns` times.
    fn play(host: &mut Peer, joiner: &mut Peer, turns: usize) {
        for _ in 0..turns {
            host.stack_commands(Command::Add {
                player: 0,
                points: 1,
            });
            joiner.stack_commands(Command::Add {
                player: 1,
                points: 10,
            });
            host.turn();
            joiner.turn();
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Sends the fields of each event logged with `net.desync_field`, by name, as text.
    struct Partings(mpsc::Sender<BTreeMap<String, String>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Partings {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Fields(BTreeMap<String, String>);
            impl tracing::field::Visit for Fields {
                fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                    self.0.insert(field.name().to_owned(), value.to_owned());
                }
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    self.0.insert(field.name().to_owned(), format!("{value:?}"));
                }
            }
            let mut fields = Fields(BTreeMap::new());
            event.record(&mut fields);
            if fields
                .0
                .get("code")
                .is_some_and(|code| code == "net.desync_field")
            {
                let _ = self.0.send(fields.0);
            }
        }
    }

    /// A joiner starts from the host's world, joining within [`DELAY`] ticks of the host's start,
    /// before which the host has sent no inputs; from then each tick runs on both players' commands
    /// on both peers, which hash alike. Worlds that go apart, as commands go both ways, are named
    /// at their tick once by each peer, the joiner naming the field where they parted, there, with
    /// both values, and made the same again once.
    #[test]
    fn two_peers_run_each_tick_on_both_players_inputs() {
        let (codes, logged) = mpsc::channel();
        let (partings, parted) = mpsc::channel();
        let subscriber = tracing_subscriber::registry()
            .with(Codes(codes))
            .with(Partings(partings));
        let _logging = tracing::subscriber::set_default(subscriber);
        let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
        let mut joiner = Peer::joiner(&mut host);
        join(&mut host, &mut joiner);
        play(&mut host, &mut joiner, 50);
        let at = host.next.max(joiner.next) + DELAY + 1;
        meet(&mut host, &mut joiner, at);
        assert_eq!(host.score(), joiner.score());
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
        let [ours, theirs] = host.score();
        assert!(ours > 0 && theirs > 0, "{ours} {theirs}");

        // The host's world changed behind the session's back, between ticks, the players playing
        // on: the first tick after parts the worlds by the change to player 1's points.
        play(&mut host, &mut joiner, 10);
        let (edited, points) = (host.next, host.score()[1]);
        let edit = host.set("cheat", 1, 5);
        host.cx.apply_edit(edit).expect("applied");
        // The host runs ahead as far as its joiner's inputs reach, and the joiner then past it as
        // far as the host's do, so that the joiner runs ticks after the one their worlds parted at
        // before the host's world after it comes, as over a link slower than the loopback.
        for _ in 0..DELAY * 2 {
            host.stack_commands(Command::Add {
                player: 0,
                points: 1,
            });
            host.turn();
            thread::sleep(Duration::from_millis(1));
        }
        for _ in 0..DELAY * 4 {
            joiner.turn();
            thread::sleep(Duration::from_millis(1));
        }
        assert!(joiner.next > edited + 1, "{} {edited}", joiner.next);
        play(&mut host, &mut joiner, 100);
        let start = Instant::now();
        loop {
            let at = host.next.max(joiner.next) + 1;
            meet(&mut host, &mut joiner, at);
            if host.cx.world().hash() == joiner.cx.world().hash() {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the peers never came back together"
            );
        }
        assert_eq!(host.score(), joiner.score());
        let desyncs = logged.try_iter().filter(|code| code == "net.desync");
        assert_eq!(desyncs.count(), 2);
        let partings: Vec<BTreeMap<String, String>> = parted.try_iter().collect();
        let [parting] = &partings[..] else {
            panic!("the joiner alone names it, once: {partings:?}");
        };
        let world = host.cx.world();
        let score = world.query::<(crate::Entity, &Score)>().next();
        let score = world.id(score.expect("a score").0).expect("an ID");
        assert_eq!(parting["tick"], edited.to_string(), "{parting:?}");
        assert_eq!(parting["entity"], score.to_string(), "{parting:?}");
        assert_eq!(parting["field"], "Score.points[1]", "{parting:?}");
        let value = |peer: &str| {
            let value = parting[peer].strip_prefix("UInt(");
            let value = value.and_then(|value| value.strip_suffix(')'));
            value.and_then(|value| value.parse::<i64>().ok())
        };
        let (Some(joiner), Some(host)) = (value("joiner"), value("host")) else {
            panic!("two points: {parting:?}");
        };
        assert_eq!(joiner - host, i64::from(points) - 5, "{parting:?}");
    }

    /// A lockstep session's host and joiner are networked as their layers attach, and the joiner as
    /// its layers pick up from its host's world; and so is a host started again from a snapshot of
    /// its world, as `gg run` restarts a peer, as its layers pick up from it in place of attaching:
    /// so that a game keeps what only a run alone has, as a player it plays itself, out of a
    /// session's worlds.
    #[test]
    fn peers_are_networked_as_their_layers_attach_and_pick_up() {
        let seen = |game: &Game| Rc::clone(&game.networked);
        let game = Game::default();
        let host_saw = seen(&game);
        let mut host = Peer::host(Context::seeded(1), game, 1);
        let address = host.net().listening.expect("listening").to_string();
        let game = Game::default();
        let joiner_saw = seen(&game);
        let mut joiner = Peer::new(&Net::Join(address), Context::seeded(2), game);
        join(&mut host, &mut joiner);
        assert_eq!(*host_saw.borrow(), [("attach", true)]);
        assert_eq!(*joiner_saw.borrow(), [("attach", true), ("restored", true)]);

        let of = crate::snapshot::Of {
            world: host.cx.world(),
            scenes: &host.cx.scenes.files,
            cached: &host.cx.scenes.cached,
            open: &host.cx.scenes.open,
            scripts: &[],
            mods: &[],
            seen: None,
        };
        let playing = &crate::snapshot::Run::Playing;
        let text = crate::snapshot::write(&of, playing, "Tested", host.stack.names(), host.next);
        let from = "the host's world".to_owned();
        let snapshot = crate::snapshot::Snapshot::parse(text.expect("written"), from, "Tested");
        let game = Game::default();
        let restarted_saw = seen(&game);
        let mut stack = Stack::default();
        stack.push_layer(game);
        let names = stack.names().to_vec();
        let net = Net::Host("127.0.0.1:0".to_owned());
        stack.lockstep(Lockstep::start(&net, "Tested", &names).expect("started"));
        let mut cx = Context::seeded(3);
        cx.world_mut().register::<Score>().expect("described");
        let restored = stack.restore(&mut cx, &snapshot.expect("read"), false);
        restored.expect("restored");
        assert!(cx.networked());
        assert_eq!(*restarted_saw.borrow(), [("restored", true)]);
    }

    /// A joiner its host refuses, as of another app, closes its connection and tries again only
    /// after [`REFUSED`], so that the host refuses it once.
    #[test]
    fn a_refused_joiner_waits_before_trying_again() {
        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
        let address = host.net().listening.expect("listening").to_string();
        let mut other = Lockstep::start(&Net::Join(address), "Other", &["Game"]).expect("started");
        let start = Instant::now();
        let mut refused = 0;
        while start.elapsed() < Duration::from_millis(600) {
            host.turn();
            assert!(matches!(other.turn(), Turn::None));
            refused += logged
                .try_iter()
                .filter(|code| code == "net.refused")
                .count();
            thread::sleep(Duration::from_millis(1));
        }
        // Each side's.
        assert_eq!(refused, 2);
        assert!(other.link.is_none() && host.net().link.is_none());
    }

    /// A line as long as [`MAX_LINE`] is read, and one longer, as a connection that never breaks
    /// its line sends, drops its connection before its hello, freeing the joiner's place.
    #[test]
    fn a_line_past_the_most_drops_its_connection() {
        let mut most = vec![b'x'; MAX_LINE];
        most.push(b'\n');
        let read = read_line(&mut &most[..]).expect("a line");
        assert_eq!(read.map(|line| line.len()), Some(MAX_LINE));

        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
        let address = host.net().listening.expect("listening");
        let mut stranger = TcpStream::connect(address).expect("connected");
        // Written as the host reads it, the connection held open after.
        let writer = thread::spawn(move || {
            let _ = stranger.write_all(&vec![b'x'; MAX_LINE + 1]);
            stranger
        });
        let start = Instant::now();
        while !logged.try_iter().any(|code| code == "net.lost") {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the connection was never dropped"
            );
            host.turn();
            thread::sleep(Duration::from_millis(1));
        }
        let _stranger = writer.join().expect("written");
        let mut joiner = Peer::joiner(&mut host);
        join(&mut host, &mut joiner);
    }

    /// A connection that says no hello within [`HELLO`] of the host's taking it is lost, freeing the
    /// joiner's place, which a joiner then takes.
    #[test]
    fn a_connection_that_says_no_hello_is_lost() {
        let logged = crate::testing::logged();
        let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
        let address = host.net().listening.expect("listening");
        let _stranger = TcpStream::connect(address).expect("connected");
        let start = Instant::now();
        while host.net().link.is_none() {
            assert!(start.elapsed() < Duration::from_secs(10), "never taken");
            host.turn();
            thread::sleep(Duration::from_millis(1));
        }
        for _ in 0..10 {
            host.turn();
        }
        assert!(host.net().link.is_some(), "kept until its hello's time");
        // As though that time had gone by since the host took it.
        let link = host.net().link.as_mut().expect("a link");
        link.unhello = Instant::now().checked_sub(HELLO);
        host.turn();
        assert!(host.net().link.is_none());
        assert!(logged.codes.try_iter().any(|code| code == "net.lost"));
        let mut joiner = Peer::joiner(&mut host);
        join(&mut host, &mut joiner);
    }

    /// A peer that says hello and then nothing, as one cut off from the network or stopped in a
    /// debugger, is lost once nothing has come by its link for the link's silence, the host playing
    /// on alone; until then it's kept.
    #[test]
    fn a_peer_nothing_comes_from_is_lost() {
        let logged = crate::testing::logged();
        let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
        let address = host.net().listening.expect("listening");
        let mut stranger = TcpStream::connect(address).expect("connected");
        let net = host.net();
        let line = hello(net.protocol(), net.app, net.build, &net.layers, 9);
        stranger.write_all(line.as_bytes()).expect("said hello");
        let start = Instant::now();
        while host.net().from.is_none() {
            assert!(start.elapsed() < Duration::from_secs(10), "never taken");
            host.turn();
            thread::sleep(Duration::from_millis(1));
        }
        // Its hello came a few turns ago.
        host.net().link.as_mut().expect("a link").silence = Duration::from_millis(300);
        host.turn();
        assert!(host.net().link.is_some(), "kept until its silence");
        let start = Instant::now();
        while host.net().link.is_some() {
            assert!(start.elapsed() < Duration::from_secs(10), "never lost");
            host.turn();
            thread::sleep(Duration::from_millis(1));
        }
        assert!(logged.codes.try_iter().any(|code| code == "net.lost"));
        let at = host.next;
        for _ in 0..10 {
            host.turn();
        }
        assert!(host.next > at, "the host plays on alone");
    }

    /// A peer whose frames stop for longer than its link's silence, as one busy restoring a large
    /// world, or stopped at a script's breakpoint, is kept, as its link's writing thread says it's
    /// there; and the peers go on in step once its frames go on.
    #[test]
    fn a_peer_whose_frames_stop_is_kept_by_its_link_s_thread() {
        let logged = crate::testing::logged();
        let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
        let mut joiner = Peer::joiner(&mut host);
        join(&mut host, &mut joiner);
        play(&mut host, &mut joiner, 10);
        // Past `KEEP`, after which the joiner's thread writes `k`.
        let silence = KEEP * 5 / 2;
        host.net().link.as_mut().expect("a link").silence = silence;
        let stopped = Instant::now();
        while stopped.elapsed() < silence + KEEP {
            host.turn();
            thread::sleep(Duration::from_millis(1));
        }
        assert!(host.net().link.is_some(), "the joiner was lost");
        play(&mut host, &mut joiner, 10);
        let at = host.next.max(joiner.next) + DELAY + 1;
        meet(&mut host, &mut joiner, at);
        assert_eq!(host.score(), joiner.score());
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert!(!codes.iter().any(|code| code == "net.lost"), "{codes:?}");
    }

    /// A joiner of another build, whose simulation may go otherwise, is refused, its hello naming
    /// both builds.
    #[test]
    fn a_joiner_of_another_build_is_refused() {
        let ours = [
            "gg-lockstep 6",
            "Tested",
            "0.1.0 debug",
            "0123456789abcdef",
            "Game",
        ];
        let theirs = hello(ours[0], ours[1], "fedcba9876543210", ours[4], 9);
        let theirs = theirs.trim_end().replace(&engine(), ours[2]);
        let why = port_of(&theirs, ours, "the host").expect_err("refused");
        assert!(
            why.contains("build fedcba9876543210") && why.contains("build 0123456789abcdef"),
            "{why}"
        );

        let logged = crate::testing::logged();
        let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
        let address = host.net().listening.expect("listening").to_string();
        let layers = host.net().layers.clone();
        let layers: Vec<&str> = layers.split(' ').collect();
        let mut other = Lockstep::start(&Net::Join(address), "Tested", &layers).expect("started");
        other.build = "0000000000000000";
        let start = Instant::now();
        let mut refused = 0;
        while refused < 2 {
            assert!(start.elapsed() < Duration::from_secs(10), "never refused");
            host.turn();
            other.turn();
            refused += logged
                .codes
                .try_iter()
                .filter(|code| code == "net.refused")
                .count();
            thread::sleep(Duration::from_millis(1));
        }
        assert!(other.link.is_none() && host.net().link.is_none());
    }

    /// A peer's command that names the other's player is refused by both peers, which go on in
    /// step, its command for its own player applying: neither player steers the other's.
    #[test]
    fn a_peer_s_command_for_the_other_s_player_is_refused_by_both() {
        let logged = crate::testing::logged();
        let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
        let mut joiner = Peer::joiner(&mut host);
        join(&mut host, &mut joiner);
        joiner.stack_commands(Command::Add {
            player: 0,
            points: 100,
        });
        joiner.stack_commands(Command::Add {
            player: 1,
            points: 10,
        });
        let at = host.next.max(joiner.next) + DELAY + 2;
        meet(&mut host, &mut joiner, at);
        assert_eq!((host.score(), joiner.score()), ([0, 10], [0, 10]));
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
        let codes: Vec<String> = logged.codes.try_iter().collect();
        let refused = codes.iter().filter(|code| *code == "net.bad_command");
        assert_eq!(refused.count(), 2, "one by each peer: {codes:?}");
        assert!(!codes.iter().any(|code| code == "net.desync"), "{codes:?}");
    }

    /// An edit the joiner offers, and a save of a scene file the host's game loaded, apply on both
    /// peers before the same tick, each answered to the peer that offered it; the joiner, which
    /// doesn't look at the files, applies the save as the host does, and keeps the file's text.
    #[cfg(feature = "dev")]
    #[test]
    fn offered_edits_and_scene_saves_apply_on_both_peers() {
        use std::fs;

        use crate::Transform;
        use crate::dirs::tests::Scratch;

        let dir = Scratch::new("net-scene");
        fs::create_dir_all(dir.0.join("scenes")).expect("a directory");
        let path = dir.0.join("scenes").join("level.ron");
        // Seeded apart from the peers, whose worlds would have its entity's ID already.
        let mut file = World::seeded(3);
        let a = file.spawn(Transform::IDENTITY);
        fs::write(&path, file.save_scene(&[a]).expect("saved")).expect("written");
        let game = Game {
            scene: Some(path.clone()),
            ..Game::default()
        };
        let mut host = Peer::host(Context::seeded(1).root(&dir.0), game, 10);
        let mut joiner = Peer::joiner(&mut host);
        join(&mut host, &mut joiner);
        assert_eq!(joiner.cx.scenes.files, host.cx.scenes.files);

        // The joiner's edit.
        let edit = joiner.set("agent", 0, 1000);
        let ticket = joiner.net().offer(edit);
        until(&mut host, &mut joiner, |_, joiner| {
            joiner.net().is_done(ticket)
        });
        let (_, done) = joiner.net().done(ticket).expect("done");
        assert!(matches!(done, Ok(Done::Applied(_))), "{done:?}");
        let at = host.next.max(joiner.next) + 1;
        meet(&mut host, &mut joiner, at);
        assert_eq!(host.score(), [1000, 0]);
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());

        // The scene file saved, before either peer's first look, which finds it changed from the
        // text it loaded as.
        let moved = Transform {
            translation: DVec3::new(1.0, 2.0, 3.0),
            ..Transform::IDENTITY
        };
        file.insert(a, moved).expect("alive");
        let saved = file.save_scene(&[a]).expect("saved");
        fs::write(&path, &saved).expect("written");
        crate::scene_files::look(&mut joiner.cx, &mut joiner.stack);
        assert!(joiner.net().offered.is_empty());
        crate::scene_files::look(&mut host.cx, &mut host.stack);
        assert_eq!(host.net().offered.len(), 1);
        let at = |peer: &Peer| *peer.cx.world().query::<&Transform>().next().expect("one");
        until(&mut host, &mut joiner, |host, joiner| {
            at(host) == moved && at(joiner) == moved
        });
        let met = host.next.max(joiner.next) + 1;
        meet(&mut host, &mut joiner, met);
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
        for peer in [&host, &joiner] {
            assert_eq!(peer.cx.scenes.files[0].text, saved);
            let logs = peer.cx.authors.logs(peer.cx.world());
            let authors: Vec<&str> = logs.map(|(author, _)| author).collect();
            assert!(authors.contains(&"scenes/level.ron"), "{authors:?}");
        }
    }

    /// A joiner runs its host's scripts, which come with its world, whatever its own files hold;
    /// and a script's save, which the host alone looks at, reloads on both peers before the same
    /// tick, the component's fields changing on both.
    #[cfg(feature = "dev")]
    #[test]
    fn a_joiner_runs_its_host_s_scripts_which_reload_on_both_peers_as_they_re_saved() {
        use std::fs;

        use crate::dirs::tests::Scratch;

        let spin = |fields: &str, step: u32| {
            format!(
                "local Spin = gg.declare(\"Spin\", {{ count = 0{fields} }})\n\
                 function Spin.tick(entity, dt)\n\
                     entity[Spin.count] = entity[Spin.count] + {step}\n\
                 end\n\
                 return Spin\n"
            )
        };
        let game = |name: &str, text: &str| {
            let dir = Scratch::new(name);
            fs::create_dir_all(dir.0.join("scripts")).expect("a directory");
            let path = dir.0.join("scripts").join("spin.luau");
            fs::write(&path, text).expect("written");
            let scripts = crate::Scripts::new().expect("Luau starts");
            let game = Game {
                script: Some((scripts, path.clone())),
                ..Game::default()
            };
            (dir, path, game)
        };
        let (host_dir, path, ours) = game("net-script-host", &spin("", 1));
        let mut host = Peer::host(Context::seeded(1).root(&host_dir.0), ours, 10);
        let (joiner_dir, _, theirs) = game("net-script-joiner", &spin("", 100));
        let address = host.net().listening.expect("listening").to_string();
        let cx = Context::seeded(2).root(&joiner_dir.0);
        let mut joiner = Peer::new(&Net::Join(address), cx, theirs);
        join(&mut host, &mut joiner);
        assert_eq!(joiner.cx.hosts.applied(), host.cx.hosts.applied());
        let at = host.next.max(joiner.next) + 5;
        meet(&mut host, &mut joiner, at);
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());

        // Saved with a field more, which the joiner doesn't look for.
        let saved = spin(", turned = true", 2);
        fs::write(&path, &saved).expect("written");
        crate::luau::look(&mut joiner.cx, &mut joiner.stack);
        assert!(joiner.net().offered.is_empty());
        crate::luau::look(&mut host.cx, &mut host.stack);
        assert_eq!(host.net().offered.len(), 1);
        let turned = |peer: &Peer| {
            let world = peer.cx.world();
            let spun = world.entities(Some("Spin")).expect("a type").next();
            world
                .get_field(spun.expect("one"), "Spin", "turned")
                .is_ok()
        };
        until(&mut host, &mut joiner, |host, joiner| {
            turned(host) && turned(joiner)
        });
        let met = host.next.max(joiner.next) + 5;
        meet(&mut host, &mut joiner, met);
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
        let applied = vec![("scripts/spin.luau".to_owned(), saved)];
        assert_eq!(host.cx.hosts.applied(), applied);
        assert_eq!(joiner.cx.hosts.applied(), applied);
    }

    /// A joiner without its host's mod runs the host's, which comes with its world, the peers going
    /// on alike.
    #[test]
    fn a_joiner_without_its_host_s_mod_runs_the_host_s() {
        use std::fs;

        use crate::dirs::tests::Scratch;

        let installed = Scratch::new("net-mods-host");
        let folder = installed.0.join("tally");
        fs::create_dir_all(&folder).expect("a directory");
        let manifest = "(name: \"tally\", version: \"1.0.0\", game: \"Tested\", \
            capabilities: [\"read:Score\"], scripts: [\"tally.luau\"])";
        fs::write(folder.join("mod.ron"), manifest).expect("written");
        let script = "local Tally = gg.declare(\"Tally\", { count = 0 }, { on = \"Score\" })\n\
            function Tally.tick(entity, dt)\n\
                entity[Tally.count] = entity[Tally.count] + 1\n\
            end\n\
            return Tally\n";
        fs::write(folder.join("tally.luau"), script).expect("written");
        let none = Scratch::new("net-mods-joiner");
        let modded = |dir: &Scratch, seed| {
            let mut cx = Context::seeded(seed);
            cx.mods.game = "Tested".to_owned();
            cx.mods.dir = Some(dir.0.clone());
            let game = Game {
                modded: true,
                ..Game::default()
            };
            (cx, game)
        };
        let (cx, game) = modded(&installed, 1);
        let mut host = Peer::host(cx, game, 10);
        let address = host.net().listening.expect("listening").to_string();
        let (cx, game) = modded(&none, 2);
        let mut joiner = Peer::new(&Net::Join(address), cx, game);
        assert!(joiner.cx.mods.applied().is_empty());
        join(&mut host, &mut joiner);
        assert_eq!(joiner.cx.mods.applied(), host.cx.mods.applied());
        assert_eq!(host.cx.mods.applied().len(), 1);
        let at = host.next.max(joiner.next) + 5;
        meet(&mut host, &mut joiner, at);
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
        let world = joiner.cx.world();
        let tallied = world.entities(Some("Tally")).expect("a type").next();
        let count = world.get_field(tallied.expect("one"), "Tally", "count");
        assert_eq!(
            count.expect("its field"),
            crate::reflect::Value::F64(at as f64)
        );
    }

    /// Patches `gg run` gives both peers alike, in lockstep and in a rollback session: the host's,
    /// alone, applies before the tick its delay on from its next; the joiner's, which it got before
    /// it joined, before the first tick of its host's world; and once they've joined, the host's
    /// next applies on both before the tick it chose, the joiner waiting there for its own from `gg
    /// run`, or, in a rollback session, there or past it on the host's inputs predicted, rolling
    /// back to it once its patch has come. Each is answered with the tick it applied before, here a
    /// refusal, as no process takes the tables of a test.
    #[cfg(feature = "dev")]
    #[test]
    fn patches_apply_on_both_peers_before_the_ticks_the_host_chooses() {
        for rollback in [false, true] {
            patches_apply_before_the_ticks_the_host_chooses(rollback);
        }
    }

    #[cfg(feature = "dev")]
    fn patches_apply_before_the_ticks_the_host_chooses(rollback: bool) {
        /// Gives `peer` the patch `table` names, as `gg run` does through the API.
        fn patch(peer: &mut Peer, table: subsecond::JumpTable) -> Ticket {
            let Ok(ticket) = peer.stack.offer_patch(table, peer.next) else {
                panic!("a peer offers its patches");
            };
            ticket
        }
        let table = || subsecond::JumpTable {
            lib: std::env::temp_dir().join("gg-net-patch.dll"),
            map: Default::default(),
            aslr_reference: u64::MAX,
            new_base_address: 0,
            ifunc_count: 0,
        };
        let refused = |done: Result<Done>| {
            let code = done.err().map(|error| error.code());
            assert_eq!(code, Some("api.patch"));
        };
        let delay = if rollback { ROLLBACK_DELAY } else { DELAY };
        let game = Game {
            rollback,
            ..Game::default()
        };
        let mut host = Peer::host(Context::seeded(1), game, 10);
        let first = patch(&mut host, table());
        let chosen = host.next + delay;
        while host.next <= chosen {
            host.turn();
        }
        let (at, done) = host.net().done(first).expect("applied");
        assert_eq!(at, chosen);
        refused(done);

        // The joiner's, from before it joined, which applies as the first tick of its host's world
        // runs, once the host's inputs for it, which follow the world, have come.
        let mut joiner = Peer::joiner(&mut host);
        let theirs = patch(&mut joiner, table());
        let world = host.next;
        join(&mut host, &mut joiner);
        until(&mut host, &mut joiner, |_, joiner| {
            joiner.net().is_done(theirs)
        });
        let (at, done) = joiner.net().done(theirs).expect("applied");
        assert_eq!(at, world);
        refused(done);

        // The host's next, which the joiner waits for from `gg run`, though it has the host's inputs
        // for the tick, as the host goes on.
        let second = patch(&mut host, table());
        let chosen = host.next + delay;
        until(&mut host, &mut joiner, |_, joiner| joiner.next >= chosen);
        until(&mut host, &mut joiner, |host, joiner| {
            let inputs = joiner.net().theirs.get(&chosen);
            host.next > chosen && inputs.is_some_and(|inputs| inputs.whole)
        });
        if rollback {
            let next = joiner.next;
            let predicted = &joiner.net().predicted;
            let past = (chosen..next).all(|tick| predicted.contains_key(&tick));
            assert!(
                past,
                "run past it, to {next}, on the host's inputs predicted alone"
            );
        } else {
            assert_eq!(joiner.next, chosen);
        }
        let theirs = patch(&mut joiner, table());
        until(&mut host, &mut joiner, |host, joiner| {
            host.net().is_done(second) && joiner.net().is_done(theirs)
        });
        for (peer, ticket) in [(&mut host, second), (&mut joiner, theirs)] {
            let (at, done) = peer.net().done(ticket).expect("applied");
            assert_eq!(at, chosen);
            refused(done);
        }
        let at = host.next.max(joiner.next) + 1;
        meet(&mut host, &mut joiner, at);
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    }

    impl Peer {
        /// Queues `command` for the game's next tick.
        fn stack_commands(&mut self, command: Command) {
            let queue = self.stack.commands_of(0).expect("the game queues commands");
            queue
                .push(&command.to_value())
                .expect("the game takes its commands");
        }
    }

    /// A command of [`Painter`]'s: spawns a sprite of this texture.
    #[cfg(feature = "dev")]
    #[derive(Reflect)]
    struct Paint(crate::Texture);

    /// Loads `assets/stone.png` under its context's root as it loads, attaches a sprite of it, and
    /// spawns a sprite of each texture its commands give.
    #[cfg(feature = "dev")]
    #[derive(Default)]
    struct Painter {
        stone: Option<crate::Texture>,
        commands: Vec<Paint>,
    }

    #[cfg(feature = "dev")]
    impl Layer for Painter {
        fn load(&mut self, cx: &mut Context) {
            let root = cx.root.clone().expect("a root");
            let stone = cx.add_texture("stone", root.join("assets/stone.png"));
            self.stone = Some(stone.expect("a texture"));
        }

        fn attach(&mut self, cx: &mut Context) {
            self.commands.push(Paint(self.stone.expect("loaded")));
            self.tick(cx.world_mut(), Tick { number: 0 });
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            for Paint(texture) in self.commands.drain(..) {
                let sprite = crate::Sprite {
                    texture: Some(texture),
                    ..crate::Sprite::default()
                };
                world.spawn((crate::Transform::IDENTITY, sprite));
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.commands)
        }
    }

    /// A joiner whose disk holds none of its host's ID files, as another machine's checkout without
    /// those not yet committed, names its textures as its host does: those it loaded, as its host's
    /// world comes, one both add after it joined, as the host's line says before the command that
    /// names it, and one the host alone loaded where text named it, which the joiner loads as it
    /// reads that command; so the peers' worlds hash alike, and neither parts from the other.
    #[cfg(feature = "dev")]
    #[test]
    fn a_joiner_names_textures_as_its_host_does() {
        use std::fs;

        use crate::dirs::tests::Scratch;

        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let dir = Scratch::new("net-named");
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        let [ours, theirs] = ["host", "joiner"].map(|peer| dir.0.join(peer));
        for root in [&ours, &theirs] {
            fs::create_dir_all(root.join("assets")).expect("a directory");
            for name in ["stone.png", "late.png", "named.png"] {
                fs::write(root.join("assets").join(name), &png).expect("a PNG");
            }
        }
        let (stone, late, named) = ("5f3c9a1e07b2d846", "0a1b2c3d4e5f6071", "7e6d5c4b3a291807");
        for (name, id) in [("stone", stone), ("late", late), ("named", named)] {
            let meta = gg_api::asset::Meta {
                id: id.parse().expect("an ID"),
            };
            let file = ours.join("assets").join(format!("{name}.png.meta"));
            fs::write(file, meta.text()).expect("its ID file");
        }
        let peer = |net: &Net, mut cx: Context| {
            let mut stack = Stack::default();
            stack.push_layer(Painter::default());
            let names = stack.names().to_vec();
            stack.lockstep(Lockstep::start(net, "Tested", &names).expect("started"));
            stack.attach(&mut cx);
            Peer { stack, cx, next: 0 }
        };
        let mut host = peer(
            &Net::Host("127.0.0.1:0".to_owned()),
            Context::seeded(1).root(&ours),
        );
        host.turn();
        let address = host.net().listening.expect("listening").to_string();
        // As a joiner's run starts, its textures by their paths until its host's world comes.
        let mut cx = Context::seeded(2).root(&theirs);
        cx.name_as([]);
        let mut joiner = peer(&Net::Join(address), cx);
        join(&mut host, &mut joiner);
        // The name of the texture a peer loaded from `path`.
        let textured = |peer: &Peer, path: &str| {
            let assets = peer.cx.world().assets();
            let texture = assets.texture(path)?;
            assets.name(texture).map(str::to_owned)
        };
        assert_eq!(
            textured(&joiner, "assets/stone.png").as_deref(),
            Some(stone)
        );

        let add = |peer: &mut Peer| {
            let root = peer.cx.root.clone().expect("a root");
            let added = peer.cx.add_texture("late", root.join("assets/late.png"));
            added.expect("a texture")
        };
        let paint = |peer: &mut Peer, texture| {
            let queue = peer.stack.commands_of(0);
            let queue = queue.expect("the painter queues commands");
            queue
                .push(&Paint(texture).to_value())
                .expect("the painter takes its commands");
        };
        let texture = add(&mut host);
        add(&mut joiner);
        let path = "assets/late.png";
        assert_eq!(textured(&joiner, path).as_deref(), Some(path));
        paint(&mut host, texture);
        let at = host.next.max(joiner.next) + DELAY + 1;
        meet(&mut host, &mut joiner, at);
        assert_eq!(textured(&joiner, path).as_deref(), Some(late));
        // One the host loaded where text named it, as through its tool API, which the joiner's
        // layers never add, loads there as the command naming it is read.
        host.cx
            .load_named([crate::assets::Asset::Texture(named)], false);
        let texture = host.cx.world().assets().texture(named);
        paint(&mut host, texture.expect("loaded"));
        let at = host.next.max(joiner.next) + DELAY + 1;
        meet(&mut host, &mut joiner, at);
        let path = "assets/named.png";
        assert_eq!(textured(&joiner, path).as_deref(), Some(named));
        let sprites = |peer: &Peer| peer.cx.world().query::<&crate::Sprite>().count();
        assert_eq!((sprites(&host), sprites(&joiner)), (3, 3));
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
        let codes: Vec<String> = logged.try_iter().collect();
        let parted = ["net.desync", "net.bad_command"];
        assert!(
            !codes.iter().any(|code| parted.contains(&code.as_str())),
            "{codes:?}"
        );
    }

    /// A scene file's save that names textures no layer added has its host load them before it
    /// offers it, so that their names go out first; the joiner, whose disk holds none of the ID
    /// files, loads each by the path its host's name gives, or stands in for one it hasn't, and
    /// the peers hash alike. A save naming a texture the host can't load isn't offered, as the
    /// joiner might stand in for it. The joiner loads the texture the host's scene named as it
    /// joined, as the host's world comes, and one an edit of the host's names, as it reads it.
    #[cfg(feature = "dev")]
    #[test]
    fn a_joiner_loads_the_textures_its_hosts_scene_saves_name() {
        use std::fs;

        use crate::dirs::tests::Scratch;
        use crate::scene_files::tests::{look_now, save};

        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let dir = Scratch::new("net-loaded");
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        let [ours, theirs] = ["host", "joiner"].map(|peer| dir.0.join(peer));
        let (late, gone, set) = ("0a1b2c3d4e5f6071", "1d2c3b4a59687f0e", "2b3c4d5e6f708192");
        for (root, names) in [
            (&ours, &["late", "set", "gone"][..]),
            (&theirs, &["late", "set"]),
        ] {
            fs::create_dir_all(root.join("assets")).expect("a directory");
            fs::create_dir_all(root.join("scenes")).expect("a directory");
            for name in names {
                fs::write(root.join("assets").join(format!("{name}.png")), &png).expect("a PNG");
            }
        }
        for (name, id) in [("late", late), ("gone", gone), ("set", set)] {
            let meta = gg_api::asset::Meta {
                id: id.parse().expect("an ID"),
            };
            let file = ours.join("assets").join(format!("{name}.png.meta"));
            fs::write(file, meta.text()).expect("its ID file");
        }
        // A scene of an entity of a name, and a sprite of each texture named.
        let scene = |names: &[&str]| {
            let mut text = "(\n    entities: {\n".to_owned();
            text += "        \"0000000000003000\": (\n            Name: (\"a\"),\n        ),\n";
            for (i, name) in names.iter().enumerate() {
                text += &format!(
                    "        \"{:016x}\": (\n            Sprite: (\n                texture: \
                     Some(\"{name}\"),\n            ),\n        ),\n",
                    0x3001 + i
                );
            }
            text + "    },\n)\n"
        };
        let path = ours.join("scenes/level.ron");
        fs::write(&path, scene(&[late])).expect("written");
        let game = Game {
            scene: Some(path.clone()),
            ..Game::default()
        };
        let mut host = Peer::host(Context::seeded(1).root(&ours), game, 10);
        let address = host.net().listening.expect("listening").to_string();
        let mut cx = Context::seeded(2).root(&theirs);
        cx.name_as([]);
        let mut joiner = Peer::new(&Net::Join(address), cx, Game::default());
        join(&mut host, &mut joiner);

        save(&path, &scene(&["assets/none.png"]), 1);
        look_now(&mut host.cx, &mut host.stack);
        assert!(
            host.net().offered.is_empty(),
            "a texture the host can't load"
        );
        save(&path, &scene(&[late, gone]), 2);
        look_now(&mut host.cx, &mut host.stack);
        assert_eq!(host.net().offered.len(), 1);
        let sprites = |peer: &Peer| peer.cx.world().query::<&crate::Sprite>().count();
        until(&mut host, &mut joiner, |host, joiner| {
            sprites(host) == 2 && sprites(joiner) == 2
        });
        let at = host.next.max(joiner.next) + 1;
        meet(&mut host, &mut joiner, at);
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());

        // An edit of the host's naming a texture it loaded for it, as its tool API does.
        host.cx
            .load_named([crate::assets::Asset::Texture(set)], false);
        let texture = host.cx.world().assets().texture(set).expect("loaded");
        let operation = Operation::Set {
            entity: "0000000000003001".parse().expect("an ID"),
            component: "Sprite".into(),
            path: "texture".to_owned(),
            value: Value::Option(Some(Box::new(Value::Texture(texture)))),
        };
        let edit = Edit::Apply {
            author: "agent".to_owned(),
            operations: vec![operation],
        };
        let ticket = host.net().offer(edit);
        until(&mut host, &mut joiner, |host, _| host.net().is_done(ticket));
        let (_, done) = host.net().done(ticket).expect("done");
        assert!(matches!(done, Ok(Done::Applied(_))), "{done:?}");
        let at = host.next.max(joiner.next) + 1;
        meet(&mut host, &mut joiner, at);
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
        let assets = joiner.cx.world().assets();
        let loaded = |name| {
            assets
                .texture(name)
                .and_then(|texture| assets.path(texture))
        };
        let paths = (loaded(late), loaded(set));
        assert_eq!(paths, (Some("assets/late.png"), Some("assets/set.png")));
        let codes: Vec<String> = logged.try_iter().collect();
        let parted = ["net.desync", "net.bad_command"];
        assert!(
            !codes.iter().any(|code| parted.contains(&code.as_str())),
            "{codes:?}"
        );
        let stood_in = codes.iter().filter(|code| *code == "asset.stand_in");
        assert_eq!(stood_in.count(), 1, "{codes:?}");
    }

    /// A peer keeps the other's inputs within a second of ticks past the first it hasn't had whole:
    /// a packet holding inputs past them, as one from elsewhere at the other's address may, or a
    /// tick no run reaches, isn't one, which loses the other, those before taken.
    #[test]
    fn a_peer_keeps_the_other_s_inputs_to_a_second() {
        let net = Net::Join("127.0.0.1:1".to_owned());
        let mut peer = Lockstep::start(&net, "Tested", &[]).expect("started");
        assert!(peer.read_ticks(b"a 0 0\nt 0\nt 63\n").is_some());
        peer.came_whole();
        assert_eq!(peer.whole, 1);
        for far in ["a 0 0\nt 2\nt 65\n", "a 0 18446744073709551615\n"] {
            assert!(peer.read_ticks(far.as_bytes()).is_none(), "{far}");
        }
        let kept: Vec<u64> = peer.theirs.keys().copied().collect();
        assert_eq!(kept, [0, 2, 63]);
    }

    /// A command that shows a texture.
    #[derive(Reflect)]
    struct Show(crate::Texture);

    /// A tick's commands fill their layer's queue each alone: one that isn't one, or that the layer
    /// refuses, is refused, logged with its player, and those after it are queued still. A server's
    /// client's name only the assets the server has loaded, one naming another refused before
    /// anything loads it, where a peer's loads what it names, standing in for what can't be, as a
    /// replay's does.
    #[test]
    fn commands_fill_their_queue_each_alone_a_client_s_naming_only_what_s_loaded() {
        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let mut cx = Context::seeded(1);
        let missing = "assets/missing.png";
        let shows = format!("Struct([(\"0\", Texture(\"{missing}\"))])");
        let queued = |player: u8, remote: bool, text: &str| Queued {
            layer: 0,
            player: Some(player),
            remote,
            text: text.to_owned(),
        };
        let fill = |commands: Vec<Queued>, cx: &mut Context| {
            let merged = Merged {
                commands,
                ..Merged::default()
            };
            let mut queue: Vec<Show> = Vec::new();
            merged.fill(0, "Shows", &mut queue, cx);
            queue.len()
        };
        let loaded = |cx: &Context| cx.world().assets().has(&Asset::Texture(missing));
        let refused = [queued(1, true, "Struct("), queued(2, true, &shows)];
        assert_eq!(fill(refused.into(), &mut cx), 0);
        assert!(!loaded(&cx), "a client's command loads nothing");
        let codes: Vec<String> = logged.try_iter().collect();
        let bad = codes.iter().filter(|code| *code == "net.bad_command");
        assert_eq!(bad.count(), 1, "one for the layer and tick: {codes:?}");
        let taken = [
            queued(1, false, "UInt(3)"),
            queued(0, false, &shows),
            queued(2, true, &shows),
        ];
        assert_eq!(fill(taken.into(), &mut cx), 2);
        assert!(loaded(&cx), "a peer's command stands in for it");
    }

    /// The entities a joiner and its host spawn after it joins take the same slots, as code that
    /// orders by [`Entity`] needs: the host's authors' logs go as it sends its world, warning how
    /// many edits went, and the slots they kept vacant for their undos are freed, as the joiner's
    /// load frees them; and no undo reaches past the world the joiner started from.
    #[test]
    fn a_joiner_and_its_host_spawn_into_the_same_slots() {
        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        // Entities made and deleted by an edit, as a peer's are between ticks, behind the session's
        // back.
        let edit = |peer: &mut Peer, operations: Vec<Operation>| {
            let edit = Edit::Apply {
                author: "editor".to_owned(),
                operations,
            };
            match peer.cx.apply_edit(edit).expect("applied") {
                Done::Applied(made) => made,
                Done::Stepped(_) => panic!("an apply"),
            }
        };
        let create = || Operation::Create {
            entity: None,
            parent: None,
            components: Vec::new(),
        };
        let spawned = |peer: &mut Peer| {
            let made = edit(peer, vec![create(), create(), create(), create()]);
            let world = peer.cx.world();
            let made = made.into_iter().map(|id| world.entity(id).expect("alive"));
            made.collect::<Vec<_>>()
        };
        let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
        let gone = edit(&mut host, vec![create(), create(), create()]);
        let delete = gone.into_iter().map(|entity| Operation::Delete { entity });
        edit(&mut host, delete.collect());
        let mut joiner = Peer::joiner(&mut host);
        join(&mut host, &mut joiner);
        let at = host.next.max(joiner.next) + 1;
        meet(&mut host, &mut joiner, at);
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
        let undo = Edit::Undo {
            author: "editor".to_owned(),
        };
        let undone = host.cx.apply_edit(undo).expect("stepped");
        assert_eq!(undone, Done::Stepped(None));
        assert_eq!(spawned(&mut host), spawned(&mut joiner));
        let cleared = logged
            .try_iter()
            .filter(|code| code == "edits.history_cleared");
        assert_eq!(cleared.count(), 1);
    }

    /// A renewal, as a lockstep host's as it sends its joiner its world, refuses the entries of
    /// every log from before, a layer's as the Inspector keeps its own among them, not only the
    /// authors': its undo is refused, and its clear frees nothing the renewal freed, so that no two
    /// entities take one slot. It's an edit, which a recording keeps, so that a replay renews its
    /// world where the run did.
    #[test]
    fn a_renewal_refuses_every_log_s_entries_from_before() {
        let mut cx = Context::seeded(1);
        let world = cx.world_mut();
        let gone = world.spawn(crate::Transform::IDENTITY);
        let id = world.id(gone).expect("an ID");
        let mut log = crate::Log::default();
        let delete = Operation::Delete { entity: id };
        log.apply(world, delete).expect("deleted");
        let renew = Edit::from_value(&Edit::Renew.to_value()).expect("an edit");
        cx.apply_edit(renew).expect("renewed");
        let world = cx.world_mut();
        assert!(log.undo(world).is_err(), "refused");
        log.clear(world);
        let [a, b] = [0, 1].map(|_| world.spawn(crate::Transform::IDENTITY));
        assert_ne!(a.slot(), b.slot());
        assert!(world.entity(id).is_none());
    }
}
