//! Lockstep between two peers, as [Networking](../../../ARCHITECTURE.md#networking) describes: a
//! host, player 0, that listens, and a joiner, player 1, that connects and starts from the host's
//! world; each tick's inputs, every layer's commands and the edits that apply before it, sent
//! [`DELAY`] ticks ahead and run once both peers' are in; the world's hash after each tick,
//! compared; and, in development builds, the patches of the game's code, which the host has apply
//! at a tick of its choosing.
//!
//! Messages are lines of text over TCP, each a thread of the connection's reads, written by the
//! main thread:
//!
//! ```text
//! hello\tgg-lockstep 2\tShards\t0.1.0 debug dev\tShards Inspector
//!                                                    the joiner, as it connects
//! refused <why>                                      the host, refusing it, before it closes
//! world 1234 2                                       the host's world, at tick 1234, its code
//! gg-snapshot 1                                        2 patches on, as a snapshot file's text,
//! ...                                                  to its `end`
//! end
//! joined 1234                                        the joiner, once it has restored it
//! c 1238 0 Variant("Move", [...])                    a command for tick 1238, of layer 0
//! e 1238 Variant("Apply", [...])                     an edit before tick 1238
//! s 1238 List([String("scenes/arena.ron"), ...])     a scene file's save before tick 1238
//! t 1238                                             the inputs for tick 1238 are whole
//! h 1234 58183b6352a09a44 fb695ce29daa8479           the world's hash after tick 1234
//! p 3 1240                                           the host's third patch, before tick 1240
//! n List([String("5f3c9a1e07b2d846"), String(...)])  the name the host gave a texture it added,
//!                                                      and the path it names
//! ```
//!
//! Values are written as recordings write them, entities by their IDs and textures by their names,
//! and read into values only at their tick, against the world then. The joiner names its textures
//! as its host does, whatever ID files its own disk holds: as its host's world gives them, and as
//! the host's `n` lines give the names of those it adds after, before the inputs that may name
//! them. A peer's ticks allocate, for their inputs' text and the hash's lines, as recorded ticks
//! do.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write as _};
use std::mem;
#[cfg(test)]
use std::net::SocketAddr;
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
#[cfg(feature = "dev")]
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use gg_core::{Error, Result};
use tracing::{error, info, warn};

use crate::args::Net;
use crate::assets::Counts;
use crate::edits::{Done, Edit};
use crate::layer::Commands;
use crate::recording::text::{self, Text};
use crate::reflect::Value;
use crate::{Context, World, WorldHash};

/// How many ticks ahead a peer sends its inputs: 62.5 ms, so that they reach the other peer before
/// it needs them across a LAN, at the cost of as long between a press and what it does.
pub(crate) const DELAY: u64 = 4;

/// The hello's first part: the protocol and its version, which a change to its lines takes to the
/// next.
const PROTOCOL: &str = "gg-lockstep 2";

/// How long a joiner waits between tries at connecting to its host.
const RETRY: Duration = Duration::from_millis(250);

/// How long a joiner its host refused waits before trying it again, as a host of another build
/// refuses it until one of them is rebuilt.
const REFUSED: Duration = Duration::from_secs(5);

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
/// the layer's index, and what applies before the tick, with the ticket of each this peer offered.
#[derive(Default)]
struct Inputs {
    commands: Vec<(usize, String)>,
    edits: Vec<(Option<Ticket>, String)>,
    scenes: Vec<(Option<Ticket>, String)>,
    whole: bool,
}

/// What a tick runs on, both peers' inputs merged, the host's first: what applies before it, each
/// with its ticket if this peer offered it, and each layer's commands, as text.
#[derive(Default)]
pub(crate) struct Merged {
    pub(crate) inputs: Vec<(Option<Ticket>, Outside)>,
    commands: Vec<(usize, String)>,
    /// The patches that apply before the tick, in order, with their tickets.
    #[cfg(feature = "dev")]
    pub(crate) patches: Vec<(Ticket, subsecond::JumpTable)>,
}

