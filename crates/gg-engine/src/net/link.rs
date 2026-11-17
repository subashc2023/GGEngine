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

/// How long a link through a relay waits for the relay to answer its socket's bind before binding
/// again; once answered, how long before it binds anyway: the router's mapping may change, and
/// binding keeps it alive while the link sends nothing else.
const BIND: Duration = Duration::from_millis(250);
const BOUND: Duration = Duration::from_secs(1);

/// How long the writing thread goes with nothing to write before it writes `k`, a line that says
/// only that this end is there.
pub(super) const KEEP: Duration = Duration::from_secs(1);

/// A connection to the other peer: a thread each to write and read its stream, so a slow link never
/// holds the main thread, and the socket its packets come and go by.
pub(super) struct Link {
    /// What the writing thread writes; and why it stopped, if it did.
    to: Sender<String>,
    unwritten: Receiver<io::Error>,
    from: Receiver<String>,
    peer: Option<SocketAddr>,
    /// When a line or packet last came: past [`Link::silence`] the link is lost. `unhello` is, on
    /// an accepted link whose hello hasn't come, when it was made: past [`HELLO`] it's lost.
    heard: Instant,
    pub(super) unhello: Option<Instant>,
    pub(super) silence: Duration,
    /// Set once the link counts: for the host, once the joiner has said it joined (what it sends
    /// before was sent against a world it no longer has); for the joiner, once it has the host's
    /// world.
    pub(super) joined: bool,
    /// The socket, at the connection's local IP and a port the OS picks, and the other peer's,
    /// once its hello or world has said (the relay's from the start): the only address packets
    /// are taken from.
    pub(super) udp: UdpSocket,
    pub(super) other: Option<SocketAddr>,
    /// Through a relay: the relay and the socket's bind token; where the relay last said the bind
    /// came from (the socket as its router maps it); where this end last told the other its socket
    /// is (`at`, on the stream); when it last sent a bind; the bind's buffer.
    pub(super) relayed: Option<Relayed>,
    pub(super) reflexive: Option<SocketAddr>,
    said: Option<SocketAddr>,
    bind_sent: Option<Instant>,
    bind: Vec<u8>,
    /// Through a relay: which way packets go ([`Path`]).
    path: Path,
    pub(super) traffic: Traffic,
    /// The handshake's keys, for the threads' stream frames and the main thread's packets; the
    /// nonce of this end's next packet, from [`PACKETS`](secure::PACKETS); the nonces taken; a
    /// packet's text after its byte saying whether this end hears the other direct, and its
    /// sealed form; whether a packet from the other end's address was dropped as forged or
    /// replayed, warned once a link.
    pub(super) keys: Arc<Keys>,
    pub(super) nonce: u64,
    replays: Replays,
    plain: Vec<u8>,
    pub(super) sealed: Vec<u8>,
    forged: bool,
    /// What came, each held until due (`--link`).
    lines: VecDeque<(Instant, String)>,
    packets: Vec<(Instant, Vec<u8>)>,
    /// When this peer last sent its packets.
    pub(super) sent: Option<Instant>,
    /// Whether a packet couldn't be sent, warned once a link.
    unsent: bool,
    /// When the link was made, the epoch of this end's packet clock; the other end's clock from
    /// its latest packet, and when that came, which this end's packets echo so the other times the
    /// round trip; and the round trip, timed by the echoes of this end's clock.
    made: Instant,
    echo: Option<(u64, Instant)>,
    pub(super) trip: Trip,
    /// The shortest time the packets taken this turn timed: packets that waited while this end's
    /// frames stopped timed the wait too.
    fastest: Option<Duration>,
    /// A peer's: how many ticks this one was past the other's next as the other's latest packet
    /// came, which its packets tell the other; and how far ahead of the other it is: its count
    /// less the other's, halved as in GGPO's time sync so the way's length cancels, smoothed. See
    /// [`Lockstep::pace`].
    pub(super) past: Option<i64>,
    pub(super) ahead: f64,
}

impl Link {
    /// Starts a thread reading `stream`'s lines, in frames the handshake's `keys` open. It ends at
    /// an invalid line (such as one past [`MAX_LINE`]), a frame that doesn't open, the
    /// connection's end, and twice [`LOST`] with nothing come. Starts [`write_from`] on another;
    /// binds the socket, which the main thread reads each frame without waiting. An `accepted`
    /// link is lost unless the other end's hello comes within [`HELLO`]
    /// ([`hello_came`](Self::hello_came)). A `relayed` link sends its packets through the relay.
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
        // Past the link's silence, so the link is lost for that first; and so a dropped link's
        // reading thread, which reads until the other end closes, ends though it never does.
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

