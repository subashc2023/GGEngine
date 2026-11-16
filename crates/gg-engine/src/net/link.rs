//! A session's link to the other end: its stream's threads, its packets, and what it counts.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::{self, BufRead, BufReader, ErrorKind, Read as _};
use std::mem;
use std::net::{Shutdown, SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use super::handshake::Relayed;
use super::path::{self, Change, Path};
use super::secure::{self, Keys, Opened, Replays, Sealer};
#[cfg(test)]
use super::tests;
use super::{HELLO, LOST, MAX_LINE, MAX_PACKET};
#[cfg(doc)]
use super::{Lockstep, MAX_PACKET_TEXT};
use crate::Tick;
use crate::args::Conditions;

/// How long a link through a relay waits for the relay to answer its socket's bind before it binds
/// again; and, once answered, how long before it binds again anyway, as its router's mapping of
/// the socket may change, and as that keeps the mapping while the link sends nothing else.
const BIND: Duration = Duration::from_millis(250);
const BOUND: Duration = Duration::from_secs(1);

/// How long a link's writing thread goes with nothing to write before it writes `k`, a line that
/// says only that this end is there.
pub(super) const KEEP: Duration = Duration::from_secs(1);

/// A connection to the other peer: what its threads write to it and read from it, each a thread of
/// its own, so that a slow link never holds the main thread; and the socket its packets come and go
/// by.
pub(super) struct Link {
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
    pub(super) unhello: Option<Instant>,
    pub(super) silence: Duration,
    /// Set once the link counts: for the host, once the joiner has said it joined, before which
    /// what it sends was sent against a world it no longer has; for the joiner, once it has the
    /// host's world.
    pub(super) joined: bool,
    /// The socket, of the address the connection is from, at a port the OS picks; and where the
    /// other peer's is, once its hello or its world has said, which alone its packets are taken
    /// from: on a link through a relay, the relay's, from the start.
    pub(super) udp: UdpSocket,
    pub(super) other: Option<SocketAddr>,
    /// A link's through a relay: the relay and the token its socket binds there by; where the
    /// relay last said the bind came from, the socket as its router maps it, and where this end
    /// last told the other its socket is, on the stream (`at`); when this end last sent a bind;
    /// and what one is written into.
    pub(super) relayed: Option<Relayed>,
    pub(super) reflexive: Option<SocketAddr>,
    said: Option<SocketAddr>,
    bind_sent: Option<Instant>,
    bind: Vec<u8>,
    /// A link's through a relay: which way its packets go, as [`Path`] chooses it.
    path: Path,
    /// What the link sent and took, which its changes of way and its end log.
    pub(super) traffic: Traffic,
    /// The keys the link's handshake made, which its threads seal and open the stream's frames
    /// under, and the main thread its packets; the nonce of this end's next packet, from
    /// [`PACKETS`](secure::PACKETS); the nonces of those taken from the other end; what a packet's
    /// text goes into, after the byte that says whether this end hears the other direct, and what
    /// it's sealed into; and whether one from the other end's address was dropped, as forged or
    /// come before, which is warned of once a link.
    pub(super) keys: Arc<Keys>,
    pub(super) nonce: u64,
    replays: Replays,
    plain: Vec<u8>,
    pub(super) sealed: Vec<u8>,
    forged: bool,
    /// What came, each held until it's due, as `--link` says, or at once.
    lines: VecDeque<(Instant, String)>,
    packets: Vec<(Instant, Vec<u8>)>,
    /// When this peer last sent its packets, if it has.
    pub(super) sent: Option<Instant>,
    /// Whether a packet couldn't be sent, which is warned of once a link.
    unsent: bool,
    /// When the link was made, which this end's packets' clock counts from; the other end's clock
    /// as its latest packet gave it, and when that came, which this end's packets echo, so that
    /// the other times the way there and back by them; and the way there and back, as the other's
    /// packets' echoes of this end's clock time it.
    made: Instant,
    echo: Option<(u64, Instant)>,
    pub(super) trip: Trip,
    /// The shortest time the packets taken this turn timed the way at, which alone is taken: those
    /// that waited to be taken while this end's frames stopped timed the wait as well.
    fastest: Option<Duration>,
    /// A peer's: how many ticks this one was past the other's next as the other's latest packet
    /// came, which this one's packets tell the other; and how many it's ahead of the other, its
    /// own count less the other's, halved, as GGPO's time sync has it, so that the way's length
    /// cancels out, a slow link's or an uneven one's, smoothed, as [`Lockstep::pace`] slows it.
    pub(super) past: Option<i64>,
    pub(super) ahead: f64,
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
    pub(super) fn new(
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
    pub(super) fn send(&mut self, text: String) -> std::io::Result<()> {
        self.to
            .send(text)
            .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "the link no longer writes to it"))
    }

    /// Has the other peer's packets be taken from its socket's `port`, at the address the
    /// connection comes from; on a link through a relay, from where they come from there, as they
    /// already are.
    pub(super) fn packets_from(&mut self, port: u16) {
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
    pub(super) fn at(&mut self, rest: &str) -> std::result::Result<(), String> {
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
    pub(super) fn hello_came(&mut self) {
        self.unhello = None;
    }

    /// The port this peer's packets come from.
    pub(super) fn port(&self) -> u16 {
        self.udp.local_addr().map_or(0, |at| at.port())
    }

    /// This end's clock at `now`: the milliseconds since the link was made.
    fn clock(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.made).as_millis() as u64
    }

    /// Writes after `out` what a packet sent at `now` says of the times: this end's clock, and the
    /// other end's latest, with the milliseconds since it came, or `-` for each while none has.
    pub(super) fn write_times(&self, out: &mut String, now: Instant) {
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
    pub(super) fn timed(&mut self, clock: u64, echoed: Option<(u64, u64)>, now: Instant) {
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
    pub(super) fn took(&mut self) {
        if let Some(fastest) = self.fastest.take() {
            self.trip.sample(fastest);
        }
    }

    /// Takes the lines read and the packets come since the last take, once what `conditions` hold
    /// is due from `now`, as [`receive`](Self::receive) holds them, the writing thread's `k`s among
    /// the lines, and why the link is lost, if it is, once what came before has been taken: its
    /// connection closed, or a write to it failed; nothing came by it for [`Link::silence`]; or
    /// the other end's hello hasn't come within [`HELLO`] of a link accepted.
    pub(super) fn take(
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
    pub(super) fn send_packet(&mut self, packet: &[u8]) {
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
    pub(super) fn ended(&self, whose: &str) {
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
pub(super) const DIRECT: usize = 1;

/// What a link sent and took, which `net.traffic` logs as it ends: its stream's bytes, sealed in
/// their frames, as its threads write and read them, `k`s among them; its packets and their bytes,
/// sealed, each way, by what they went through, as the main thread sends them and takes them from
/// the other end, the binds and the relay's answers among those through the relay, and those
/// `--link` loses not taken; and the most bytes in a second of the link, each way, as the main
/// thread counts them as it takes what came, a frame's at a time, so that a second its frames
/// stopped for counts toward the next.
#[derive(Default)]
pub(super) struct Traffic {
    pub(super) written: Arc<AtomicU64>,
    pub(super) read: Arc<AtomicU64>,
    /// By [`RELAYED`] and [`DIRECT`].
    pub(super) sent: [Count; 2],
    pub(super) taken: [Count; 2],
    /// The second of the link counted toward, how many bytes each way had gone as it started,
    /// and the most in one of those before it, each way.
    second: u64,
    at_second: [u64; 2],
    most: [u64; 2],
}

/// Packets and their bytes.
#[derive(Clone, Copy, Default)]
pub(super) struct Count {
    pub(super) packets: u64,
    pub(super) bytes: u64,
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
pub(super) struct Trip {
    pub(super) mean: Option<f64>,
    pub(super) deviation: f64,
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
    pub(super) fn ticks(&self) -> Option<u64> {
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

/// Reads a line, without its line break, `\n` or `\r\n`: `None` once the stream ends, or ends
/// partway through a line, as a connection cut short does. As `gg_api::read_line` reads the tool
/// API's, which builds without `dev` haven't.
///
/// Fails with `InvalidData` for a line of more than [`MAX_LINE`] bytes, or one that isn't UTF-8.
pub(super) fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
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