impl Merged {
    /// Empties `queue`, layer `layer`'s, and fills it with the layer's commands for the tick, read
    /// against `cx`'s world, once the textures they name are loaded, as a run that follows another
    /// loads them.
    ///
    /// Fails with `net.bad_command` for a command the layer refuses, or that isn't one.
    pub(crate) fn fill(
        &self,
        layer: usize,
        queue: &mut dyn Commands,
        cx: &mut Context,
    ) -> Result<()> {
        queue.clear();
        let commands = self.commands.iter().filter(|(of, _)| *of == layer);
        for (_, command) in commands {
            let value = Text::parse(command, 1).map(|text| {
                cx.load_followed(&text);
                text.value(cx.world())
            });
            value
                .and_then(|value| queue.push(&value))
                .map_err(|error| {
                    let message = format!("layer {layer} refuses a peer's command: {error}");
                    Error::new("net.bad_command", message)
                })?;
        }
        Ok(())
    }
}

/// A connection to the other peer: what writes to it, and the lines its thread reads.
struct Link {
    to: TcpStream,
    from: Receiver<String>,
    /// Set once the link counts: for the host, once the joiner has said it joined, before which
    /// what it sends was sent against a world it no longer has; for the joiner, once it has the
    /// host's world.
    joined: bool,
}

impl Link {
    /// Starts reading `stream`'s lines on a thread of their own.
    fn new(stream: TcpStream) -> std::io::Result<Self> {
        stream.set_nodelay(true)?;
        let reader = BufReader::new(stream.try_clone()?);
        let (lines, from) = mpsc::channel();
        thread::Builder::new()
            .name("gg-net".to_owned())
            .spawn(move || {
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if lines.send(line).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            to: stream,
            from,
            joined: false,
        })
    }

    /// Sends `text`, whole lines, failing as the connection does.
    fn send(&mut self, text: &str) -> std::io::Result<()> {
        self.to.write_all(text.as_bytes())
    }
}