    /// Sends `text`, whole lines, for the writing thread.
    ///
    /// Fails once that thread has stopped.
    pub(super) fn send(&mut self, text: String) -> std::io::Result<()> {
        self.to
            .send(text)
            .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "the link no longer writes to it"))
    }

    /// Takes the other peer's packets from `port` at the connection's address. Through a relay,
    /// they're already taken from the relay's.
    pub(super) fn packets_from(&mut self, port: u16) {
        if self.relayed.is_none() {
            self.other = self.peer.map(|at| SocketAddr::new(at.ip(), port));
        }
    }

    /// Through a relay: binds the socket there ([`gg_relay::bind`]) every [`BIND`] until the relay
    /// answers, then every [`BOUND`].
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

    /// Through a relay: tells the other end where the socket is, `at <reflexive> <local>` (as the
    /// relay saw it, and as bound), whenever the relay's answer changes, for direct packets
    /// ([`Path`]).
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

    /// Takes the other end's `at` line's words, `rest`: where its socket is, which a relayed link
    /// then probes ([`Path::at`]).
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

    /// Logs a change of way ([`Path::step`]) with the packets sent and taken each way and the
    /// round trip if timed.
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

    pub(super) fn hello_came(&mut self) {
        self.unhello = None;
    }

    pub(super) fn port(&self) -> u16 {
        self.udp.local_addr().map_or(0, |at| at.port())
    }

    /// This end's clock, in milliseconds.
    fn clock(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.made).as_millis() as u64
    }

    /// Writes the times a packet sent at `now` carries: this end's clock, the other end's latest
    /// and the milliseconds since it came, or `-` for both while none has.
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

    /// Takes the times ([`write_times`](Self::write_times)) of a packet that came at `now`: its
    /// `clock`, which this end's packets echo unless a later one has come; and `echoed`, if it
    /// echoes this end's: that clock and the milliseconds the other held it. The time since, less
    /// the hold, is the round trip ([`took`](Self::took)).
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

    /// After a turn's packets: samples the round trip at the shortest time they timed.
    pub(super) fn took(&mut self) {
        if let Some(fastest) = self.fastest.take() {
            self.trip.sample(fastest);
        }
    }

    /// Takes the lines (`k`s among them) and packets come since the last take, once due under
    /// `conditions` ([`receive`](Self::receive)). Also why the link is lost, if it is, once what
    /// came before is taken:
    /// - its connection closed, or a write failed;
    /// - nothing came for [`Link::silence`];
    /// - an accepted link's hello hasn't come within [`HELLO`].
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

    /// Takes the packets from the other peer's socket, read through `buffer`: each's text, once it
    /// opens and its nonce is fresh ([`Replays`]), held or lost under `conditions`, drawing on
    /// `random`. Through a relay, also the relay's answers to binds and the other's packets that
    /// came direct, from wherever: only the other end seals them so, and those that don't open are
    /// dropped unsaid, as anyone can send the socket anything. The [`Path`] takes each.
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
                // Windows' answer to a packet sent to a port nothing listens at.
                Err(error) if error.kind() == ErrorKind::ConnectionReset => {}
                Err(error) => {
                    warn!(code = "net.link", "Can't read the peer's packets: {error}");
                    break;
                }
            }
        }
    }

    /// Seals `packet`, its text of at most [`MAX_PACKET_TEXT`] bytes after a byte saying whether
    /// this end hears the other direct, and sends it to the other peer's socket. One that can't be
    /// sent is lost, as a packet may be, and warned of once a link. Through a relay it goes the
    /// ways [`Path::step`] gives, each copy under its own nonce, a change of way logged.
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
            // A probe that doesn't reach the other goes unsaid.
            self.send_sealed(to, DIRECT, !ways.relay);
        }
    }

    /// Seals what [`send_packet`](Self::send_packet) wrote under the next nonce and sends it to
    /// `to`, counted as gone `way`. A failure warns once a link if `warned`.
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

    /// Logs, as the link `whose` ends, what it sent and took (`net.traffic`): each way the bytes,
    /// their mean a second and the most in one; the stream's and packets' bytes, the packets'
    /// count and how many went through the relay; the smoothed round trip and its deviation, if
    /// timed. IP and UDP headers aren't counted: some 28 bytes a packet over IPv4.
    pub(super) fn ended(&self, whose: &str) {
        let seconds = self.made.elapsed().as_secs_f64();
        let traffic = &self.traffic;
        // Each counter read once: the threads count on meanwhile.
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

/// Starts the thread that writes to `stream` what's sent through the returned channel, each text
/// whole and in order, sealed under `keys` ([`Sealer`]), adding the bytes to `written`. It writes
/// `k` after [`KEEP`] with nothing else, so the other end hears from this one while the main
/// thread is busy (restoring a world, stopped at a breakpoint). Also returns where the error that
/// stopped it comes.
///
/// Once the channel closes (the link dropped) and what was sent is written, it shuts the
/// connection's sending side: the other end reads what was sent, such as a refusal's why, then the
/// end, and closes its side, which ends the thread reading this one. Shutting the receiving side
/// too would have Windows reset the connection at what came unread, such as a joiner's hello the
/// host refuses, and the other end lose what was sent. At a failed write, such as one that can't go
/// on for [`LOST`], it shuts both sides.
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

/// A stream that adds the bytes read or written, sealed, to `bytes`.
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

/// Indices into [`Traffic`]'s counts: through the relay, or direct (every packet of a link not
/// through a relay).
const RELAYED: usize = 0;
pub(super) const DIRECT: usize = 1;

/// What a link sent and took, which `net.traffic` logs at its end:
/// - the stream's bytes, sealed in frames, `k`s among them, counted by its threads;
/// - the packets and their bytes, sealed, each way, by path, counted by the main thread: binds and
///   the relay's answers count as relayed, packets `--link` loses aren't taken;
/// - the most bytes in a second of the link, each way, counted a frame at a time as the main
///   thread takes what came, so a second its frames stopped for counts toward the next.
#[derive(Default)]
pub(super) struct Traffic {
    pub(super) written: Arc<AtomicU64>,
    pub(super) read: Arc<AtomicU64>,
    /// By [`RELAYED`] and [`DIRECT`].
    pub(super) sent: [Count; 2],
    pub(super) taken: [Count; 2],
    /// The second counted toward, the bytes each way when it started, and the most in one before
    /// it, each way.
    second: u64,
    at_second: [u64; 2],
    most: [u64; 2],
}

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
    fn bytes(&self) -> [u64; 2] {
        let packets = |[relayed, direct]: [Count; 2]| relayed.bytes + direct.bytes;
        [
            self.written.load(Ordering::Relaxed) + packets(self.sent),
            self.read.load(Ordering::Relaxed) + packets(self.taken),
        ]
    }

    /// At a new link `second`: closes the one counted toward and starts the new.
    fn count(&mut self, second: u64) {
        if second != self.second {
            let bytes = self.bytes();
            self.most = self.busiest(bytes);
            self.at_second = bytes;
            self.second = second;
        }
    }

    /// The most bytes in a second of the link, each way, the current one included, `bytes` having
    /// gone ([`bytes`](Self::bytes)).
    fn busiest(&self, bytes: [u64; 2]) -> [u64; 2] {
        [0, 1].map(|way| self.most[way].max(bytes[way] - self.at_second[way]))
    }
}

