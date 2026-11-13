//! A server that owns the world (`--serve`), as [Networking](../../../../ARCHITECTURE.md#networking)
//! describes, and serves it to clients, [`Client`](super::Client)s (`--connect`): it runs its ticks
//! in real time on its own layers' commands and on each client's inputs, which apply in the order
//! their ticks have them, each tick's at the first the server runs that's its tick or past it, once
//! those before have; and after each tick it sends each client the world's text, compressed
//! against the text it sent after the latest tick the client has said it has, with the world's
//! hash, as the client's state.
//!
//! A client's stream is lines as a lockstep session's are, as [the session's](super) say, but for
//! these:
//!
//! ```text
//! hello\tgg-server 1\tShards\t0.1.0 debug dev\tShards Inspector\t50123
//!                                                    the client, as it connects, and the port its
//!                                                      packets come from
//! refused <why>                                      the server, refusing it, before it closes
//! world 1234 1 50124                                 the server's world, at tick 1234, as a
//! gg-snapshot 2                                        snapshot file's text, with the client's
//! ...                                                  player and the port the server's packets
//! end                                                  come from
//! joined 1234                                        the client, once it has restored it
//! ```
//!
//! A client's packets, sent at each tick it runs and again a tick's time after the last, carry its
//! inputs for each tick from the first the server hasn't said it applied, as many as fit
//! [`MAX_PACKET`], so that a packet lost loses none:
//!
//! ```text
//! i 1238 1240                                        the latest tick the client has the state
//!                                                      after, `-` for none; and the first tick of
//!                                                      the inputs it holds, all those before it
//!                                                      applied, or none
//! c 1241 0 Variant("Steer", [...])                   its inputs for tick 1241, as a peer's are
//! t 1241
//! ```
//!
//! The server's, after each tick, are the client's state, in as many packets as it takes, each a
//! line and then its part of the state's bytes, the world's text as zstd compresses it, which the
//! text of the state it's against is the dictionary of:
//!
//! ```text
//! s 1240 1238 1241 2 58183b6352a09a44 fb695ce29daa8479 0 1 34183
//!     the world after tick 1240, against the client's state after tick 1238, `-` for none, its
//!     text whole; the first tick of the client's inputs not applied; how many ticks before the
//!     server ran theirs the client's latest inputs came; the world's hash; the part, of how many;
//!     and the bytes of its text
//! ```
//!
//! A server's ticks allocate, for their states.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::mem;
#[cfg(test)]
use std::net::SocketAddr;
use std::net::TcpStream;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use gg_core::Result;
use tracing::{info, warn};

use super::{
    AHEAD, Link, MAX_LINE, MAX_PACKET, Merged, Queued, Turn, engine, listen, port_of, read_batches,
    tick_of,
};
use crate::World;
use crate::args::Conditions;
use crate::layer::Commands;
use crate::recording::text;

/// The hello's first part, the protocol and its version, which a client's must be: a change to the
/// lines or the packets takes it to the next.
pub(super) const PROTOCOL: &str = "gg-server 1";

/// How many of the latest ticks the server keeps the world's text after, and a client its states,
/// half a second's, past which a client's state is sent whole.
pub(super) const KEPT: usize = 32;

/// The most bytes of a state's a packet carries, past its line.
pub(super) const PART: usize = MAX_PACKET - 128;

/// The level zstd compresses states at: its default, quick, as states go out each tick.
const LEVEL: i32 = 3;

/// A server: where its clients' connections come, and each client.
pub(crate) struct Server {
    accepted: Receiver<TcpStream>,
    /// Where the server listens, as the OS has it: for tests, which listen where it picks.
    #[cfg(test)]
    pub(super) listening: SocketAddr,
    pub(super) app: &'static str,
    /// The layers' names, from the bottom up, which a client's must be.
    layers: String,
    /// What `--link` has the links do to what comes, and the state of the random numbers it draws.
    pub(super) conditions: Option<Conditions>,
    pub(super) random: u64,
    clients: Vec<Remote>,
    /// The server's own layers' commands for the tick about to run, each with its layer's index.
    ours: Vec<(usize, String)>,
    /// The world's text after each of the latest [`KEPT`] ticks, by tick, which a client's state is
    /// compressed against; none while no client has joined.
    texts: BTreeMap<u64, String>,
    /// What a packet is read into.
    buffer: Vec<u8>,
    /// How many times the server sent a client its world, how many states it sent, and their bytes.
    served: [u64; 3],
    /// Whether the world couldn't be written as a state, which is warned of once.
    unsaved: bool,
}

