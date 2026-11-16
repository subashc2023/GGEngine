//! A peer of a lockstep session, as [`net`](super) describes it.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::hash::{BuildHasher, RandomState};
use std::mem;
#[cfg(test)]
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use gg_core::{Error, Result};
use tracing::{debug, error, info, warn};

#[cfg(doc)]
use super::handshake::Heard;
use super::handshake::{
    Accepted, Answer, Handshaken, Looking, connect, engine, hello, listen, port_of,
};
use super::link::Link;
#[cfg(doc)]
use super::link::Trip;
use super::{
    AHEAD, Code, DELAY, FALLS, Inputs, LEAST_DELAY, MAX_PACKET_TEXT, MAX_WORLD, MOST_DELAY,
    MOST_PACE, MOST_PREDICTED, Merged, Outside, PACE, PREDICTED, Queued, REFUSED, ROLLBACK_DELAY,
    SessionPhase, Stalls, Ticket, Turn, optional, read_batches, tick_of, write_inputs,
};
#[cfg(doc)]
use super::{Build, SessionStatus};
use crate::args::{Conditions, Net, Toward};
use crate::assets::Counts;
use crate::edits::{Done, Edit};
use crate::layer::Commands;
use crate::recording::text::{self, Text};
use crate::reflect::Value;
use crate::{Context, Sound, Tick, World, WorldHash};

/// The hello's first part: the protocol and its version, which a change to its lines or its packets
/// takes to the next, a lockstep session's or a rollback session's, which both peers' must be.
pub(super) const LOCKSTEP: &str = "gg-lockstep 9";
const ROLLBACK: &str = "gg-rollback 9";

/// A tick run on the other peer's inputs predicted, which rollback runs again from if they hold
/// something: the world as it started, and the sounds its runs played, which a run again doesn't
/// play again.
pub(super) struct Predicted {
    start: World,
    played: Vec<Sound>,
}

