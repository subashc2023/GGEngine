//! A server that owns the world (`--serve`), as [Networking](../../../../ARCHITECTURE.md#networking)
//! describes, and serves it to clients, [`Client`](super::Client)s (`--connect`): it runs its ticks
//! in real time on its own layers' commands and on each client's inputs, which apply in the order
//! their ticks have them, each tick's at the first the server runs that's its tick or past it, once
//! those before have; and after every [`STATES`]th tick it sends each client the text of what it
//! sees of the world, the world but for the cells near no viewer of its own, as `World::seen_by`
//! says, with the hash of what it sees, as the client's state: what changed since the latest state
//! the client has said it has, the rest named as kept, compressed against that state's text, as
//! [`states`](super::states) says, which a thread of the server's own writes from a copy of the
//! world taken as the tick ends.
//!
//! A client's stream is lines as a lockstep session's are, as [the session's](super) say, but for
//! these:
//!
//! ```text
//! hello\tgg-server 5\tShards\t0.1.0 debug dev\t3f8a6c2e9b1d4057\tShards Inspector\t50123
//!                                                    the client, as it connects: its app, engine,
//!                                                      build and layers, and the port its packets
//!                                                      come from
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
//! [`MAX_PACKET_TEXT`], so that a packet lost loses none:
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
//! The server's, after every [`STATES`]th tick the states thread was idle at the end of, are the
//! client's state, in as many packets as it takes, each a line and then its part of the state's
//! bytes, its text as zstd compresses it, which the whole text of the state it's against is the
//! dictionary of:
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

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::mem;
#[cfg(test)]
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use gg_core::Result;
use tracing::{info, warn};

use super::states::{Base, States, Wanted, Written};
use super::{
    AHEAD, Accepted, Code, Link, MAX_LINE, MAX_PACKET_TEXT, Merged, Queued, Turn, engine, listen,
    port_of, read_batches, tick_of,
};
use crate::World;
use crate::args::{Conditions, Listen};
use crate::cells::{Cell, Seen};
use crate::layer::Commands;
use crate::recording::text;

/// The hello's first part, the protocol and its version, which a client's must be: a change to the
/// lines or the packets takes it to the next.
pub(super) const PROTOCOL: &str = "gg-server 5";

/// How many of the latest ticks the server keeps the world's text after, and a client its states,
/// half a second's, past which a client's state is sent whole.
pub(super) const KEPT: usize = 32;

/// The most bytes of a state's a packet carries, past its line.
pub(super) const PART: usize = MAX_PACKET_TEXT - 128;

/// How many ticks apart the server sends its clients' states: two, 32 a second, which takes a
/// client's download to about half of what a state each tick takes, as a state holds about as many
/// bytes whichever tick it's against; a client's corrections come up to a tick later, and the other
/// players draw further behind, as [`Client`](super::Client) says.
const STATES: u64 = 2;

/// A server: where its clients' connections come, and each client.
pub(crate) struct Server {
    accepted: Accepted,
    /// Where the server listens, as the OS has it, if it does: for tests, which listen where it
    /// picks.
    #[cfg(test)]
    pub(super) listening: Option<SocketAddr>,
    pub(super) app: &'static str,
    /// The run's build, as [`Build::name`](super::Build::name) names it, which a client's must be.
    build: String,
    /// The layers' names, from the bottom up, which a client's must be.
    layers: String,
    /// What `--link` has the links do to what comes, and the state of the random numbers it draws.
    pub(super) conditions: Option<Conditions>,
    pub(super) random: u64,
    clients: Vec<Remote>,
    /// How many clients have connected, which numbers each.
    numbered: u64,
    /// The server's own layers' commands for the tick about to run, each with its layer's index.
    ours: Vec<(usize, String)>,
    /// What writes the clients' states.
    states: States,
    /// What a packet is read into.
    buffer: Vec<u8>,
    /// How many times the server sent a client its world, how many states it sent, and their
    /// bytes; how many ticks ended without states, as the states thread was still writing the last
    /// tick's; how many tables the states sent wrote, and how many they kept; and how many ticks
    /// of the clients' inputs applied after their tick, as they came late, and the most ticks after.
    served: [u64; 8],
    /// Whether the world couldn't be written as a state, which is warned of once.
    unsaved: bool,
}

/// A client, as its server knows it.
struct Remote {
    /// Which client it is among those that connected, as the states thread names it.
    number: u64,
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
    /// The cells it saw as its last state was written, as [`World::seen_by`] has them; and each of
    /// its latest [`KEPT`] states whole, by tick, which its next is against, shared with the
    /// clients that saw the world alike against the same.
    cells: BTreeSet<Cell>,
    texts: BTreeMap<u64, Arc<Base>>,
}

impl Server {
    /// Starts taking clients of app `app` of build `build`, as
    /// [`Build::name`](super::Build::name) names it, whose layers are `layers`, from the bottom
    /// up, where `at` says, as [`listen`] takes them, given the session's `code`.
    ///
    /// Fails with `net.start` if it can't listen at its address or start the thread that writes
    /// the clients' states.
    pub(crate) fn start(
        at: &Listen,
        code: Code,
        app: &'static str,
        build: &str,
        layers: &[&str],
    ) -> Result<Self> {
        let states = States::start()?;
        let (accepted, at) = listen(at, "clients", code)?;
        match at {
            Some(at) => info!(address = %at, "Serving the world"),
            None => info!("Serving the world at its relay alone"),
        }
        Ok(Self {
            accepted,
            #[cfg(test)]
            listening: at,
            app,
            build: build.to_owned(),
            layers: layers.join(" "),
            conditions: None,
            random: 0,
            clients: Vec::new(),
            numbered: 0,
            ours: Vec::new(),
            states,
            buffer: vec![0; 1 << 16],
            served: [0; 8],
            unsaved: false,
        })
    }