/// A client, as its server knows it.
struct Remote {
    link: Link,
    /// Whether its hello has been taken, after which it has a player, unless the server has no
    /// player left, and the server owes it its world.
    hello: bool,
    player: Option<u8>,
    owed: bool,
    /// Its inputs come whole, by tick, from `next`, the first tick of its inputs not applied; and
    /// the lines so far of a tick's that come on the stream.
    inputs: BTreeMap<u64, Vec<(usize, String)>>,
    next: u64,
    stream: String,
    /// The latest tick of its inputs come, and how many ticks before the server ran it they came.
    newest: Option<u64>,
    lead: i64,
    /// The latest tick it has said it has the state after.
    have: Option<u64>,
}

impl Server {
    /// Starts listening at `address` for clients of app `app`, whose layers are `layers`, from the
    /// bottom up.
    ///
    /// Fails with `net.start` if it can't listen there.
    pub(crate) fn start(address: &str, app: &'static str, layers: &[&str]) -> Result<Self> {
        let (accepted, at) = listen(address, "clients")?;
        info!(address = %at, "Serving the world");
        Ok(Self {
            accepted,
            #[cfg(test)]
            listening: at,
            app,
            layers: layers.join(" "),
            conditions: None,
            random: 0,
            clients: Vec::new(),
            ours: Vec::new(),
            texts: BTreeMap::new(),
            buffer: vec![0; 1 << 16],
            served: [0; 3],
            unsaved: false,
        })
    }

    /// Between ticks, as a frame starts, the next tick to run being `next`: takes the connections
    /// made, and the lines and packets each client's link has, as `--link` holds them; refuses a
    /// hello of another protocol, app, engine or layers, giving one taken the least player no other
    /// client has; and returns whether the server owes a client its world. Logs a client lost.
    pub(crate) fn turn(&mut self, next: u64) -> Turn {
        let now = Instant::now();
        while let Ok(stream) = self.accepted.try_recv() {
            match Link::new(stream) {
                Ok(link) => self.clients.push(Remote {
                    link,
                    hello: false,
                    player: None,
                    owed: false,
                    inputs: BTreeMap::new(),
                    next,
                    stream: String::new(),
                    newest: None,
                    lead: 0,
                    have: None,
                }),
                Err(error) => warn!(code = "net.link", "Can't take a client: {error}"),
            }
        }
        let engine = engine();
        let ours = [PROTOCOL, self.app, &engine, &self.layers];
        let Self {
            clients,
            buffer,
            conditions,
            random,
            ..
        } = self;
        clients.retain_mut(|client| {
            let (lines, packets, closed) = client.link.take(buffer, *conditions, random, now);
            let mut lost = Ok(());
            for line in &lines {
                lost = lost.and_then(|()| client.read(line, ours, next));
            }
            for packet in &packets {
                if lost.is_ok() && client.link.joined && client.read_packet(packet, next).is_none()
                {
                    lost = Err(Some("the client sent a packet that isn't one".to_owned()));
                }
            }
            if lost.is_ok() && closed {
                lost = Err(Some("the connection closed".to_owned()));
            }
            match lost {
                Ok(()) => true,
                Err(Some(why)) => {
                    warn!(
                        code = "net.lost",
                        player = client.player,
                        "Lost a client: {why}"
                    );
                    false
                }
                Err(None) => false,
            }
        });
        for at in 0..clients.len() {
            if clients[at].hello && clients[at].player.is_none() {
                let taken = |player: u8| clients.iter().any(|client| client.player == Some(player));
                match (0..=u8::MAX).find(|&player| !taken(player)) {
                    Some(player) => clients[at].player = Some(player),
                    None => {
                        let why = "the server has as many players as it can";
                        warn!(code = "net.refused", "Refused a client: {why}");
                        let _ = clients[at].link.send(&format!("refused {why}\n"));
                        clients[at].owed = false;
                    }
                }
            }
        }
        clients.retain(|client| !client.hello || client.player.is_some());
        if clients.iter().any(|client| client.owed) {
            Turn::Send
        } else {
            Turn::None
        }
    }