/// A link's round trip, smoothed as TCP does (RFC 6298's SRTT and RTTVAR): mean and mean
/// deviation, in seconds; none before the first sample.
#[derive(Default)]
pub(super) struct Trip {
    pub(super) mean: Option<f64>,
    pub(super) deviation: f64,
}

impl Trip {
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

    /// The ticks one way takes: half the round trip and [`MARGIN`] deviations, rounded up. What's
    /// sent comes within them, all but seldom. None before the first sample.
    pub(super) fn ticks(&self) -> Option<u64> {
        let way = self.mean? / 2.0 + MARGIN * self.deviation;
        Some((way / Tick::DT.as_secs_f64()).ceil() as u64)
    }

    /// Mean and mean deviation in milliseconds, to a tenth, for the logs.
    fn millis(&self) -> Option<[f64; 2]> {
        let millis = |seconds: f64| (seconds * 10_000.0).round() / 10.0;
        Some([millis(self.mean?), millis(self.deviation)])
    }
}

/// How many deviations past one way a link's delay allows: two, where TCP allows four for its
/// retransmissions. The deviation is the round trip's, some 1.4 times one way's where each way's
/// jitter is its own, so two allow nearly three of one way's. Inputs that come late cost a stall
/// (`net.delay` counts them); four would have the delay most of the round trip.
const MARGIN: f64 = 2.0;

/// Reads a line without its `\n` or `\r\n`: `None` once the stream ends, even partway through a
/// line. Like `gg_api::read_line`, which builds without `dev` lack.
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

/// The next of `random`'s SplitMix64 numbers, from 0 up to 1 in steps of 2⁻⁵³: for `--link`'s
/// conditions, which nothing replays.
fn unit(random: &mut u64) -> f64 {
    *random = random.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *random;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}