    /// Between ticks, as a frame starts, the next tick to run being `next`: takes the connections
    /// made whose handshakes held, refusing those whose didn't, and the lines and packets each
    /// client's link has, as `--link` holds them; refuses a hello of another protocol, app,
    /// engine, build or layers, giving one taken the least player no other client has; and returns
    /// whether the server owes a client its world. Logs a client lost: its connection closed,
    /// nothing came from it for [`LOST`](super::LOST), or no hello within [`HELLO`](super::HELLO).
    /// First it sends the states the states thread has written since, if it has.
    pub(crate) fn turn(&mut self, next: u64) -> Turn {
        self.send_written(None);
        let now = Instant::now();
        while let Some(handshaken) = self.accepted.handshaken() {
            let shaken = match handshaken {
                Ok(shaken) => shaken,
                Err(why) => {
                    warn!(code = "net.refused", "Refused a client: {why}");
                    continue;
                }
            };
            let number = self.numbered;
            self.numbered += 1;
            match Link::new(shaken.stream, shaken.keys, true, shaken.relayed) {
                Ok(link) => self.clients.push(Remote {
                    number,
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
                    cells: BTreeSet::new(),
                    texts: BTreeMap::new(),
                }),
                Err(error) => warn!(code = "net.link", "Can't take a client: {error}"),
            }
        }
        let engine = engine();
        let ours = [PROTOCOL, self.app, &engine, &self.build, &self.layers];
        let Self {
            clients,
            buffer,
            conditions,
            random,
            ..
        } = self;
        clients.retain_mut(|client| {
            let (lines, packets, gone) = client.link.take(buffer, *conditions, random, now);
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
            if let (Ok(()), Some(why)) = (&lost, gone) {
                lost = Err(Some(why));
            }
            match lost {
                Ok(()) => return true,
                Err(Some(why)) => warn!(
                    code = "net.lost",
                    player = client.player,
                    "Lost a client: {why}"
                ),
                Err(None) => {}
            }
            client.ended();
            false
        });
        for at in 0..clients.len() {
            if clients[at].hello && clients[at].player.is_none() {
                let taken = |player: u8| clients.iter().any(|client| client.player == Some(player));
                match (0..=u8::MAX).find(|&player| !taken(player)) {
                    Some(player) => clients[at].player = Some(player),
                    None => {
                        let why = "the server has as many players as it can";
                        warn!(code = "net.refused", "Refused a client: {why}");
                        let _ = clients[at].link.send(format!("refused {why}\n"));
                        clients[at].owed = false;
                        clients[at].ended();
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
    /// what `write` gives of `world`, a snapshot file's text of what the client sees, as
    /// [`World::seen_by`] has it, with its player and where the server's packets come from, after
    /// which its inputs are needed from `next` on. A client whose world `write` doesn't give stays
    /// owed it.
    pub(crate) fn send_world(
        &mut self,
        world: &World,
        write: &mut dyn FnMut(Option<&Seen>) -> Option<String>,
        next: u64,
    ) {
        for client in &mut self.clients {
            let Some(player) = client.player.filter(|_| client.owed) else {
                continue;
            };
            let seen = world.seen_by(player, &BTreeSet::new());
            let Some(text) = write(Some(&seen)) else {
                continue;
            };
            client.owed = false;
            client.link.joined = false;
            client.inputs.clear();
            client.stream.clear();
            client.texts.clear();
            client.cells = seen.cells.cells_in;
            (client.next, client.newest, client.lead, client.have) = (next, None, 0, None);
            let port = client.link.port();
            match client
                .link
                .send(format!("world {next} {player} {port}\n{text}"))
            {
                Ok(()) => {
                    info!(player, tick = next, "Sent a client the world");
                    self.served[0] += 1;
                }
                // Lost at the next turn, which finds the connection closed, as the link's writing
                // thread, which has stopped, closed it.
                Err(error) => {
                    warn!(
                        code = "net.link",
                        player, "Can't send a client the world: {error}"
                    );
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
    /// came late at the first tick after theirs that has them, which are counted.
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
                if client.next < tick {
                    self.served[6] += 1;
                    self.served[7] = self.served[7].max(tick - client.next);
                }
                client.next += 1;
            }
        }
        merged
    }

    /// Once tick `tick` has run, leaving `world`: sends the states the states thread has written
    /// since, if it has; and then, at every [`STATES`]th tick, if it's idle, gives it a copy of the
    /// world, for it to write each
    /// joined client's state, as [`states`](super::states) says, against the latest state the
    /// client has said it has, if the server still keeps it, and whole otherwise, with the first
    /// tick of its inputs not applied and how early its latest came. If it isn't idle, the tick
    /// sends no states, and the next state each client gets is against its latest, as ever. Then
    /// it moves the world's change tick on, as a run that draws does as each tick starts, so that
    /// what changes after, between ticks too, is marked as changed since the tick.
    pub(crate) fn end_tick(&mut self, tick: u64, world: &mut World) {
        self.send_written(None);
        let joined = |client: &&Remote| client.link.joined && client.player.is_some();
        if tick.is_multiple_of(STATES) && self.clients.iter().any(|client| joined(&client)) {
            if self.states.idle() {
                let wanted = (self.clients.iter().filter(joined))
                    .map(|client| Wanted {
                        client: client.number,
                        player: client.player.expect("a joined client's"),
                        had: client.cells.clone(),
                        base: (client.have)
                            .and_then(|have| Some((have, Arc::clone(client.texts.get(&have)?)))),
                        next: client.next,
                        lead: client.lead,
                    })
                    .collect();
                self.states.give(world, tick, wanted);
            } else {
                self.served[3] += 1;
            }
        }
        world.advance();
    }

    /// Between frames, until `until`: sends the states the states thread is writing, as soon as
    /// it has, and then waits out the time left.
    pub(crate) fn wait(&mut self, until: Instant) {
        self.send_written(Some(until));
        std::thread::sleep(until.saturating_duration_since(Instant::now()));
    }

    /// Sends each client its state that the states thread has written, by `until`, waiting until
    /// then, or at once if it's `None`, if it has; and keeps it whole, for the client's next state
    /// to be against. Warns once, as the world can't be written as a state (`net.link` and the
    /// codes `World::save_seen` fails with).
    fn send_written(&mut self, until: Option<Instant>) {
        let Some((tick, written)) = self.states.take(until) else {
            return;
        };
        let written = match written {
            Ok(written) => written,
            Err(error) => {
                if !mem::replace(&mut self.unsaved, true) {
                    warn!(
                        code = error.code(),
                        "Can't send the clients the world, which they don't correct to: {error:#}"
                    );
                }
                return;
            }
        };
        let served = &mut self.served;
        for Written {
            client,
            base,
            next,
            lead,
            hash,
            cells,
            state,
        } in written
        {
            // Unless it was lost since. A client joined has had its world, which a client is sent
            // once, as its hello comes.
            let Some(client) = self
                .clients
                .iter_mut()
                .find(|remote| remote.number == client)
            else {
                continue;
            };
            client.cells = cells;
            let base = base.map_or_else(|| "-".to_owned(), |base| base.to_string());
            let (parts, size) = (state.bytes.len().div_ceil(PART), state.size);
            for (part, bytes) in state.bytes.chunks(PART).enumerate() {
                let mut line = String::new();
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
            served[2] += state.bytes.len() as u64;
            served[4] += state.tables[0];
            served[5] += state.tables[1];
            client.texts.insert(tick, Arc::clone(&state.whole));
            while client.texts.len() > KEPT {
                client.texts.pop_first();
            }
        }
    }

    /// Logs, as the run ends, what each client's link sent and took, as [`Link::ended`] says; and
    /// what the server sent its clients (`net.served`): how many times a client its world, and how
    /// many states, and their bytes; how many ticks sent none, as the states thread was still
    /// writing the last tick's; how many tables the states wrote, and how many they kept, as the
    /// client's base had them; and how many ticks of the clients' inputs came late, applying after
    /// their tick, and the most ticks after.
    pub(crate) fn ended(&self) {
        for client in &self.clients {
            client.ended();
        }
        let [
            worlds,
            states,
            bytes,
            skipped,
            written,
            kept,
            late,
            most_late,
        ] = self.served;
        let mean = bytes.checked_div(states).unwrap_or(0);
        info!(
            code = "net.served",
            worlds,
            states,
            bytes,
            mean,
            skipped,
            written,
            kept,
            late,
            most_late,
            "Sent clients the world {worlds} times, and {states} states, {mean} bytes each on \
             average; {skipped} ticks sent none, as the last tick's were still being written; the \
             states wrote {written} tables and kept {kept}; {late} ticks of the clients' inputs \
             came late, applying up to {most_late} ticks after theirs"
        );
    }
}

impl Remote {
    /// Logs what the client's link sent and took, as it ends, as [`Link::ended`] says.
    fn ended(&self) {
        let whose = match self.player {
            Some(player) => format!("player {player}'s client"),
            None => "a client".to_owned(),
        };
        self.link.ended(&whose);
    }

    /// Takes `line`, from the client, of the protocol, app, engine, build and layers `ours` are: its
    /// hello, that it has joined, or a line of a tick's inputs, the next tick to run being `next`.
    /// Fails with `None` for a hello refused, which it logs, and with why for any other line, and
    /// for a tick's lines past [`MAX_LINE`] bytes together, or inputs past those it may send, as
    /// [`AHEAD`] says.
    fn read(
        &mut self,
        line: &str,
        ours: [&str; 5],
        next: u64,
    ) -> std::result::Result<(), Option<String>> {
        let (kind, rest) = line.split_once([' ', '\t']).unwrap_or((line, ""));
        match kind {
            // The client's writing thread, saying it's there.
            "k" => Ok(()),
            "at" => self.link.at(rest).map_err(Some),
            "hello" if !self.hello => match port_of(line, ours, "the server") {
                Ok(port) => {
                    self.link.hello_came();
                    self.link.packets_from(port);
                    self.hello = true;
                    self.owed = true;
                    Ok(())
                }
                Err(why) => {
                    warn!(code = "net.refused", "Refused a client: {why}");
                    let _ = self.link.send(format!("refused {why}\n"));
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
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use gg_core::math::{DVec3, Vec3};
    use tracing_subscriber::layer::SubscriberExt;

    use super::super::client::{LATE, LEAD};
    use super::super::tests::{Command, Game, SLOW, Score};
    use super::super::{BUILD, Build, Client, Session, TESTED, secure};
    use super::*;
    use crate::Context;
    use crate::args::Net;
    use crate::dirs::tests::Scratch;
    use crate::layer::Stack;
    use crate::main_loop::Stepper;
    use crate::reflect::Reflect;
    use crate::testing::Codes;
    use crate::{
        AnimationParam, AnimationState, AnimationStates, BlendPoint, BodyType, BoxCollider3D,
        CapsuleCollider3D, CharacterController3D, Entity, Model, NavAgent, NavMeshVolume, Owner,
        ParamCondition, ParamTest, ParamValue, Parent, ParticleEmitter, Playback, RigidBody3D,
        SphereCollider3D, StateMotion, StateTransition, Tick, Transform, WorldHash, step_animation,
        step_navigation, step_physics,
    };

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
            let names = stack.names();
            let mut session = Session::start(net, Some(TESTED), "Tested", &Build::default(), names)
                .expect("started");
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
    /// client that sends inputs for ticks however far on, or the lines of a tick without end, and
    /// at a tick no run reaches.
    #[test]
    fn a_client_s_inputs_are_kept_to_a_second_and_a_line() {
        let _real_time = crate::testing::real_time();
        let [_other, (stream, keys)] = secure::linked();
        let mut link = Link::new(stream, keys, true, None).expect("a link");
        link.joined = true;
        let mut client = Remote {
            number: 0,
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
            cells: BTreeSet::new(),
            texts: BTreeMap::new(),
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
        let ours = [""; 5];
        assert!(client.read("t 11", ours, next).is_ok());
        assert!(client.read("t 74", ours, next).is_err());
        client.stream.clear();
        let half = format!("c 12 0 String(\"{}\")", "x".repeat(MAX_LINE / 2));
        assert!(client.read(&half, ours, next).is_ok());
        assert!(client.read(&half, ours, next).is_err());
    }

    /// The server counts a client's inputs that apply after their tick, as they came late, and the
    /// most ticks after; those that apply at their tick aren't late.
    #[test]
    fn a_server_counts_the_inputs_that_came_late() {
        let _real_time = crate::testing::real_time();
        let listen = crate::args::Listen::at("127.0.0.1:0");
        let mut server = Server::start(&listen, TESTED, "Tested", BUILD, &[]).expect("started");
        let [_other, (stream, keys)] = secure::linked();
        let mut link = Link::new(stream, keys, true, None).expect("a link");
        link.joined = true;
        let inputs = (10..14).map(|tick| (tick, Vec::new()));
        server.clients.push(Remote {
            number: 0,
            link,
            hello: true,
            player: Some(0),
            owed: false,
            inputs: inputs.collect(),
            next: 10,
            stream: String::new(),
            newest: None,
            lead: 0,
            have: None,
            cells: BTreeSet::new(),
            texts: BTreeMap::new(),
        });
        // Ticks 10 and 11's inputs at tick 12, 2 and 1 ticks late; 12's and 13's at theirs.
        for tick in 12..14 {
            server.inputs(tick);
        }
        assert_eq!(server.served[6..], [2, 2]);
        assert!(server.clients[0].inputs.is_empty());
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
        let _real_time = crate::testing::real_time();
        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let mut server = Node::new(
            &Net::Serve(crate::args::Listen::at("127.0.0.1:0")),
            Context::seeded(1),
        );
        // A server plays no player, as a run alone doesn't, but is networked, as its clients are.
        assert_eq!((server.cx.player(), server.cx.networked()), (None, true));
        let address = match server.stack.session_mut() {
            Some(Session::Server(server)) => server.listening.expect("listening").to_string(),
            _ => panic!("a server"),
        };
        let connect = |seed| {
            Node::new(
                &Net::Connect(crate::args::Toward::Address(address.clone())),
                Context::seeded(seed),
            )
        };
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

    /// A client whose server's world comes a second after the server wrote it, as a large world
    /// over a slow link does, starts behind, its world going on from the first state, its inputs
    /// still late by the way there and back, and catches up at once: within 2.5 s of joining
    /// begins a second in which none of its inputs come more than a few ticks late, as for a
    /// client catching up at its pace none would for some 3.5 s.
    #[test]
    fn a_client_that_starts_behind_catches_up_at_once() {
        let _real_time = crate::testing::real_time();
        let serve = Net::Serve(crate::args::Listen::at("127.0.0.1:0"));
        let mut server = Node::new(&serve, Context::seeded(1));
        let address = match server.stack.session_mut() {
            Some(Session::Server(server)) => server.listening.expect("listening").to_string(),
            _ => panic!("a server"),
        };
        let connect = Net::Connect(crate::args::Toward::Address(address));
        let mut clients = [Node::new(&connect, Context::seeded(2))];
        // 150 ms each way: some 20 ticks there and back.
        let link = Conditions {
            latency: Duration::from_millis(150),
            ..SLOW
        };
        match server.stack.session_mut() {
            Some(Session::Server(server)) => server.conditions = Some(link),
            _ => panic!("a server"),
        }
        clients[0].client().conditions = Some(link);
        fn served(server: &mut Node) -> &mut [u64; 8] {
            match server.stack.session_mut() {
                Some(Session::Server(server)) => &mut server.served,
                _ => panic!("a server"),
            }
        }
        until(&mut server, &mut clients, |server, _| {
            served(server)[0] == 1
        });
        // The world held a second before the client takes it.
        let held = Instant::now();
        while held.elapsed() < Duration::from_secs(1) {
            server.turn();
            thread::sleep(Duration::from_millis(2));
        }
        until(&mut server, &mut clients, |_, clients| clients[0].joined());
        let joined = Instant::now();
        // Jitter and lost packets have inputs 2 ticks late at most; a stall of the thread that
        // turns the server and client, as on a loaded runner, more, the second starting again after
        // it; caught up at its pace, more than `LATE` for some 3.5 s.
        let (mut last, mut late, mut most_late) = (joined, 0, 0);
        let within = Duration::from_millis(2500);
        until(&mut server, &mut clients, |server, _| {
            let served = served(server);
            if served[7] > LATE as u64 {
                (last, late, most_late) = (Instant::now(), late + served[6], served[7]);
                served[6..].fill(0);
            }
            last.elapsed() >= Duration::from_secs(1) || last - joined > within
        });
        let caught_up = last - joined;
        assert!(
            caught_up <= within,
            "{late} ticks' inputs late, the last of them up to {most_late} ticks, {caught_up:?} \
             after it joined"
        );
    }

    /// A server registered at a relay alone, and two clients given the relay and the code, meet
    /// there, as players 0 and 1, and play: each client's command applies on the server and on
    /// both clients, which come to the server's world, their packets through the relay until each
    /// link's go direct.
    #[test]
    fn a_server_and_its_clients_meet_through_a_relay() {
        let _real_time = crate::testing::real_time();
        let logged = crate::testing::logged();
        let relay = gg_relay::serve("127.0.0.1:0", gg_relay::Limits::default()).expect("a relay");
        let at = relay.address().to_string();
        let listen = crate::args::Listen {
            address: None,
            relay: Some(at.clone()),
        };
        let mut server = Node::new(&Net::Serve(listen), Context::seeded(1));
        let connect = |seed| {
            let toward = crate::args::Toward::Relay(at.clone());
            Node::new(&Net::Connect(toward), Context::seeded(seed))
        };
        let mut clients: Vec<Node> = (2..4).map(connect).collect();
        until(&mut server, &mut clients, |_, clients| {
            clients.iter().all(Node::joined)
        });
        let mut players: Vec<u8> = clients
            .iter()
            .filter_map(|client| client.cx.player())
            .collect();
        players.sort_unstable();
        assert_eq!(players, [0, 1]);
        for client in &mut clients {
            let player = client.cx.player().expect("a player");
            let points = if player == 0 { 1 } else { 10 };
            client.queue(Command::Add { player, points });
        }
        until(&mut server, &mut clients, |server, clients| {
            let hash = server.cx.world().hash();
            server.score() == [1, 10]
                && clients
                    .iter()
                    .all(|client| client.cx.world().hash() == hash)
        });
        assert!(relay.forwarded() > 0);
        // Each link's packets go direct, the server's two and each client's, as on the loopback
        // no router stands between them.
        let mut direct = 0;
        until(&mut server, &mut clients, |_, _| {
            direct += logged
                .codes
                .try_iter()
                .filter(|code| code == "net.direct")
                .count();
            direct >= 4
        });
        assert_eq!(direct, 4);
        relay.stop();
    }

    /// A client's command that names another client's player is refused, by the server and by the
    /// client as it predicts it, and its command for its own player applies: no client steers
    /// another's player.
    #[test]
    fn a_client_s_command_for_another_s_player_is_refused() {
        let _real_time = crate::testing::real_time();
        let logged = crate::testing::logged();
        let mut server = Node::new(
            &Net::Serve(crate::args::Listen::at("127.0.0.1:0")),
            Context::seeded(1),
        );
        let address = match server.stack.session_mut() {
            Some(Session::Server(server)) => server.listening.expect("listening").to_string(),
            _ => panic!("a server"),
        };
        let connect = |seed| {
            Node::new(
                &Net::Connect(crate::args::Toward::Address(address.clone())),
                Context::seeded(seed),
            )
        };
        let mut clients: Vec<Node> = (2..4).map(connect).collect();
        until(&mut server, &mut clients, |_, clients| {
            clients.iter().all(Node::joined)
        });
        let second = clients
            .iter()
            .position(|client| client.cx.player() == Some(1));
        let second = &mut clients[second.expect("player 1's")];
        second.queue(Command::Add {
            player: 0,
            points: 100,
        });
        second.queue(Command::Add {
            player: 1,
            points: 10,
        });
        until(&mut server, &mut clients, |server, clients| {
            let hash = server.cx.world().hash();
            server.score() == [0, 10]
                && clients
                    .iter()
                    .all(|client| client.cx.world().hash() == hash)
        });
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert!(
            codes.iter().any(|code| code == "net.bad_command"),
            "{codes:?}"
        );
        assert!(!codes.iter().any(|code| code == "net.lost"), "{codes:?}");
    }

    /// A client given another code than the server's is refused in its handshake, by the server
    /// and by itself, which tries again only after [`REFUSED`](super::super::REFUSED), never
    /// joining; a client given the server's joins.
    #[test]
    fn a_client_given_another_code_is_refused() {
        let _real_time = crate::testing::real_time();
        let logged = crate::testing::logged();
        let mut server = Node::new(
            &Net::Serve(crate::args::Listen::at("127.0.0.1:0")),
            Context::seeded(1),
        );
        let address = match server.stack.session_mut() {
            Some(Session::Server(server)) => server.listening.expect("listening").to_string(),
            _ => panic!("a server"),
        };
        // Of the server's app and layers, so that its code alone differs.
        let (other, layers) = ("0000-0000-0000-0000".parse(), server.stack.names().to_vec());
        let other = other.expect("a code");
        let toward = crate::args::Toward::Address(address.clone());
        let mut stranger = Client::start(&toward, other, "Tested", BUILD, &layers);
        let start = Instant::now();
        let mut refused = 0;
        while start.elapsed() < Duration::from_millis(600) {
            server.turn();
            assert!(matches!(stranger.turn(), Turn::None));
            let codes = logged.codes.try_iter();
            refused += codes.filter(|code| code == "net.refused").count();
            thread::sleep(Duration::from_millis(1));
        }
        // Each side's.
        assert_eq!(refused, 2);
        let mut clients = [Node::new(
            &Net::Connect(crate::args::Toward::Address(address)),
            Context::seeded(2),
        )];
        until(&mut server, &mut clients, |_, clients| clients[0].joined());
    }

    /// A command of [`Players`]': puts player `player`'s viewer at x `x`.
    #[derive(Clone, Copy, Debug, Reflect)]
    struct Move {
        player: u8,
        x: f32,
    }

    /// Builds a world of cells as it attaches, of the cells
    /// [`project`](crate::cells::tests::project) makes, 10 units on a side, with a viewer of a
    /// radius of 10 for each of players 0 and 1, at x 5 and 35, and moves each as its commands say,
    /// and cell 0's entity, if it's in, a unit up, so that cell 0 keeps it as it goes out.
    #[derive(Default)]
    struct Players(Vec<Move>);

    impl crate::Layer for Players {
        fn attach(&mut self, cx: &mut Context) {
            let world = cx.world_mut();
            world.spawn(crate::Cells {
                folder: "cells".to_owned(),
                size: 10.0,
            });
            for (player, x) in [(0, 5.0), (1, 35.0)] {
                let at = crate::Transform {
                    translation: (gg_core::math::Vec3::new(x, 0.0, 0.0)).as_dvec3(),
                    ..crate::Transform::IDENTITY
                };
                let viewer = crate::Viewer { radius: 10.0 };
                world.spawn((at, viewer, crate::Owner { player }));
            }
        }

        fn tick(&mut self, world: &mut World, _: crate::Tick) {
            for Move { player, x } in self.0.drain(..) {
                let mut owned = world.query::<(crate::Entity, &crate::Owner)>();
                let viewer = owned.find(|(_, owner)| owner.player == player);
                let Some(viewer) = viewer.map(|(viewer, _)| viewer) else {
                    continue;
                };
                let at = world.get_mut::<crate::Transform>(viewer).expect("placed");
                at.translation.x = f64::from(x);
                if let Some(zero) = world.entity(crate::cells::tests::id_of(0)) {
                    let at = world.get_mut::<crate::Transform>(zero).expect("placed");
                    at.translation.y += 1.0;
                }
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.0)
        }
    }

    /// A server brings cells in and takes them out around its clients' viewers, as their commands
    /// move them, and sends each client only what it sees: the world but for the cells near no
    /// viewer of its own, the cells it sees its cells' state, what the cells out keep staying on
    /// the server; each client comes to its view of the server's world, hashing as the server's
    /// view for it, though its own ticks bring no cell in; and a client joining late starts from
    /// its view, without a cell's entities or file while it has no viewer.
    #[test]
    fn a_server_s_clients_get_their_own_cells() {
        use crate::cells::CellState;
        use crate::cells::tests::{id_of, project, xs};
        use crate::dirs::tests::Scratch;

        let _real_time = crate::testing::real_time();
        let (codes, logged) = mpsc::channel();
        let subscriber = tracing_subscriber::registry().with(Codes(codes));
        let _logging = tracing::subscriber::set_default(subscriber);
        let dir = Scratch::new("server-cells");
        let root = project(&dir.0);
        let serve = Net::Serve(crate::args::Listen::at("127.0.0.1:0"));
        let mut server = Node::of(&serve, Context::seeded(1).root(&root), Players::default());
        let address = match server.stack.session_mut() {
            Some(Session::Server(server)) => server.listening.expect("listening").to_string(),
            _ => panic!("a server"),
        };
        let connect = Net::Connect(crate::args::Toward::Address(address));
        let node = |seed| {
            Node::of(
                &connect,
                Context::seeded(seed).root(&root),
                Players::default(),
            )
        };
        let mut clients = vec![node(2), node(3)];
        until(&mut server, &mut clients, |_, clients| {
            clients.iter().all(Node::joined)
        });
        clients.sort_by_key(|client| client.cx.player());
        assert_eq!(clients[1].cx.player(), Some(1));
        // Whether each client holds what it sees, each player's cells, hashing as the server's view
        // of the world for it.
        let viewing = |server: &Node, clients: &[Node], cells: &[&[i32]]| {
            let world = server.cx.world();
            clients.iter().zip(cells).all(|(client, &cells)| {
                let theirs = client.cx.world();
                let had = theirs.cells_in().collect();
                let player = client.cx.player().expect("a player");
                let seen = world.seen_by(player, &had);
                xs(theirs) == cells && theirs.hash() == world.hash_seen(Some(&seen))
            })
        };
        // Each step's move, the server's cells in, and the cells each of players 0 and 1 sees:
        // player 1's viewer, at x 35, keeps cell 1 in on the server, within its radius and half a
        // cell, but doesn't see it, as it didn't; and player 0's, at x 24, sees cell 0 still,
        // within its radius and half a cell, as it did.
        type Step = (Option<Move>, &'static [i32], [&'static [i32]; 2]);
        let walk: [Step; 4] = [
            (None, &[0, 1, 2, 3], [&[0, 1], &[2, 3]]),
            (
                Some(Move {
                    player: 0,
                    x: 100.0,
                }),
                &[1, 2, 3],
                [&[], &[2, 3]],
            ),
            (
                Some(Move { player: 0, x: 5.0 }),
                &[0, 1, 2, 3],
                [&[0, 1], &[2, 3]],
            ),
            (
                Some(Move { player: 0, x: 24.0 }),
                &[0, 1, 2, 3],
                [&[0, 1, 2, 3], &[2, 3]],
            ),
        ];
        let mut walk = walk.into_iter();
        let mut step = |server: &mut Node, clients: &mut [Node]| {
            let (moved, all, cells) = walk.next().expect("a step");
            if let Some(moved) = moved {
                let queue = clients[0].stack.commands_of(0).expect("a queue");
                queue.push(&moved.to_value()).expect("queued");
            }
            until(server, clients, |server, clients| {
                xs(server.cx.world()) == all && viewing(server, clients, &cells)
            });
            for client in &*clients {
                let world = client.cx.world();
                let state = world.resource::<CellState>().expect("streamed");
                assert!(state.kept.is_empty() && state.files.is_empty(), "{state:?}");
                for cell in 0..4 {
                    let shown = world.entity(id_of(cell)).is_some();
                    assert_eq!(shown, xs(world).contains(&cell), "cell {cell}'s entity");
                }
            }
        };
        step(&mut server, &mut clients[..]);
        step(&mut server, &mut clients[..]);
        let state = server.cx.world().resource::<CellState>().expect("streamed");
        assert_eq!(state.kept.len(), 1, "cell 0's, kept on the server");

        // A third client, without a viewer.
        clients.push(node(4));
        until(&mut server, &mut clients, |_, clients| clients[2].joined());
        let late = &clients[2];
        assert_eq!(late.cx.player(), Some(2));
        assert!((0..4).all(|cell| late.cx.world().entity(id_of(cell)).is_none()));
        let cell_file = |file: &crate::scene_files::SceneFile| file.author.starts_with("cells/");
        assert!(server.cx.scenes.cached.iter().any(cell_file));
        assert!(!late.cx.scenes.cached.iter().any(cell_file));
        let state = late.cx.world().resource::<CellState>();
        assert_eq!(state, Some(&CellState::default()));
        step(&mut server, &mut clients[..]);
        step(&mut server, &mut clients[..]);
        let codes: Vec<String> = logged.try_iter().collect();
        let parted = [
            "net.desync",
            "net.bad_command",
            "net.bad_state",
            "net.lost",
            "snapshot.hash",
        ];
        assert!(
            !codes.iter().any(|code| parted.contains(&code.as_str())),
            "{codes:?}"
        );
    }

    /// A resource that saves itself once its lock is free, waiting while the test holds it, but
    /// no more than 5 s.
    #[derive(Clone, Default)]
    struct Gate(Arc<std::sync::Mutex<()>>);

    impl crate::Saved for Gate {
        const NAME: &'static str = "Gate";

        fn save(&self, _: &World) -> Result<String> {
            let start = Instant::now();
            while self.0.try_lock().is_err() {
                if start.elapsed() > Duration::from_secs(5) {
                    return Err(gg_core::Error::new("test.gate", "held for 5 s"));
                }
                thread::sleep(Duration::from_millis(1));
            }
            Ok("open".to_owned())
        }

        fn load(_: &str, _: &World) -> Result<Self> {
            Ok(Self::default())
        }
    }

    /// A world of a [`Gate`], an entity that moves along each tick, and one that stays.
    struct Gated(Gate);

    impl crate::Layer for Gated {
        fn attach(&mut self, cx: &mut Context) {
            let world = cx.world_mut();
            world.register_saved::<Gate>().expect("registered");
            world.insert_resource(self.0.clone());
            world.spawn(crate::Transform::IDENTITY);
            world.spawn(crate::Owner { player: 7 });
        }

        fn tick(&mut self, world: &mut World, _: crate::Tick) {
            for at in world.query_mut::<&mut crate::Transform>() {
                at.translation.x += 1.0;
            }
        }
    }

    /// A server's clients' states are written on a thread of the server's own, from a copy of the
    /// world taken as a tick ends: while the thread waits, the server's ticks go on, sending no
    /// states, which it counts; and once it's done, the states go on, after every [`STATES`]th
    /// tick, each against the latest the client has, which reads it, without a desync or a bad
    /// state.
    #[test]
    fn states_are_written_on_a_thread_of_the_server_s_own() {
        let _real_time = crate::testing::real_time();
        let logged = crate::testing::logged();
        let gate = Gate::default();
        let serve = Net::Serve(crate::args::Listen::at("127.0.0.1:0"));
        let mut server = Node::of(&serve, Context::seeded(1), Gated(gate.clone()));
        let address = match server.stack.session_mut() {
            Some(Session::Server(server)) => server.listening.expect("listening").to_string(),
            _ => panic!("a server"),
        };
        let connect = Net::Connect(crate::args::Toward::Address(address));
        let mut clients = [Node::of(
            &connect,
            Context::seeded(2),
            Gated(Gate::default()),
        )];
        let served = |server: &mut Node| match server.stack.session_mut() {
            Some(Session::Server(server)) => server.served,
            _ => panic!("a server"),
        };
        let taken = |client: &mut Node| client.client().corrected[3];
        until(&mut server, &mut clients, |_, clients| {
            clients[0].joined() && taken(&mut clients[0]) > 0
        });
        let held = gate.0.lock().expect("unpoisoned");
        let (ticks, [_, sent, _, skipped, ..]) = (server.stepper.ticks, served(&mut server));
        until(&mut server, &mut clients, |server, _| {
            server.stepper.ticks >= ticks + 16
        });
        let [_, held_sent, _, held_skipped, ..] = served(&mut server);
        // The thread's tick's states, at most, once it was given them; each of the 8 or so ticks
        // of the 16 that would have sent states, every second, skipped.
        assert!(held_sent <= sent + 1, "sent {held_sent} for {sent}");
        assert!(
            held_skipped >= skipped + 7,
            "skipped {held_skipped} for {skipped}"
        );
        drop(held);
        let had = taken(&mut clients[0]);
        until(&mut server, &mut clients, |_, clients| {
            taken(&mut clients[0]) >= had + 16
        });
        let [_, _, _, _, written, kept, ..] = served(&mut server);
        assert!(
            kept > 0 && written > 0,
            "wrote {written} tables, kept {kept}"
        );
        // States after every second tick alone.
        let sent: Vec<u64> = match server.stack.session_mut() {
            Some(Session::Server(server)) => server.clients[0].texts.keys().copied().collect(),
            _ => panic!("a server"),
        };
        assert!(
            sent.len() >= 16 && sent.iter().all(|tick| tick.is_multiple_of(STATES)),
            "{sent:?}"
        );
        let codes: Vec<String> = logged.codes.try_iter().collect();
        let parted = ["net.desync", "net.bad_state", "net.lost", "test.gate"];
        assert!(
            !codes.iter().any(|code| parted.contains(&code.as_str())),
            "{codes:?}"
        );
    }

    /// A command of [`Arena`]'s: player `player`'s fox walks across at `x` and `z` units a second.
    #[derive(Reflect)]
    struct Walk {
        player: u8,
        x: f32,
        z: f32,
    }

    /// A 3D world as a networked game's: on a floor, each player's fox a character its player's
    /// [`Walk`]s walk, and a third steered over the navmesh toward a ball, which they push about;
    /// each fox's skinned model played by a state machine, standing or walking blended with running
    /// by how fast its character goes; and an emitter, burst every half second.
    struct Arena {
        root: PathBuf,
        fox: Option<Model>,
        walks: Vec<Walk>,
        /// The world's hash after each tick, as the tick's latest run left it.
        hashes: Hashes,
    }

    type Hashes = Rc<RefCell<BTreeMap<u64, WorldHash>>>;

    impl Arena {
        /// An arena of the Fox's files in `root`, as [`fox_files`] lays them out, keeping its
        /// ticks' hashes in `hashes`.
        fn new(root: &Path, hashes: Hashes) -> Self {
            let (root, fox, walks) = (root.to_owned(), None, Vec::new());
            Self {
                root,
                fox,
                walks,
                hashes,
            }
        }
    }

    impl crate::Layer for Arena {
        fn load(&mut self, cx: &mut Context) {
            let fox = cx.add_model("fox", self.root.join("Fox.gltf"));
            self.fox = Some(fox.expect("the Fox reads"));
        }

        fn attach(&mut self, cx: &mut Context) {
            let fox = self.fox.expect("loaded");
            let world = cx.world_mut();
            let floor = Transform {
                translation: DVec3::new(0.0, -0.5, 0.0),
                scale: Vec3::new(16.0, 1.0, 16.0),
                ..Transform::IDENTITY
            };
            let fixed = RigidBody3D {
                body_type: BodyType::Static,
                ..RigidBody3D::default()
            };
            world.spawn((floor, fixed, BoxCollider3D::default()));
            let volume = NavMeshVolume {
                size: Vec3::new(16.0, 4.0, 16.0),
                ..NavMeshVolume::default()
            };
            world.spawn((Transform::IDENTITY, volume));
            let ball = SphereCollider3D {
                radius: 0.4,
                ..SphereCollider3D::default()
            };
            world.spawn((at(0.0, 0.4, 0.0), RigidBody3D::default(), ball));
            for (i, x) in [0.0, 3.0, -3.0].into_iter().enumerate() {
                let capsule = CapsuleCollider3D {
                    height: 1.0,
                    radius: 0.35,
                    friction: 0.0,
                    ..CapsuleCollider3D::default()
                };
                let character = CharacterController3D::default();
                let body = world.spawn((at(x, 0.5, -3.0), capsule, character));
                let steered = if i < 2 {
                    world.insert(body, Owner { player: i as u8 })
                } else {
                    let agent = NavAgent {
                        speed: 3.0,
                        radius: 0.35,
                        ..NavAgent::default()
                    };
                    world.insert(body, agent)
                };
                // The model's units centimeters, its feet at the capsule's foot.
                let feet = Transform {
                    translation: DVec3::new(0.0, -0.5, 0.0),
                    scale: Vec3::splat(0.02),
                    ..Transform::IDENTITY
                };
                let model = fox.spawn(world, feet);
                let driven = steered.and_then(|()| world.insert(model, fox_states()));
                driven
                    .and_then(|()| world.set_parent(model, Some(body)))
                    .expect("just spawned");
            }
            let emitter = ParticleEmitter {
                rate: 0.0,
                burst: 8,
                ..ParticleEmitter::default()
            };
            world.spawn((Transform::IDENTITY, emitter));
        }

        fn tick(&mut self, world: &mut World, tick: Tick) {
            for walk in self.walks.drain(..) {
                let foxes = world.query_mut::<(&Owner, &mut CharacterController3D)>();
                for (owner, character) in foxes {
                    if owner.player == walk.player {
                        (character.velocity.x, character.velocity.z) = (walk.x, walk.z);
                    }
                }
            }
            if tick.number.is_multiple_of(15) {
                let ball = ball_at(world);
                for agent in world.query_mut::<&mut NavAgent>() {
                    agent.destination = Some(ball);
                }
            }
            if tick.number.is_multiple_of(30) {
                for emitter in world.query_mut::<&mut ParticleEmitter>() {
                    emitter.bursts += 1;
                }
            }
            step_navigation(world);
            let speeds: Vec<(Entity, f32)> = (world.query::<(Entity, &CharacterController3D)>())
                .map(|(body, character)| (body, character.velocity.with_y(0.0).length()))
                .collect();
            for (&Parent(body), machine) in world.query_mut::<(&Parent, &mut AnimationStates)>() {
                let speed = speeds.iter().find(|&&(fox, _)| fox == body);
                let speed = speed.map_or(0.0, |&(_, speed)| speed);
                machine.set_param("speed", speed).expect("a float");
            }
            step_animation(world);
            step_physics(world);
            self.hashes.borrow_mut().insert(tick.number, world.hash());
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.walks)
        }
    }

    /// A fox's state machine: Survey standing, and Walk blended with Run by its `speed`.
    fn fox_states() -> AnimationStates {
        let state = |name: &str, motion| AnimationState {
            name: name.to_owned(),
            motion,
            speed: 1.0,
            playback: Playback::Loop,
        };
        let point = |clip: &str, at| BlendPoint {
            clip: clip.to_owned(),
            at,
        };
        let moving = StateMotion::Blend {
            param: "speed".to_owned(),
            points: vec![point("Walk", 1.6), point("Run", 5.0)],
        };
        let transition = |from: &str, to: &str, test| StateTransition {
            from: Some(from.to_owned()),
            to: to.to_owned(),
            conditions: vec![ParamCondition {
                param: "speed".to_owned(),
                test,
            }],
            exit_time: None,
            fade: 0.25,
        };
        AnimationStates {
            params: vec![AnimationParam {
                name: "speed".to_owned(),
                value: ParamValue::Float(0.0),
            }],
            states: vec![
                state("Idle", StateMotion::Clip("Survey".to_owned())),
                state("Move", moving),
            ],
            transitions: vec![
                transition("Idle", "Move", ParamTest::Above(0.2)),
                transition("Move", "Idle", ParamTest::Below(0.2)),
            ],
            ..AnimationStates::default()
        }
    }

    /// The Fox's files, without its ID file, under a root of their own, for an [`Arena`].
    fn fox_files(name: &str) -> Scratch {
        let scratch = Scratch::new(name);
        fs::create_dir_all(&scratch.0).expect("a directory");
        let files = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models/fox");
        for file in ["Fox.gltf", "Fox.bin", "Texture.png"] {
            fs::copy(files.join(file), scratch.0.join(file)).expect("copied");
        }
        scratch
    }

    fn at(x: f64, y: f64, z: f64) -> Transform {
        Transform {
            translation: DVec3::new(x, y, z),
            ..Transform::IDENTITY
        }
    }

    fn ball_at(world: &World) -> Vec3 {
        let mut balls = world.query::<(&SphereCollider3D, &Transform)>();
        balls.next().expect("a ball").1.translation.as_vec3()
    }

    /// Queues `walk`, a command of the [`Arena`] `node` runs.
    fn walk(node: &mut Node, walk: Walk) {
        let queue = node.stack.commands_of(0).expect("the arena queues walks");
        queue.push(&walk.to_value()).expect("the arena takes walks");
    }

    /// An [`Arena`]'s ticks, each run again on a copy of the world as it started, as a SyncTest
    /// runs them, leave the copy as they leave the world, as rollback needs, while its foxes walk,
    /// push the ball and stand again, one steered over the navmesh, and its emitter bursts.
    #[test]
    fn a_3d_world_s_ticks_run_again_alike_as_a_sync_test() {
        let _alone = crate::testing::alone();
        let scratch = fox_files("sync-test-3d");
        let mut cx = Context::seeded(1).root(scratch.0.clone());
        let mut stack = Stack::default();
        stack.push_layer(Arena::new(&scratch.0, Hashes::default()));
        stack.sync_test();
        stack.attach(&mut cx);
        let (ball, agent) = (ball_at(cx.world()), Vec3::new(-3.0, 0.5, -3.0));
        // Player 0's fox walks into the ball, player 1's about, and both stand from tick 120.
        let walks = [(0, 0, 0.0, 3.0), (1, 1, -2.0, 1.0), (60, 1, 2.0, 1.0)];
        let stand = [(120, 0, 0.0, 0.0), (120, 1, 0.0, 0.0)];
        let mut moved = [false; 3];
        for number in 0..240 {
            let due = walks.iter().chain(&stand).filter(|walk| walk.0 == number);
            for &(_, player, x, z) in due {
                let queue = stack.commands_of(0).expect("the arena queues walks");
                queue
                    .push(&Walk { player, x, z }.to_value())
                    .expect("queued");
            }
            assert!(stack.tick(&mut cx, Tick { number }));
            assert!(!cx.exiting(), "tick {number}");
            let machines = cx.world().query::<&AnimationStates>();
            for (moved, machine) in moved.iter_mut().zip(machines) {
                *moved |= machine.state == "Move";
            }
        }
        assert_eq!(stack.sync_test_mut().map(|sync| sync.matched()), Some(240));
        let world = cx.world();
        assert!(ball_at(world).distance(ball) > 0.5, "the ball pushed");
        let mut agents = world.query::<(&NavAgent, &Transform)>();
        let (_, steered) = agents.next().expect("an agent");
        assert!(
            steered.translation.as_vec3().distance(agent) > 1.0,
            "the agent walked"
        );
        assert_eq!(moved, [true; 3], "each fox walked");
        let mut emitters = world.query::<&ParticleEmitter>();
        assert_eq!(emitters.next().expect("an emitter").bursts, 8);
        stack.detach(&mut cx);
        assert!(!stack.failed());
    }

    /// A server's [`Arena`] and two clients over a slow link: each client walks its fox, predicting
    /// the other's and correcting toward the server's states, which read into its world hashing as
    /// the server's did; and once both foxes stand, so that its predictions are right, each
    /// client's runs of two seconds' ticks hash as the server's did, its foxes still walking and the
    /// ball rolling, as a 3D world read from a state steps as the server's does, navmesh and all;
    /// though it catches up past some, as a debug build's client may, reading each state whole.
    #[test]
    fn a_server_s_clients_come_to_its_3d_world() {
        let _alone = crate::testing::alone();
        let logged = crate::testing::logged();
        let scratch = fox_files("server-3d");
        // The server's ticks' hashes, then each client's, by its seed.
        let hashes: [Hashes; 3] = Default::default();
        let node = |net: &Net, seed: u64| {
            let cx = Context::seeded(seed).root(scratch.0.clone());
            let kept = Rc::clone(&hashes[seed as usize - 1]);
            Node::of(net, cx, Arena::new(&scratch.0, kept))
        };
        let mut server = node(&Net::Serve(crate::args::Listen::at("127.0.0.1:0")), 1);
        let address = match server.stack.session_mut() {
            Some(Session::Server(server)) => server.listening.expect("listening").to_string(),
            _ => panic!("a server"),
        };
        let connect = Net::Connect(crate::args::Toward::Address(address));
        let mut clients = [node(&connect, 2), node(&connect, 3)];
        until(&mut server, &mut clients, |_, clients| {
            clients.iter().all(Node::joined)
        });
        let ball = ball_at(server.cx.world());
        // For a second and a half, player 0's fox walks into the ball, and player 1's turns about
        // every quarter second, which the other client can't predict.
        let start = Instant::now();
        let mut next = 0;
        until(&mut server, &mut clients, |_, clients| {
            let turn = start.elapsed().as_millis() / 250;
            if turn >= next {
                for client in clients.iter_mut() {
                    let player = client.cx.player().expect("a player");
                    let x = if turn.is_multiple_of(2) { 2.0 } else { -2.0 };
                    let (x, z) = if player == 0 { (0.0, 3.0) } else { (x, 1.0) };
                    walk(client, Walk { player, x, z });
                }
                next = turn + 1;
            }
            start.elapsed() >= Duration::from_millis(1500)
        });
        for client in &mut clients {
            let player = client.cx.player().expect("a player");
            walk(
                client,
                Walk {
                    player,
                    x: 0.0,
                    z: 0.0,
                },
            );
        }
        until(&mut server, &mut clients, |server, _| {
            let mut foxes = server
                .cx
                .world()
                .query::<(&Owner, &CharacterController3D)>();
            foxes.all(|(_, fox)| fox.velocity.x == 0.0 && fox.velocity.z == 0.0)
        });
        // From half a second after both stand on the server, past which each client knows it; at
        // least a quarter of them run, a client on a loaded runner catching up past many.
        let from = server.stepper.ticks + 30;
        let to = from + 120;
        until(&mut server, &mut clients, |server, clients| {
            let past = |node: &Node| node.stepper.ticks > to;
            past(server) && clients.iter().all(past)
        });
        let served = hashes[0].borrow();
        for (client, hashes) in clients.iter_mut().zip(&hashes[1..]) {
            let player = client.cx.player();
            let hashes = hashes.borrow();
            let ran: Vec<u64> = (from..to)
                .filter(|tick| hashes.contains_key(tick))
                .collect();
            assert!(
                ran.len() >= 30,
                "player {player:?}'s ran {} ticks",
                ran.len()
            );
            let parted: Vec<&u64> = ran.iter().filter(|&t| hashes[t] != served[t]).collect();
            assert!(
                parted.is_empty(),
                "player {player:?}'s parted at {parted:?}"
            );
            let [corrections, ..] = client.client().corrected;
            assert!(corrections > 0, "player {player:?}'s never corrected");
        }
        assert!(
            ball_at(server.cx.world()).distance(ball) > 0.5,
            "the ball pushed"
        );
        let codes: Vec<String> = logged.codes.try_iter().collect();
        let parted = ["net.desync", "net.bad_command", "net.bad_state", "net.lost"];
        assert!(
            !codes.iter().any(|code| parted.contains(&code.as_str())),
            "{codes:?}"
        );
    }
}