    /// Between ticks, the next to run being `next`: sends each client the server owes its world
    /// `world`, a snapshot file's text, with its player and where the server's packets come from,
    /// after which its inputs are needed from `next` on.
    pub(crate) fn send_world(&mut self, world: &str, next: u64) {
        for client in &mut self.clients {
            let Some(player) = client.player.filter(|_| client.owed) else {
                continue;
            };
            client.owed = false;
            client.link.joined = false;
            client.inputs.clear();
            client.stream.clear();
            (client.next, client.newest, client.lead, client.have) = (next, None, 0, None);
            let port = client.link.port();
            match client
                .link
                .send(&format!("world {next} {player} {port}\n{world}"))
            {
                Ok(()) => {
                    info!(player, tick = next, "Sent a client the world");
                    self.served[0] += 1;
                }
                // Lost at the next turn, which finds the connection closed.
                Err(error) => {
                    warn!(
                        code = "net.link",
                        player, "Can't send a client the world: {error}"
                    );
                    let _ = client.link.to.shutdown(std::net::Shutdown::Both);
                }
            }
        }
    }

    /// As a tick starts: takes the queues' commands, each layer's by its index, written against
    /// `world`, as the server's own for the tick, emptying them.
    pub(crate) fn start_tick<'q>(
        &mut self,
        queues: impl Iterator<Item = (usize, &'q mut dyn Commands)>,
        world: &World,
    ) {
        for (layer, queue) in queues {
            for i in 0..queue.len() {
                let mut value = String::new();
                text::write(&mut value, &queue.value(i), world);
                self.ours.push((layer, value));
            }
            queue.clear();
        }
    }

    /// What tick `tick` runs on: the server's own commands, then each joined client's, by its
    /// player, those of its inputs come from the first not applied for each tick to `tick`, in
    /// turn, until one hasn't come, so that a client's inputs apply in their order, those that
    /// came late at the first tick after theirs that has them.
    pub(crate) fn inputs(&mut self, tick: u64) -> Merged {
        let mut merged = Merged {
            commands: Queued::of(mem::take(&mut self.ours), None, false).collect(),
            ..Merged::default()
        };
        let mut joined: Vec<&mut Remote> = (self.clients.iter_mut())
            .filter(|client| client.link.joined)
            .collect();
        joined.sort_by_key(|client| client.player);
        for client in joined {
            while client.next <= tick
                && let Some(commands) = client.inputs.remove(&client.next)
            {
                let player = client.player;
                merged.commands.extend(Queued::of(commands, player, true));
                client.next += 1;
            }
        }
        merged
    }

    /// Once tick `tick` has run, leaving `world`: sends each joined client its state, the world's
    /// text and hash, against the text after the latest tick the client has said it has, if the
    /// server still keeps it, and whole otherwise, with the first tick of its inputs not applied
    /// and how early its latest came; and keeps the text.
    pub(crate) fn end_tick(&mut self, tick: u64, world: &World) {
        if !self.clients.iter().any(|client| client.link.joined) {
            self.texts.clear();
            return;
        }
        let mut text = String::new();
        if let Err(error) = world.save_whole(&mut text) {
            if !mem::replace(&mut self.unsaved, true) {
                warn!(
                    code = error.code(),
                    "Can't send the clients the world, which they don't correct to: {error:#}"
                );
            }
            return;
        }
        let hash = world.hash();
        let Self {
            clients,
            texts,
            served,
            ..
        } = self;
        for client in clients.iter_mut().filter(|client| client.link.joined) {
            let base = client.have.and_then(|have| Some((have, texts.get(&have)?)));
            let compressed = match base {
                Some((_, base)) => zstd::bulk::Compressor::with_dictionary(LEVEL, base.as_bytes())
                    .and_then(|mut compressor| compressor.compress(text.as_bytes())),
                None => zstd::bulk::compress(text.as_bytes(), LEVEL),
            };
            let bytes = match compressed {
                Ok(bytes) => bytes,
                Err(error) => {
                    warn!(
                        code = "net.link",
                        "Can't compress a client's state: {error}"
                    );
                    continue;
                }
            };
            let base = base.map_or_else(|| "-".to_owned(), |(have, _)| have.to_string());
            let parts = bytes.len().div_ceil(PART);
            for (part, bytes) in bytes.chunks(PART).enumerate() {
                let mut line = String::new();
                let (next, lead, size) = (client.next, client.lead, text.len());
                writeln!(
                    line,
                    "s {tick} {base} {next} {lead} {hash} {part} {parts} {size}"
                )
                .expect("a string takes any text");
                let mut packet = line.into_bytes();
                packet.extend_from_slice(bytes);
                client.link.send_packet(&packet);
            }
            served[1] += 1;
            served[2] += bytes.len() as u64;
        }
        texts.insert(tick, text);
        while texts.len() > KEPT {
            texts.pop_first();
        }
    }