impl Drop for Link {
    /// Closes the connection, which the thread reading it holds too: the other peer reads its end,
    /// after what was sent, and closes its side, which ends the thread.
    fn drop(&mut self) {
        let _ = self.to.shutdown(Shutdown::Both);
    }
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
    /// This peer's inputs, by tick, kept until the tick runs.
    ours: BTreeMap<u64, Inputs>,
    /// The other peer's, by tick.
    theirs: BTreeMap<u64, Inputs>,
    /// The tick from which the other peer's inputs are needed: before it, they're empty; `None`
    /// while there's no other peer, for the host, which plays on alone.
    from: Option<u64>,
    /// What this peer offered since its last tick, with the ticket of each that's answered, which
    /// goes out with its inputs for the tick [`DELAY`] on from its next.
    offered: Vec<(Option<Ticket>, Outside)>,
    /// The last ticket given.
    #[cfg(feature = "dev")]
    tickets: Ticket,
    /// What the edits and patches this peer offered did, by ticket, with the tick they applied
    /// before.
    done: Vec<(Ticket, u64, Result<Done>)>,
    /// The world's hash after each tick, this peer's and the other's, until they're compared.
    hashes: [BTreeMap<u64, WorldHash>; 2],
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
    /// Fails with `net.start` if the host can't listen at its address.
    pub(crate) fn start(net: &Net, app: &'static str, layers: &[&str]) -> Result<Self> {
        #[cfg(test)]
        let mut listening = None;
        let find = match net {
            Net::Host(address) => {
                let listener = TcpListener::bind(address.as_str()).map_err(|error| {
                    let message = format!("can't listen at {address} for a lockstep peer");
                    Error::new("net.start", message).with_source(error)
                })?;
                #[cfg(test)]
                {
                    listening = listener.local_addr().ok();
                }
                let at = listener.local_addr();
                let at = at.map_or_else(|_| address.clone(), |at| at.to_string());
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
                        let message = "can't start the thread that accepts peers".to_owned();
                        Error::new("net.start", message).with_source(error)
                    })?;
                info!(address = %at, "Hosting a lockstep session");
                Find::Host(find)
            }
            Net::Join(address) => Find::Join {
                address: address.clone(),
                connecting: None,
            },
        };
        let host = matches!(find, Find::Host(_));
        Ok(Self {
            find,
            #[cfg(test)]
            listening,
            app,
            layers: layers.join(" "),
            link: None,
            coming: None,
            owed: false,
            parted: false,
            ours: BTreeMap::new(),
            theirs: BTreeMap::new(),
            from: (!host).then_some(0),
            offered: Vec::new(),
            #[cfg(feature = "dev")]
            tickets: 0,
            done: Vec::new(),
            hashes: Default::default(),
            #[cfg(feature = "dev")]
            patches: Patches::default(),
            #[cfg(feature = "dev")]
            named: (0, 0, 0),
            #[cfg(feature = "dev")]
            names: Vec::new(),
        })
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

    /// Between ticks, as a frame starts: takes the lines read and the connections made since the
    /// last turn, and returns what the peer must do of them: send its world, if it's the host and
    /// owes one, or restore the host's, if it's the joiner and has one whole. Logs a connection
    /// lost, a peer refused and a desync.
    pub(crate) fn turn(&mut self) -> Turn {
        let mut lines = Vec::new();
        let mut lost = false;
        if let Some(link) = &self.link {
            loop {
                match link.from.try_recv() {
                    Ok(line) => lines.push(line),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        lost = true;
                        break;
                    }
                }
            }
        }
        let mut turn = Turn::None;
        for line in lines {
            if let Some(world) = self.read(&line) {
                turn = Turn::Restore(world);
            }
            if self.link.is_none() {
                break;
            }
        }
        if lost && self.link.is_some() {
            self.lose("the connection closed");
        }
        // After the link's lines, so that a joiner that closed its link and connects again finds
        // the host's place free, as the host has seen the close.
        self.find_peer();
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
                    match Link::new(stream) {
                        Ok(link) if self.link.is_none() => self.link = Some(link),
                        // A second joiner, refused before its hello.
                        Ok(mut link) => {
                            let _ = link.send("refused the session has its two players\n");
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
                let made =
                    connecting.get_or_insert_with(|| connect(address.clone(), Duration::ZERO));
                let Ok(stream) = made.try_recv() else { return };
                *connecting = None;
                (stream, address.clone())
            }
        };
        let (stream, address) = made;
        let hello = format!(
            "hello\t{PROTOCOL}\t{}\t{}\t{}\n",
            self.app,
            engine(),
            self.layers
        );
        match Link::new(stream).and_then(|mut link| link.send(&hello).map(|()| link)) {
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
        if let Some((_, text)) = &mut self.coming {
            text.push_str(line);
            text.push('\n');
            if line != "end" {
                return None;
            }
            let (tick, text) = self.coming.take()?;
            self.came(tick);
            return Some((tick, text));
        }
        // The hello's parts are split by tabs, the rest's by spaces.
        let (kind, rest) = line.split_once([' ', '\t']).unwrap_or((line, ""));
        let joined = self.link.as_ref().is_some_and(|link| link.joined);
        match kind {
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
                    *connecting = Some(connect(address.clone(), REFUSED));
                }
            }
            "world" if !self.hosts() => {
                let world = rest.split_once(' ').and_then(|(tick, patches)| {
                    Some((tick.parse::<u64>().ok()?, patches.parse::<usize>().ok()?))
                });
                match world {
                    Some((tick, patches)) => {
                        #[cfg(feature = "dev")]
                        {
                            self.patches.hosts = patches;
                        }
                        // Builds without patches are refused by hosts with them.
                        #[cfg(not(feature = "dev"))]
                        let _ = patches;
                        self.coming = Some((tick, String::new()));
                    }
                    None => self.lose("the host's world names no tick or patches"),
                }
            }
            "joined" if self.hosts() => {
                if let Some(link) = &mut self.link {
                    link.joined = true;
                    info!(tick = rest, "The joiner joined the lockstep session");
                }
            }
            // Sent against a world the joiner no longer has, as the host's comes.
            _ if !joined => {}
            "c" | "e" | "s" => {
                let mut parts = rest.splitn(if kind == "c" { 3 } else { 2 }, ' ');
                let tick = parts.next().and_then(|tick| tick.parse::<u64>().ok());
                let layer = if kind == "c" {
                    parts.next().and_then(|layer| layer.parse::<usize>().ok())
                } else {
                    Some(0)
                };
                let value = parts.next().map(str::to_owned);
                match (tick, layer, value) {
                    (Some(tick), Some(layer), Some(value)) => {
                        let inputs = self.theirs.entry(tick).or_default();
                        match kind {
                            "c" => inputs.commands.push((layer, value)),
                            "e" => inputs.edits.push((None, value)),
                            _ => inputs.scenes.push((None, value)),
                        }
                    }
                    _ => self.lose(&format!("the peer sent a line that isn't one: {line}")),
                }
            }
            "t" => match rest.parse::<u64>() {
                Ok(tick) => self.theirs.entry(tick).or_default().whole = true,
                Err(_) => self.lose(&format!("the peer sent a line that isn't one: {line}")),
            },
            "h" => {
                let hash = rest.split_once(' ').and_then(|(tick, hash)| {
                    let (content, layout) = hash.split_once(' ')?;
                    let hex = |text: &str| u64::from_str_radix(text, 16).ok();
                    let hash = WorldHash {
                        content: hex(content)?,
                        layout: hex(layout)?,
                    };
                    Some((tick.parse::<u64>().ok()?, hash))
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
                    Some((number.parse::<usize>().ok()?, tick.parse::<u64>().ok()?))
                });
                match due {
                    Some((number, tick)) if number > 0 => self.patches.due.push((number, tick)),
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

    /// Takes the joiner's hello, `line`, refusing it unless it's of this protocol, app, engine
    /// and layers; once taken, the host owes it its world.
    fn hello(&mut self, line: &str) {
        let parts: Vec<&str> = line.split('\t').collect();
        let ours = [PROTOCOL, self.app, &engine(), self.layers.as_str()];
        let why = match parts.get(1..) {
            Some(theirs) if theirs == &ours[..] => None,
            Some([protocol, ..]) if *protocol != PROTOCOL => {
                Some(format!("it speaks {protocol}, and the host {PROTOCOL}"))
            }
            Some([_, app, engine, layers]) => Some(format!(
                "it's app {app}, engine {engine}, layers `{layers}`, and the host app {}, engine \
                 {}, layers `{}`",
                ours[1], ours[2], ours[3]
            )),
            _ => Some("its hello isn't one".to_owned()),
        };
        match why {
            None => self.owed = true,
            Some(why) => {
                warn!(code = "net.refused", "Refused a lockstep peer: {why}");
                if let Some(link) = &mut self.link {
                    let _ = link.send(&format!("refused {why}\n"));
                }
                self.drop_link();
            }
        }
    }

    /// The host, between ticks, the next to run being `next`: sends the joiner `world`, a snapshot
    /// file's text, which names its assets as the host does, there being `assets` of them,
    /// with how many patches its code is on; its inputs for each tick from `next` to the tick
    /// [`DELAY`] on, which it has sent before, or has none of within `DELAY` ticks of its start or
    /// restore; and its patches that are to apply. The joiner's inputs are needed from the tick
    /// `DELAY` on from `next`, the first it can reach.
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
        let mut text = format!("world {next} {patches}\n{world}");
        // Before the inputs, as a patch's line comes before the inputs for its tick otherwise, so
        // that the joiner has it before it can run the tick.
        #[cfg(feature = "dev")]
        for (number, at) in &self.patches.due {
            writeln!(text, "p {number} {at}").expect("a string takes any text");
        }
        let none = Inputs::default();
        for tick in next..next + DELAY {
            write_inputs(&mut text, tick, self.ours.get(&tick).unwrap_or(&none));
        }
        self.theirs.clear();
        self.hashes = Default::default();
        self.parted = false;
        self.from = Some(next + DELAY);
        if let Some(link) = &mut self.link {
            link.joined = false;
            if let Err(error) = link.send(&text) {
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
        self.parted = false;
        self.from = Some(tick);
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
        if let Err(error) = link.send(&format!("joined {tick}\n")) {
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

    /// Whether tick `tick` can run: it has the other peer's inputs, or needs none, and the patches
    /// due before it, which a joiner may have from its host before it has them from `gg run`.
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
        match self.from {
            None => true,
            Some(from) if tick < from => true,
            Some(_) => self.theirs.get(&tick).is_some_and(|inputs| inputs.whole),
        }
    }

    /// As tick `tick` starts, once it's [`ready`](Self::ready): sends this peer's inputs for the
    /// tick [`DELAY`] on, `queues`' commands, each layer's by its index, written against `cx`'s
    /// world, and what it offered since its last tick, emptying them; and returns what the tick
    /// runs on, the edits read once the textures they name are loaded, as a run that follows
    /// another loads them.
    pub(crate) fn start_tick<'q>(
        &mut self,
        tick: u64,
        queues: impl Iterator<Item = (usize, &'q mut dyn Commands)>,
        cx: &mut Context,
    ) -> Merged {
        let world = cx.world();
        let ahead = tick + DELAY;
        let mut inputs = Inputs {
            whole: true,
            ..Inputs::default()
        };
        for (layer, queue) in queues {
            for i in 0..queue.len() {
                let mut value = String::new();
                text::write(&mut value, &queue.value(i), world);
                inputs.commands.push((layer, value));
            }
            queue.clear();
        }
        for (ticket, input) in mem::take(&mut self.offered) {
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
            write_inputs(&mut text, ahead, &inputs);
            self.send(&text);
        }
        self.ours.insert(ahead, inputs);
        let ours = self.ours.remove(&tick).unwrap_or_default();
        // What's left of ticks before is from before the peers' worlds were made the same.
        self.theirs = self.theirs.split_off(&tick);
        let theirs = self.theirs.remove(&tick).unwrap_or_default();
        let (host, joiner) = if self.hosts() {
            (ours, theirs)
        } else {
            (theirs, ours)
        };
        let mut merged = Merged::default();
        for peer in [host, joiner] {
            merged.commands.extend(peer.commands);
            for (ticket, edit) in peer.edits {
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
            for (ticket, scene) in peer.scenes {
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
        self.due_patches(tick, &mut merged);
        merged
    }

    /// Once tick `tick` has run, leaving `world`: sends its hash, if there's another peer, and
    /// compares it with theirs.
    pub(crate) fn end_tick(&mut self, tick: u64, world: &World) {
        if self.from.is_none() {
            return;
        }
        let hash = world.hash();
        self.send(&format!("h {tick} {hash}\n"));
        self.hashes[0].insert(tick, hash);
        self.compare();
    }

    /// Compares the hashes both peers have after the same tick, dropping them, unless the peers
    /// have parted; a pair that differs logs `net.desync`, and has the host send its world again.
    fn compare(&mut self) {
        if self.parted {
            self.hashes = Default::default();
            return;
        }
        let hosts = self.hosts();
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
                break;
            }
        }
    }

    /// Sends `text` to the other peer, losing it if that fails.
    fn send(&mut self, text: &str) {
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
        self.owed = false;
        self.parted = false;
        self.theirs.clear();
        self.hashes = Default::default();
        if self.hosts() {
            self.from = None;
        }
    }
}

#[cfg(feature = "dev")]
impl Lockstep {
    /// Offers the patch `table` names, `gg run`'s next for this process, the next tick to run
    /// being `next`, and returns its ticket, which [`done`](Self::done) gives its result with. The
    /// host has it apply before the tick [`DELAY`] on, which neither peer has run, telling its
    /// joiner, which has it apply then too; a joiner has it apply as its host says, or before the
    /// first tick of its host's world, if its host had applied it.
    pub(crate) fn offer_patch(&mut self, table: subsecond::JumpTable, next: u64) -> Ticket {
        self.tickets += 1;
        self.patches.got.push(Some((table, self.tickets)));
        if self.hosts() {
            let (number, at) = (self.patches.got.len(), next + DELAY);
            self.patches.due.push((number, at));
            if self.from.is_some() {
                self.send(&format!("p {number} {at}\n"));
            }
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

/// What a peer must do between ticks, as [`Lockstep::turn`] says.
pub(crate) enum Turn {
    None,
    /// The host owes its joiner its world.
    Send,
    /// The joiner has its host's world: the tick it's at and its text.
    Restore((u64, String)),
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

/// Connects to `address` on a thread of its own, after `after`, trying again every [`RETRY`] until
/// it does, and returns where the connection comes.
fn connect(address: String, after: Duration) -> Receiver<TcpStream> {
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
                    info!(address = %address, "Waiting for the lockstep host");
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

/// Writes `inputs`, a peer's for tick `tick`, after `out`, as their lines.
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
    writeln!(out, "t {tick}").expect(written);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Instant;

    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::layer::Stack;
    use crate::testing::Codes;
    use crate::{Component, Context, Layer, Operation, Reflect, Tick};

    #[derive(Clone, Component)]
    struct Score {
        points: [u32; 2],
    }

    #[derive(Reflect)]
    enum Command {
        Add { player: u8, points: u32 },
    }

    /// A game of two players, whose commands add to their scores, and which loads the scene file at
    /// `scene`, if it's given one.
    #[derive(Default)]
    struct Game {
        commands: Vec<Command>,
        scene: Option<PathBuf>,
    }

    impl Layer for Game {
        fn attach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn(Score { points: [0; 2] });
            if let Some(scene) = &self.scene {
                cx.load_scene_file(scene).expect("loaded");
            }
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            for Command::Add { player, points } in self.commands.drain(..) {
                for score in world.query_mut::<&mut Score>() {
                    score.points[usize::from(player)] += points;
                }
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
        fn new(net: &Net, mut cx: Context, game: Game) -> Self {
            let mut stack = Stack::default();
            stack.push_layer(game);
            let names = stack.names().to_vec();
            stack.lockstep(Lockstep::start(net, "Tested", &names).expect("started"));
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

        /// A joiner of `host`'s session, which hasn't connected yet.
        fn joiner(host: &mut Self) -> Self {
            let address = host.net().listening.expect("listening").to_string();
            let joiner = Self::new(&Net::Join(address), Context::seeded(2), Game::default());
            assert_eq!(joiner.cx.player(), Some(1));
            joiner
        }

        /// A frame's turn and the tick it runs, if the other peer's inputs have come.
        fn turn(&mut self) {
            if let Some(ticks) = self.stack.net_turn(&mut self.cx, self.next) {
                self.next = ticks;
            }
            if self.stack.tick(&mut self.cx, Tick { number: self.next }) {
                self.next += 1;
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

    /// Turns both peers until both are at tick `at`, the one ahead waiting.
    fn meet(host: &mut Peer, joiner: &mut Peer, at: u64) {
        let start = Instant::now();
        while host.next < at || joiner.next < at {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the peers never met"
            );
            if host.next < at {
                host.turn();
            }
            if joiner.next < at {
                joiner.turn();
            }
            thread::sleep(Duration::from_millis(1));
        }
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

    /// A joiner starts from the host's world, joining within [`DELAY`] ticks of the host's start,
    /// before which the host has sent no inputs; from then each tick runs on both players' commands
    /// on both peers, which hash alike. Worlds that go apart, as commands go both ways, are named
    /// at their tick once by each peer, and made the same again once.
    #[test]
    fn two_peers_run_each_tick_on_both_players_inputs() {
        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
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

        // The host's world changed behind the session's back, the players playing on.
        play(&mut host, &mut joiner, 10);
        let edit = host.set("cheat", 1, 5);
        host.cx.apply_edit(edit).expect("applied");
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

    /// An edit the joiner offers, and a save of a scene file the host's game loaded, apply on both
    /// peers before the same tick, each answered to the peer that offered it; the joiner, which
    /// doesn't look at the files, applies the save as the host does, and keeps the file's text.
    #[cfg(feature = "dev")]
    #[test]
    fn offered_edits_and_scene_saves_apply_on_both_peers() {
        use std::fs;

        use gg_core::math::Vec3;

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
            translation: Vec3::new(1.0, 2.0, 3.0),
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

    /// Patches `gg run` gives both peers alike: the host's, alone, applies before the tick
    /// [`DELAY`] on from its next; the joiner's, which it got before it joined, before the first
    /// tick of its host's world; and once they've joined, the host's next applies on both before the
    /// tick it chose, the joiner waiting there for its own from `gg run`. Each is answered with the
    /// tick it applied before, here a refusal, as no process takes the tables of a test.
    #[cfg(feature = "dev")]
    #[test]
    fn patches_apply_on_both_peers_before_the_ticks_the_host_chooses() {
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
        let mut host = Peer::host(Context::seeded(1), Game::default(), 10);
        let first = patch(&mut host, table());
        let chosen = host.next + DELAY;
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
        let chosen = host.next + DELAY;
        until(&mut host, &mut joiner, |_, joiner| joiner.next == chosen);
        until(&mut host, &mut joiner, |host, joiner| {
            let inputs = joiner.net().theirs.get(&chosen);
            host.next > chosen && inputs.is_some_and(|inputs| inputs.whole)
        });
        assert_eq!(joiner.next, chosen);
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
}