/// How a peer finds the other: the host by the connections its listener accepts and its relay
/// pairs it with, the joiner by connecting to its host's address or coming for its room at its
/// relay, again each time it loses it, by the session's code.
pub(super) enum Find {
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
    pub(super) fn again(&mut self) {
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
    pub(super) find: Find,
    /// Where the host listens, as the OS has it: for tests, which listen where it picks.
    #[cfg(test)]
    pub(super) listening: Option<SocketAddr>,
    /// The session's code, written as [`Code`]'s `Display` writes it, for [`SessionStatus`]: the
    /// session's key, logged nowhere.
    pub(super) code: Arc<str>,
    /// The joiner's: why its host last refused it, until the host's world comes.
    refused: Option<Arc<str>>,
    /// The joiner's: whether its host's world has come, so that a link lost after is its host lost.
    played: bool,
    pub(super) app: &'static str,
    /// The run's build, as [`Build::name`] names it, which both peers' must be: a test's own, to
    /// stand for another's.
    pub(super) build: String,
    /// The layers' names, from the bottom up, which both peers' must be.
    pub(super) layers: String,
    pub(super) link: Option<Link>,
    /// The joiner's world as it comes, the tick it's at and its lines so far.
    coming: Option<(u64, String)>,
    /// Set while the host owes its joiner its world, as it joins or after a desync.
    pub(super) owed: bool,
    /// Set from a desync until the peers' worlds are made the same again, while their hashes
    /// aren't compared.
    pub(super) parted: bool,
    /// How many ticks ahead this peer sends its inputs, and the most it runs ahead of the other's
    /// inputs, on them predicted: lockstep's, none, or a rollback session's. A lockstep peer's
    /// delay and a rollback peer's window follow the link, as [`follow_link`](Self::follow_link)
    /// says.
    pub(super) delay: u64,
    pub(super) window: u64,
    /// The delays or windows the link has needed in the last [`FALLS`], as a sliding window's
    /// maximum keeps them: each with when it was needed, each needed less than those before it and
    /// later, so that the first is the most, and the values from the least to the most each appear
    /// once at most, 15 of them.
    needs: VecDeque<(Instant, u64)>,
    /// The least and the most delay or window this peer has had with the other there; how many
    /// ticks waited for the other's inputs, and the last that did; and its stalls as they waited:
    /// as the run ends, `net.delay` logs them.
    ranged: Option<[u64; 2]>,
    pub(super) waits: u64,
    waited: Option<u64>,
    pub(super) stalls: Stalls,
    /// What `--link` has the link do to what comes, and the state of the random numbers it draws.
    pub(super) conditions: Option<Conditions>,
    random: u64,
    /// The next tick this peer runs, as its packets tell the other.
    next: u64,
    /// The first tick this peer hasn't made its inputs for, those for each tick before having gone
    /// out; and, while its delay falls, a tick's it holds back, which go out with the next tick's.
    pub(super) tagged: u64,
    held: Option<Inputs>,
    /// This peer's inputs, by tick, kept until the other peer has them and the tick has run on
    /// both peers'.
    ours: BTreeMap<u64, Inputs>,
    /// The other peer's, by tick, kept until the tick has run on them.
    pub(super) theirs: BTreeMap<u64, Inputs>,
    /// The tick from which the other peer's inputs are needed: before it, they're empty; `None`
    /// while there's no other peer, for the host, which plays on alone.
    pub(super) from: Option<u64>,
    /// The first tick from `from` whose inputs from the other peer haven't come whole, all before
    /// it having come; and the first of this peer's that the other hasn't said it has.
    pub(super) whole: u64,
    pub(super) acked: u64,
    /// The tick the peers' worlds were last made the same at, as the host's came, which their
    /// packets name, so that those sent before, which may come after, are dropped.
    since: u64,
    /// The ticks run on the other peer's inputs predicted, which haven't run on them since, by
    /// tick.
    pub(super) predicted: BTreeMap<u64, Predicted>,
    /// How many times this peer rolled back, how many ticks it ran again, and the most at once.
    pub(super) rolled: [u64; 3],
    /// What a packet is read into.
    buffer: Vec<u8>,
    /// What this peer offered since its last tick, with the ticket of each that's answered, which
    /// goes out with its inputs for the tick its delay on from its next.
    pub(super) offered: Vec<(Option<Ticket>, Outside)>,
    /// The last ticket given.
    #[cfg(feature = "dev")]
    tickets: Ticket,
    /// What the edits and patches this peer offered did, by ticket, with the tick they applied
    /// before.
    done: Vec<(Ticket, u64, Result<Done>)>,
    /// The world's hash after each tick, this peer's and the other's, until they're compared.
    pub(super) hashes: [BTreeMap<u64, WorldHash>; 2],
    /// A copy of this peer's world after each tick whose hashes haven't been compared, by tick,
    /// so that a desync names where the worlds parted: the joiner's after the tick they parted at
    /// kept until its host's comes. And copies kept from before, which they and the predicted
    /// ticks' are taken into again.
    pub(super) worlds: BTreeMap<u64, World>,
    pub(super) spares: Vec<World>,
    /// The joiner's: its host's world after the tick their hashes parted at, as it comes, the tick
    /// and its lines so far.
    parting: Option<(u64, String)>,
    #[cfg(feature = "dev")]
    pub(super) patches: Patches,
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
pub(super) struct Patches {
    /// Each patch, numbered from 1 by its place, with its ticket, until it applies.
    got: Vec<Option<(subsecond::JumpTable, Ticket)>>,
    /// How many have applied, from the first: the code the peer runs.
    applied: usize,
    /// Each patch that's to apply, by its number, with the tick it applies before: the host's
    /// choice, which its joiner takes, [`UNSENT`] until the host's inputs that carry it go out.
    pub(super) due: Vec<(usize, u64)>,
    /// The joiner's: how many its host had applied, as its host's world comes.
    hosts: usize,
}

/// The tick a patch the host was given applies before until the host's inputs that carry it go
/// out, which none reaches.
#[cfg(feature = "dev")]
pub(super) const UNSENT: u64 = u64::MAX;

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
    pub(super) fn protocol(&self) -> &'static str {
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
    pub(super) fn phase(&self) -> SessionPhase {
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
                None => self.bad_line(line),
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
                    _ => self.bad_line(line),
                }
            }
            "t" => match tick_of(rest).filter(|&tick| self.near(tick)) {
                Some(tick) => {
                    self.theirs.entry(tick).or_default().whole = true;
                    self.came_whole();
                }
                None => self.bad_line(line),
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
                    None => self.bad_line(line),
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
                    _ => self.bad_line(line),
                }
            }
            #[cfg(feature = "dev")]
            "n" if !self.hosts() => match text::read_named(rest, "n ".len() + 1) {
                Some(named) => self.names.push(named),
                None => self.bad_line(line),
            },
            _ => self.bad_line(line),
        }
        None
    }

    /// Loses the other peer for `line`, a line that isn't one.
    fn bad_line(&mut self, line: &str) {
        self.lose(&format!("the peer sent a line that isn't one: {line}"));
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
    pub(super) fn follow_link(&mut self, now: Instant) {
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
    pub(super) fn came_whole(&mut self) {
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
    pub(super) fn read_ticks(&mut self, packet: &[u8], now: Instant) -> Option<()> {
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
    pub(super) fn send(&mut self, text: String) {
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