    /// Logs what the server sent its clients (`net.served`), as the run ends: how many times a
    /// client its world, and how many states, and their bytes.
    pub(crate) fn ended(&self) {
        let [worlds, states, bytes] = self.served;
        let mean = bytes.checked_div(states).unwrap_or(0);
        info!(
            code = "net.served",
            worlds,
            states,
            bytes,
            mean,
            "Sent clients the world {worlds} times, and {states} states, {mean} bytes each on \
             average"
        );
    }
}

impl Remote {
    /// Takes `line`, from the client, of the protocol, app, engine and layers `ours` are: its
    /// hello, that it has joined, or a line of a tick's inputs, the next tick to run being `next`.
    /// Fails with `None` for a hello refused, which it logs, and with why for any other line, and
    /// for a tick's lines past [`MAX_LINE`] bytes together, or inputs past those it may send, as
    /// [`AHEAD`] says.
    fn read(
        &mut self,
        line: &str,
        ours: [&str; 4],
        next: u64,
    ) -> std::result::Result<(), Option<String>> {
        let (kind, rest) = line.split_once([' ', '\t']).unwrap_or((line, ""));
        match kind {
            "hello" if !self.hello => match port_of(line, ours, "the server") {
                Ok(port) => {
                    self.link.packets_from(port);
                    self.hello = true;
                    self.owed = true;
                    Ok(())
                }
                Err(why) => {
                    warn!(code = "net.refused", "Refused a client: {why}");
                    let _ = self.link.send(&format!("refused {why}\n"));
                    Err(None)
                }
            },
            "joined" if self.hello => {
                self.link.joined = true;
                info!(player = self.player, tick = rest, "A client joined");
                Ok(())
            }
            // A tick's inputs too many bytes for a packet, which come on the stream instead.
            "c" | "t" if self.link.joined => {
                if self.stream.len() + line.len() >= MAX_LINE {
                    return Err(Some(format!(
                        "the client sent a tick's inputs of more than {MAX_LINE} bytes"
                    )));
                }
                self.stream.push_str(line);
                self.stream.push('\n');
                if kind == "t" {
                    let stream = mem::take(&mut self.stream);
                    let (inputs, applied) = (&mut self.inputs, self.next);
                    let mut near = true;
                    let read = read_batches(stream.lines(), |tick, commands| {
                        near &= tick < next + AHEAD;
                        if tick >= applied && near {
                            inputs.entry(tick).or_insert(commands);
                        }
                    });
                    if read.is_none() || !near {
                        return Err(Some(format!(
                            "the client sent inputs that aren't, or for a tick too far on: \
                             {stream}"
                        )));
                    }
                }
                Ok(())
            }
            _ => Err(Some(format!(
                "the client sent a line that isn't one: {line}"
            ))),
        }
    }

    /// Takes `packet`, from the client, once it has joined, the next tick to run being `next`: the
    /// latest state it has, and its inputs for each tick that haven't come. The ticks before the
    /// first it holds that the server hasn't applied had none, as before the tick it went on from
    /// once its world came. `None` for a packet that isn't one, or that holds inputs past those
    /// the client may send, as [`AHEAD`] says, those before taken.
    fn read_packet(&mut self, packet: &[u8], next: u64) -> Option<()> {
        let mut lines = std::str::from_utf8(packet).ok()?.lines();
        let (have, first) = lines.next()?.strip_prefix("i ")?.split_once(' ')?;
        let have = match have {
            "-" => None,
            have => Some(tick_of(have)?),
        };
        self.have = self.have.max(have);
        let first = tick_of(first).filter(|&first| first < next + AHEAD)?;
        if first > self.next {
            self.next = first;
            self.inputs = self.inputs.split_off(&first);
        }
        let Self {
            inputs,
            next: applied,
            newest,
            lead,
            ..
        } = self;
        let mut near = true;
        read_batches(lines, |tick, commands| {
            near &= tick < next + AHEAD;
            if tick < *applied || !near {
                return;
            }
            inputs.entry(tick).or_insert(commands);
            if newest.is_none_or(|newest| tick > newest) {
                *newest = Some(tick);
                *lead = tick as i64 - next as i64;
            }
        })?;
        near.then_some(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use tracing_subscriber::layer::SubscriberExt;

    use super::super::Session;
    use super::super::client::LEAD;
    use super::super::tests::{Command, Game, SLOW, Score};
    use super::*;
    use crate::Context;
    use crate::args::Net;
    use crate::layer::Stack;
    use crate::main_loop::Stepper;
    use crate::reflect::Reflect;
    use crate::testing::Codes;

    /// A server's or a client's run over a slow link, [`SLOW`]: its stack, of a [`Game`], its
    /// context, and what steps its ticks in real time, as its frames do.
    struct Node {
        stack: Stack,
        cx: Context,
        stepper: Stepper,
        last: Instant,
    }

    impl Node {
        fn new(net: &Net, cx: Context) -> Self {
            Self::of(net, cx, Game::default())
        }

        /// A node of `layer` alone, in place of a [`Game`].
        fn of(net: &Net, mut cx: Context, layer: impl crate::Layer + 'static) -> Self {
            let mut stack = Stack::default();
            stack.push_layer(layer);
            let mut session = Session::start(net, "Tested", stack.names()).expect("started");
            session.condition(SLOW);
            stack.session(session);
            stack.attach(&mut cx);
            let stepper = Stepper::default();
            let last = Instant::now();
            Self {
                stack,
                cx,
                stepper,
                last,
            }
        }

        /// A frame's turn, as a run's frame takes it: what the session sent, then the ticks the
        /// time since the last owes, a client's paced.
        fn turn(&mut self) {
            let now = Instant::now();
            let dt = now - mem::replace(&mut self.last, now);
            if let Some(ticks) = self.stack.net_turn(&mut self.cx, self.stepper.ticks) {
                self.stepper.jump_to(ticks);
            }
            let owed = self.stack.paced(dt);
            let Self {
                stack, cx, stepper, ..
            } = self;
            let tick = |tick: crate::Tick| !stack.net_waits(tick.number) && stack.tick(cx, tick);
            stepper.step(owed, tick, || now.elapsed());
        }

        /// Queues a command of the game's.
        fn queue(&mut self, command: Command) {
            let queue = self.stack.commands_of(0).expect("the game queues commands");
            let queued = queue.push(&command.to_value());
            queued.expect("the game takes its commands");
        }

        fn score(&self) -> [u32; 2] {
            let mut scores = self.cx.world().query::<&Score>();
            scores.next().expect("a score").points
        }

        /// Whether the client has the server's world and runs its ticks.
        fn joined(&self) -> bool {
            self.cx.player().is_some() && !self.stack.net_waits(self.stepper.ticks)
        }

        fn client(&mut self) -> &mut super::super::Client {
            match self.stack.session_mut() {
                Some(Session::Client(client)) => client.as_mut(),
                _ => panic!("a client"),
            }
        }
    }

    /// Turns the server and its clients until `done` holds, failing after 10 seconds.
    fn until(
        server: &mut Node,
        clients: &mut [Node],
        mut done: impl FnMut(&mut Node, &mut [Node]) -> bool,
    ) {
        let start = Instant::now();
        while !done(server, clients) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "they never got there"
            );
            server.turn();
            for client in &mut *clients {
                client.turn();
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// A client's inputs are kept within a second of ticks past the server's next, and a tick's
    /// lines on the stream within a line's bytes: the client is lost at inputs past them, as from a
    /// client that sends inputs for ticks however far on, or the lines of a tick without end, or a
    /// packet from elsewhere at the client's address, and at a tick no run reaches.
    #[test]
    fn a_client_s_inputs_are_kept_to_a_second_and_a_line() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listening");
        let at = listener.local_addr().expect("an address");
        let _other = TcpStream::connect(at).expect("connected");
        let (stream, _) = listener.accept().expect("accepted");
        let mut link = Link::new(stream).expect("a link");
        link.joined = true;
        let mut client = Remote {
            link,
            hello: true,
            player: Some(0),
            owed: false,
            inputs: BTreeMap::new(),
            next: 10,
            stream: String::new(),
            newest: None,
            lead: 0,
            have: None,
        };
        let next = 10;
        let packet = |text: &str| text.as_bytes().to_vec();
        assert!(
            client
                .read_packet(&packet("i - 10\nt 10\nt 73\n"), next)
                .is_some()
        );
        assert_eq!(client.inputs.keys().copied().collect::<Vec<_>>(), [10, 73]);
        for far in [
            "i - 10\nt 74\n",
            "i - 74\n",
            "i - 10\nt 9223372036854775808\n",
        ] {
            assert!(client.read_packet(&packet(far), next).is_none(), "{far}");
        }
        assert_eq!(client.inputs.len(), 2);
        let ours = [""; 4];
        assert!(client.read("t 11", ours, next).is_ok());
        assert!(client.read("t 74", ours, next).is_err());
        client.stream.clear();
        let half = format!("c 12 0 String(\"{}\")", "x".repeat(MAX_LINE / 2));
        assert!(client.read(&half, ours, next).is_ok());
        assert!(client.read(&half, ours, next).is_err());
    }

    /// A server and three clients over a slow link, which grows slower: each client starts from the
    /// server's world, given a player of its own, as many as connect; each runs its own commands at
    /// once, predicting its own player exactly, as two of them queue commands for theirs, as their
    /// keys would, though the server applies them late, as they come late; each command applies
    /// once on the server, though they come late, lost and out of order; each client's ticks go
    /// ahead as the link slows, so that its inputs come in time again; and each client, corrected
    /// toward the server's states as it predicted the others' players, comes to the server's world,
    /// hashing alike once they've all applied, without a desync, a bad command, a bad state or a
    /// client lost.
    #[test]
    fn clients_predict_their_own_player_and_correct_toward_the_server_s_world() {
        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let mut server = Node::new(&Net::Serve("127.0.0.1:0".to_owned()), Context::seeded(1));
        // A server plays no player, as a run alone doesn't, but is networked, as its clients are.
        assert_eq!((server.cx.player(), server.cx.networked()), (None, true));
        let address = match server.stack.session_mut() {
            Some(Session::Server(server)) => server.listening.to_string(),
            _ => panic!("a server"),
        };
        let connect = |seed| Node::new(&Net::Connect(address.clone()), Context::seeded(seed));
        let mut clients: Vec<Node> = (2..5).map(connect).collect();
        until(&mut server, &mut clients, |_, clients| {
            clients.iter().all(Node::joined)
        });
        let mut players: Vec<u8> = clients
            .iter()
            .filter_map(|client| client.cx.player())
            .collect();
        players.sort_unstable();
        assert_eq!(players, [0, 1, 2]);
        assert!(clients.iter().all(|client| client.cx.networked()));

        // Its own command runs at its next tick, before the server, a link's latency away, has it.
        let first = clients
            .iter()
            .position(|client| client.cx.player() == Some(0));
        let first = &mut clients[first.expect("player 0's")];
        first.queue(Command::Add {
            player: 0,
            points: 1,
        });
        let at = first.stepper.ticks;
        while first.stepper.ticks == at {
            first.turn();
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!((first.score(), server.score()), ([1, 0], [0, 0]));

        let mut queued = [1, 0];
        let start = Instant::now();
        let mut slowed = false;
        for turn in 0.. {
            if start.elapsed() >= Duration::from_secs(2) {
                break;
            }
            // A link that grows slower: the clients' inputs come late, which the server applies at
            // the first tick it runs after they come, until the clients' ticks have gone ahead.
            if !slowed && start.elapsed() >= Duration::from_secs(1) {
                slowed = true;
                if let Some(Session::Server(server)) = server.stack.session_mut() {
                    let latency = Duration::from_millis(80);
                    server.conditions = Some(Conditions { latency, ..SLOW });
                }
            }
            for client in &mut clients {
                match client.cx.player() {
                    Some(0) => {
                        client.queue(Command::Add {
                            player: 0,
                            points: 1,
                        });
                        queued[0] += 1;
                    }
                    Some(1) if turn % 3 == 0 => {
                        client.queue(Command::Add {
                            player: 1,
                            points: 10,
                        });
                        queued[1] += 10;
                    }
                    _ => {}
                }
                client.turn();
                // Its own player's points, once its ticks have taken all it queued, are all it
                // queued, corrected or not.
                let mine = client.cx.player().map(usize::from).filter(|&mine| mine < 2);
                let taken = client
                    .stack
                    .commands_of(0)
                    .is_some_and(|queue| queue.is_empty());
                if let Some(mine) = mine.filter(|_| taken) {
                    assert_eq!(client.score()[mine], queued[mine], "player {mine}'s");
                }
            }
            server.turn();
            thread::sleep(Duration::from_millis(2));
        }
        until(&mut server, &mut clients, |server, clients| {
            let hash = server.cx.world().hash();
            server.score() == queued
                && clients
                    .iter()
                    .all(|client| client.cx.world().hash() == hash)
        });
        for client in &mut clients {
            assert_eq!(client.score(), queued);
            let [corrections, ..] = client.client().corrected;
            assert!(corrections > 1, "corrected {corrections} times");
        }
        // Each client's ticks gone ahead again, as the link slowed, until its inputs come `LEAD`
        // ticks before their ticks run, as a client whose ticks went at real time's pace wouldn't,
        // theirs 2 or 3 ticks late; which takes longer where turns come late, as on a loaded runner.
        until(&mut server, &mut clients, |_, clients| {
            let near = |client: &mut Node| (client.client().lead - LEAD as f64).abs() < 1.0;
            clients.iter_mut().all(near)
        });
        let codes: Vec<String> = logged.try_iter().collect();
        let parted = ["net.desync", "net.bad_command", "net.bad_state", "net.lost"];
        assert!(
            !codes.iter().any(|code| parted.contains(&code.as_str())),
            "{codes:?}"
        );
    }

    /// A server brings cells in and takes them out around the viewers its clients' commands move,
    /// and its states carry them, and what they keep, to each client, which comes to the server's
    /// world, hashing alike, though its own ticks bring none in.
    #[test]
    fn a_server_s_clients_take_its_cells_with_its_states() {
        use crate::cells::CellState;
        use crate::cells::tests::{Walk, Walker, id_of, project, xs};
        use crate::dirs::tests::Scratch;

        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let dir = Scratch::new("server-cells");
        let root = project(&dir.0);
        let serve = Net::Serve("127.0.0.1:0".to_owned());
        let mut server = Node::of(&serve, Context::seeded(1).root(&root), Walker::default());
        let address = match server.stack.session_mut() {
            Some(Session::Server(server)) => server.listening.to_string(),
            _ => panic!("a server"),
        };
        let connect = Net::Connect(address);
        let client = Node::of(&connect, Context::seeded(2).root(&root), Walker::default());
        let mut clients = [client];
        until(&mut server, &mut clients, |_, clients| clients[0].joined());
        assert!(clients[0].cx.client);
        for (step, cells) in [
            (Walk::Viewer(25.0), vec![0, 1, 2, 3]),
            (Walk::Crate(12.0), vec![0, 1, 2, 3]),
            (Walk::Viewer(100.0), vec![]),
            (Walk::Viewer(5.0), vec![0, 1]),
        ] {
            let queue = clients[0].stack.commands_of(0).expect("a queue");
            queue.push(&step.to_value()).expect("queued");
            until(&mut server, &mut clients, |server, clients| {
                let (ours, theirs) = (server.cx.world(), clients[0].cx.world());
                xs(ours) == cells && ours.hash() == theirs.hash()
            });
            let state = |node: &Node| node.cx.world().resource::<CellState>().cloned();
            assert_eq!(state(&clients[0]), state(&server), "{step:?}");
        }
        let world = clients[0].cx.world();
        let entity = world.entity(id_of(1)).expect("in");
        let at = world.get::<crate::Transform>(entity).expect("placed");
        assert_eq!(at.translation.x, 12.0, "as the server left it");
        let codes: Vec<String> = logged.try_iter().collect();
        let parted = ["net.desync", "net.bad_command", "net.bad_state", "net.lost"];
        assert!(
            !codes.iter().any(|code| parted.contains(&code.as_str())),
            "{codes:?}"
        );
    }
}
