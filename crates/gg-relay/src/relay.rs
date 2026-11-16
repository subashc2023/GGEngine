//! The relay that [`serve`] runs.
//!
//! A corner cut: a thread per connection (a host's two, to read and to write), one for all
//! datagrams, and one closing refused connections. Its ceiling: some thousands of connections,
//! under [`MOST`](super::MOST) by default, and the datagrams one thread forwards a second. The
//! upgrade is polling the sockets from a few threads, as `mio` does.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
    UdpSocket,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use super::{
    ANSWER, BIND, DATAGRAM, ENDS, IDLE, KEEP, Limits, MAX_DATAGRAM, Rate, Room, SILENT, TAKE,
    Ticket, Token, VERSION, answer, read_line, write_line,
};

/// How often the datagrams' thread looks whether the relay is stopping.
const WAKE: Duration = Duration::from_millis(250);

/// Where [`listen`] picks a port for port 0, IANA's dynamic ports from 0xC000, and how many it
/// tries. Where Hyper-V excludes ranges for one protocol, about one pick in six fails, so all 32
/// fail about once in 10^25.
const DYNAMIC: u16 = 0xC000;
const PICKS: u32 = 32;

/// How many refused connections wait to close, past which they close at once, and how long each
/// is read, so that it closes without a reset, which could lose the refusal.
const CLOSING: usize = 64;
const LINGER: Duration = Duration::from_secs(1);

/// A relay [`serve`] started. It runs until [`stop`](Self::stop), not until dropped, as std's
/// blocking accept can't be interrupted otherwise.
pub struct Relay {
    address: SocketAddr,
    shared: Arc<Shared>,
    accepting: JoinHandle<()>,
    forwarding: JoinHandle<()>,
}

struct Shared {
    state: Mutex<State>,
    limits: Limits,
    /// [`ANSWER`] and [`TAKE`], but in the tests that wait them out.
    answer: Duration,
    take: Duration,
    forwarded: AtomicU64,
    stopping: AtomicBool,
}

/// The relay's rooms, joiners, pairs and connections: one lock over them, never held across I/O.
#[derive(Default)]
struct State {
    /// Each open connection's stream, cloned for [`Relay::stop`] to shut; its length is what the
    /// relay holds.
    open: HashMap<u64, TcpStream>,
    /// The connections each address holds, by [`key`], while it holds any.
    addresses: HashMap<IpAddr, usize>,
    /// The last number a connection or a pair took.
    next: u64,
    /// Whether it has refused a connection as full since it last took one, so that it logs once.
    full: bool,
    rooms: HashMap<Room, Hosted>,
    waiting: HashMap<Ticket, Waiting>,
    pairs: HashMap<u64, Pair>,
    /// The pair's end each token binds, and each bound socket's.
    tokens: HashMap<Token, (u64, usize)>,
    bound: HashMap<SocketAddr, (u64, usize)>,
}

impl State {
    fn number(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

/// A room's host; `come` takes joiners' tickets to its writing thread.
struct Hosted {
    connection: u64,
    come: Sender<Ticket>,
}

/// A joiner waiting for its host; `paired` brings it the pair's number and the host's connection.
struct Waiting {
    room: Room,
    joiner: TcpStream,
    paired: Sender<(u64, TcpStream)>,
}

/// A pair, its arrays by end, the joiner's first. `held`: how long its stream was held back.
/// `datagrams` went on, `dropped` went past a rate. `counted` and `weighed`: the buckets of
/// datagrams and their bytes. `limited`: whether it logged passing one. `ends`: threads running.
struct Pair {
    room: Room,
    tokens: [Token; 2],
    bound: [Option<SocketAddr>; 2],
    bytes: [u64; 2],
    held: [Duration; 2],
    datagrams: [u64; 2],
    dropped: [u64; 2],
    counted: [Bucket; 2],
    weighed: [Bucket; 2],
    limited: [bool; 2],
    ends: u8,
    made: Instant,
}

impl Pair {
    /// Counts a datagram of `size` bytes from end `side` at `now`: `Ok` if `limits` let it on, else
    /// the rate it's past.
    fn datagram(
        &mut self,
        side: usize,
        size: usize,
        limits: &Limits,
        now: Instant,
    ) -> Result<(), &'static str> {
        let size = size as u64;
        if !self.counted[side].holds(limits.datagrams, 1, now) {
            self.dropped[side] += 1;
            return Err("datagrams");
        }
        if !self.weighed[side].holds(limits.datagram_bytes, size, now) {
            self.dropped[side] += 1;
            return Err("datagram bytes");
        }
        self.counted[side].spend(1);
        self.weighed[side].spend(size);
        self.datagrams[side] += 1;
        Ok(())
    }
}

/// A token bucket a [`Rate`] fills. `tokens` falls below 0 while what passed owes them.
struct Bucket {
    tokens: f64,
    filled: Instant,
}

impl Bucket {
    /// A full bucket, as each end starts, so that what a session sends first, such as the world a
    /// joiner takes, passes at once.
    fn full(rate: Rate, now: Instant) -> Self {
        Self {
            tokens: rate.burst as f64,
            filled: now,
        }
    }

    /// Fills it as `rate` has by `now`, up to its burst.
    fn fill(&mut self, rate: Rate, now: Instant) {
        let secs = now.saturating_duration_since(self.filled).as_secs_f64();
        self.tokens = (self.tokens + secs * rate.per_second as f64).min(rate.burst as f64);
        self.filled = now;
    }

    fn holds(&mut self, rate: Rate, n: u64, now: Instant) -> bool {
        self.fill(rate, now);
        self.tokens >= n as f64
    }

    fn spend(&mut self, n: u64) {
        self.tokens -= n as f64;
    }

    /// Takes `n` tokens at `now`, filled by `rate`, owing what it lacks. Returns how long `rate`
    /// takes to repay that, which the taker waits before passing on what it took them for.
    fn owe(&mut self, rate: Rate, n: u64, now: Instant) -> Duration {
        self.fill(rate, now);
        self.spend(n);
        if self.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-self.tokens / rate.per_second as f64)
        }
    }
}

/// What a connection from `ip` counts as toward [`Limits::per_address`]: an IPv4 address, mapped
/// or not, or an IPv6 address's /64, as one host holds a whole /64.
pub(crate) fn key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(ip) => IpAddr::V4(ip),
            None => IpAddr::V6(Ipv6Addr::from_bits(ip.to_bits() & (u128::MAX << 64))),
        },
    }
}

/// A connection the relay holds until this drops, from `address`, by [`key`].
struct Open {
    shared: Arc<Shared>,
    number: u64,
    address: IpAddr,
}

impl Drop for Open {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.open.remove(&self.number);
        if let Entry::Occupied(mut held) = state.addresses.entry(self.address) {
            *held.get_mut() -= 1;
            if *held.get() == 0 {
                held.remove();
            }
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A thread that panicked holding it left the maps whole: each change is one call.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Starts a relay at `address`, TCP and UDP on one port (for port 0, any free for both), holding
/// connections and pairs to `limits`.
///
/// Fails with `InvalidInput` if a rate of `limits` is 0 a second, and if it can't bind both, at
/// `address` or any port it tries for port 0, or start its threads.
pub fn serve(address: &str, limits: Limits) -> io::Result<Relay> {
    serve_within(address, limits, ANSWER, TAKE)
}

/// [`serve`] with `answer` and `take` in place of [`ANSWER`] and [`TAKE`].
pub(crate) fn serve_within(
    address: &str,
    limits: Limits,
    answer: Duration,
    take: Duration,
) -> io::Result<Relay> {
    let rates = [limits.stream, limits.datagrams, limits.datagram_bytes];
    if rates.iter().any(|rate| rate.per_second == 0) {
        let why = format!("a rate of 0 a second, in {limits:?}");
        return Err(io::Error::new(ErrorKind::InvalidInput, why));
    }
    let (listener, udp) = listen(address)?;
    let address = listener.local_addr()?;
    udp.set_read_timeout(Some(WAKE))?;
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        limits,
        answer,
        take,
        forwarded: AtomicU64::new(0),
        stopping: AtomicBool::new(false),
    });
    let (close, closing) = mpsc::sync_channel(CLOSING);
    thread::Builder::new()
        .name("gg-relay-close".to_owned())
        .spawn(move || close_all(closing))?;
    let accepting = {
        let shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("gg-relay-accept".to_owned())
            .spawn(move || accept(listener, &shared, &close))?
    };
    let forwarding = {
        let shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("gg-relay-datagrams".to_owned())
            .spawn(move || forward(&udp, &shared))?
    };
    let Limits {
        most,
        per_address,
        stream,
        datagrams,
        datagram_bytes,
    } = limits;
    info!(
        code = "relay.start",
        address = %address,
        most,
        per_address,
        stream = stream.per_second,
        stream_burst = stream.burst,
        datagrams = datagrams.per_second,
        datagrams_burst = datagrams.burst,
        datagram_bytes = datagram_bytes.per_second,
        datagram_bytes_burst = datagram_bytes.burst,
        "Relaying at {address}"
    );
    Ok(Relay {
        address,
        shared,
        accepting,
        forwarding,
    })
}

impl Relay {
    /// Where it listens, as the OS has it.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn forwarded(&self) -> u64 {
        self.shared.forwarded.load(Ordering::Relaxed)
    }

    /// Counts of what it holds, for tests that check it keeps nothing of what has gone.
    #[cfg(test)]
    pub(crate) fn holds(&self) -> [usize; 7] {
        let state = self.shared.lock();
        [
            state.open.len(),
            state.addresses.len(),
            state.rooms.len(),
            state.waiting.len(),
            state.pairs.len(),
            state.tokens.len(),
            state.bound.len(),
        ]
    }

    /// Waits until [`stop`](Self::stop) stops it: for the `gg-relay` executable, which relays until
    /// the process ends.
    pub fn wait(self) {
        let _ = self.accepting.join();
        let _ = self.forwarding.join();
    }

    /// Shuts every connection and ends every thread, closing its port.
    pub fn stop(self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        // Wakes the thread that accepts, which then ends.
        let ip = match self.address.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        let _ = TcpStream::connect_timeout(&SocketAddr::new(ip, self.address.port()), ANSWER);
        for stream in self.shared.lock().open.values() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        self.wait();
    }
}

/// TCP's listener and UDP's socket at `address`, on one port. Port 0 can't be left to the OS:
/// Windows picks each protocol's port by a count of its own, and TCP's can be excluded for UDP, in
/// stretches of hundreds, failing every relay until the count passes. So it picks at random among
/// the dynamic ports until one is free for both, [`PICKS`] times at most, as `portpicker` does.
fn listen(address: &str) -> io::Result<(TcpListener, UdpSocket)> {
    let mut at: Vec<SocketAddr> = address.to_socket_addrs()?.collect();
    if at.first().is_none_or(|at| at.port() != 0) {
        return both(&at);
    }
    let mut picks = 1;
    loop {
        let mut bytes = [0; 2];
        getrandom::fill(&mut bytes)
            .map_err(|error| io::Error::other(format!("the OS gave no random numbers: {error}")))?;
        // The dynamic ports are 0xC000 to 0xFFFF: the low 14 bits pick one.
        let port = DYNAMIC | (u16::from_le_bytes(bytes) & 0x3FFF);
        for at in &mut at {
            at.set_port(port);
        }
        match both(&at) {
            Err(_) if picks < PICKS => picks += 1,
            done => return done,
        }
    }
}

/// TCP's listener at the first of `at` it can bind, and UDP's socket at the same address.
fn both(at: &[SocketAddr]) -> io::Result<(TcpListener, UdpSocket)> {
    let listener = TcpListener::bind(at)?;
    let udp = UdpSocket::bind(listener.local_addr()?)?;
    Ok((listener, udp))
}

/// Accepts connections until the relay stops, serving each on a thread of its own. Those past the
/// most, in all or from one address, are refused and sent to `close`.
fn accept(listener: TcpListener, shared: &Arc<Shared>, close: &SyncSender<TcpStream>) {
    let Limits {
        most, per_address, ..
    } = shared.limits;
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                warn!(code = "relay.accept", "Can't accept a connection: {error}");
                // As at running out of files, which would come again at once.
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        // Fails once its other end has gone again.
        let Ok(address) = stream.peer_addr().map(|at| key(at.ip())) else {
            continue;
        };
        let admitted = {
            let mut state = shared.lock();
            // Read under the lock: `stop` sets it before it shuts every open connection, so it
            // shuts any taken before.
            if shared.stopping.load(Ordering::SeqCst) {
                return;
            }
            let from_address = state.addresses.get(&address).copied().unwrap_or(0);
            if state.open.len() >= most {
                if !std::mem::replace(&mut state.full, true) {
                    warn!(
                        code = "relay.refused",
                        most,
                        "Refusing connections: the relay holds the most it holds, {most}, as \
                         those after are until one closes"
                    );
                }
                Err("full".to_owned())
            } else if from_address >= per_address {
                // Without the address, as the relay logs none.
                debug!(
                    code = "relay.refused",
                    per_address, "Refused a connection: its address holds the most it may"
                );
                Err(format!(
                    "its address holds {per_address} connections, the most the relay holds from one"
                ))
            } else {
                state.full = false;
                let number = state.number();
                match stream.try_clone() {
                    Ok(clone) => {
                        state.open.insert(number, clone);
                        *state.addresses.entry(address).or_default() += 1;
                        Ok(Open {
                            shared: Arc::clone(shared),
                            number,
                            address,
                        })
                    }
                    Err(_) => Err("full".to_owned()),
                }
            }
        };
        let open = match admitted {
            Ok(open) => open,
            Err(why) => {
                let _ = stream.write_all(format!("refused {why}\n").as_bytes());
                let _ = stream.shutdown(Shutdown::Write);
                // Closed at once if `close` is full.
                let _ = close.try_send(stream);
                continue;
            }
        };
        let spawned = thread::Builder::new()
            .name("gg-relay-connection".to_owned())
            .spawn(move || serve_connection(stream, open));
        if let Err(error) = spawned {
            warn!(
                code = "relay.accept",
                "Can't start a connection's thread: {error}"
            );
        }
    }
}

/// Closes each refused connection from `closing` once its other end has, or after [`LINGER`],
/// reading meanwhile, so that it closes without a reset, which could lose the refusal.
fn close_all(closing: Receiver<TcpStream>) {
    let mut buffer = [0; 1024];
    for mut stream in closing {
        let deadline = Instant::now() + LINGER;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
                break;
            }
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    }
}

/// Serves `stream` by its first line, due within [`ANSWER`]: a host's, a joiner's or a take's.
/// Closes it as it ends, `open` with it.
fn serve_connection(mut stream: TcpStream, open: Open) {
    let within = open.shared.answer;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(within));
    let line = match answer(&mut stream, within) {
        Ok(line) => line,
        Err(error) => {
            debug!(
                code = "relay.refused",
                "Closed a connection without a first line: {error}"
            );
            return;
        }
    };
    let Some(command) = line
        .strip_prefix(VERSION)
        .and_then(|rest| rest.strip_prefix(' '))
    else {
        if let Some(rest) = line.strip_prefix("gg-relay ") {
            let theirs = rest.split(' ').next().unwrap_or_default();
            refuse(
                &mut stream,
                &format!("it speaks gg-relay {theirs}, and the relay {VERSION}"),
            );
        } else {
            debug!(
                code = "relay.refused",
                "Closed a connection whose first line isn't one"
            );
        }
        return;
    };
    match command.split_once(' ') {
        Some(("host", room)) => match room.parse() {
            Ok(room) => host(stream, &open, room),
            Err(why) => refuse(&mut stream, &why),
        },
        Some(("join", room)) => match room.parse() {
            Ok(room) => join(stream, open, room),
            Err(why) => refuse(&mut stream, &why),
        },
        Some(("take", ticket)) => match Ticket::parse(ticket) {
            Some(ticket) => take(stream, open, &ticket),
            None => refuse(&mut stream, "a ticket is 64 hex digits"),
        },
        _ => refuse(&mut stream, "its first line isn't one"),
    }
}

/// Answers `stream` `refused <why>`, which it logs, then closes it once its other end has, or after
/// [`ANSWER`].
fn refuse(stream: &mut TcpStream, why: &str) {
    info!(code = "relay.refused", "Refused a connection: {why}");
    if write_line(stream, &format!("refused {why}")).is_ok() {
        let _ = stream.shutdown(Shutdown::Write);
        // Read to its end, so that closing sends no reset, which could lose the refusal.
        let _ = stream.set_read_timeout(Some(ANSWER));
        let _ = io::copy(&mut stream.take(64 << 10), &mut io::sink());
    }
}

/// Serves a host of `room`, holding it for the first host alone. A writing thread tells it of
/// joiners and says `k` each second. Drops it once silent for [`SILENT`], or at a line not `k`.
fn host(mut stream: TcpStream, open: &Open, room: Room) {
    let shared = &open.shared;
    let (come, tickets) = mpsc::channel();
    {
        let mut state = shared.lock();
        if state.rooms.contains_key(&room) {
            drop(state);
            refuse(&mut stream, "another host holds the room");
            return;
        }
        state.rooms.insert(
            room,
            Hosted {
                connection: open.number,
                come,
            },
        );
    }
    let writer = stream.try_clone().and_then(|mut writer| {
        write_line(&mut writer, "hosting")?;
        thread::Builder::new()
            .name("gg-relay-host".to_owned())
            .spawn(move || write_tickets(writer, &tickets))
    });
    if writer.is_ok() {
        debug!(room = room.short(), "A host registered");
        let mut partial = Vec::new();
        while let Ok(Some(line)) = read_line(&mut stream, &mut partial, Instant::now() + SILENT) {
            if line != "k" {
                break;
            }
        }
    }
    // Dropping its `come` ends the writing thread.
    let mut state = shared.lock();
    if state
        .rooms
        .get(&room)
        .is_some_and(|hosted| hosted.connection == open.number)
    {
        state.rooms.remove(&room);
    }
    drop(state);
    let _ = stream.shutdown(Shutdown::Both);
}

/// Writes each ticket from `tickets` to a host's `stream`, and `k` after [`KEEP`] without one,
/// until the host is gone or a write fails.
fn write_tickets(mut stream: TcpStream, tickets: &Receiver<Ticket>) {
    loop {
        let written = match tickets.recv_timeout(KEEP) {
            Ok(ticket) => write_line(&mut stream, &format!("come {ticket}")),
            Err(RecvTimeoutError::Timeout) => write_line(&mut stream, "k"),
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if written.is_err() {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    }
}

/// Serves a joiner of `room`: answers `none` if no host holds it, or tells the host and waits
/// [`TAKE`] for its take, refusing the joiner after, and copying what it sends once taken.
fn join(mut stream: TcpStream, open: Open, room: Room) {
    let ticket = match Ticket::random() {
        Ok(ticket) => ticket,
        Err(error) => {
            warn!(
                code = "relay.random",
                "Can't make a joiner's ticket: {error}"
            );
            return refuse(&mut stream, "the relay has no random numbers");
        }
    };
    let Ok(joiner) = stream.try_clone() else {
        return;
    };
    let (paired, pairing) = mpsc::channel();
    let told = {
        let mut state = open.shared.lock();
        let told = (state.rooms.get(&room)).is_some_and(|hosted| hosted.come.send(ticket).is_ok());
        if told {
            state.waiting.insert(
                ticket,
                Waiting {
                    room,
                    joiner,
                    paired,
                },
            );
        }
        told
    };
    if !told {
        let _ = write_line(&mut stream, "none");
        let _ = stream.shutdown(Shutdown::Write);
        return;
    }
    let pair = match pairing.recv_timeout(open.shared.take) {
        Ok(pair) => pair,
        Err(_) => {
            // The thread that takes it may have just now; whichever takes it from `waiting` has it.
            let gone = open.shared.lock().waiting.remove(&ticket).is_some();
            if gone {
                let secs = open.shared.take.as_secs_f64();
                return refuse(
                    &mut stream,
                    &format!("its host didn't take it within {secs} s"),
                );
            }
            match pairing.recv() {
                Ok(pair) => pair,
                Err(_) => return,
            }
        }
    };
    let (number, to) = pair;
    copy(stream, to, &open, number, 0);
}

/// Serves a host's take of the joiner `ticket` names: pairs them, tells each end its token, and
/// copies the host's bytes to the joiner, as the joiner's thread copies the other way.
fn take(mut stream: TcpStream, open: Open, ticket: &Ticket) {
    let tokens = match Token::random().and_then(|joiner| Ok([joiner, Token::random()?])) {
        Ok(tokens) => tokens,
        Err(error) => {
            warn!(code = "relay.random", "Can't make a pair's tokens: {error}");
            return refuse(&mut stream, "the relay has no random numbers");
        }
    };
    let Ok(host) = stream.try_clone() else { return };
    let (waiting, number) = {
        let mut state = open.shared.lock();
        let Some(waiting) = state.waiting.remove(ticket) else {
            drop(state);
            return refuse(&mut stream, "no joiner waits with its ticket");
        };
        let number = state.number();
        let Limits {
            datagrams,
            datagram_bytes,
            ..
        } = open.shared.limits;
        let now = Instant::now();
        state.pairs.insert(
            number,
            Pair {
                room: waiting.room,
                tokens,
                bound: [None; 2],
                bytes: [0; 2],
                held: [Duration::ZERO; 2],
                datagrams: [0; 2],
                dropped: [0; 2],
                counted: [(); 2].map(|()| Bucket::full(datagrams, now)),
                weighed: [(); 2].map(|()| Bucket::full(datagram_bytes, now)),
                limited: [false; 2],
                ends: 2,
                made: now,
            },
        );
        for (side, token) in tokens.into_iter().enumerate() {
            state.tokens.insert(token, (number, side));
        }
        (waiting, number)
    };
    let Waiting {
        room,
        mut joiner,
        paired,
    } = waiting;
    info!(
        code = "relay.paired",
        room = room.short(),
        pair = number,
        "Paired a host and a joiner"
    );
    // Both ends told before either's bytes go across.
    let told = write_line(&mut joiner, &format!("paired {}", tokens[0]))
        .and_then(|()| write_line(&mut stream, &format!("paired {}", tokens[1])));
    if told.is_err() {
        let _ = joiner.shutdown(Shutdown::Both);
        let _ = stream.shutdown(Shutdown::Both);
    }
    // The joiner's thread copies its way, and counts its end.
    if paired.send((number, host)).is_err() {
        end(&open.shared, number, 0, 0, Duration::ZERO);
    }
    copy(stream, joiner, &open, number, 1);
}

/// Copies `from`, end `side` of pair `number`, to the other end, `to`, then counts the end done. At
/// `from`'s end it shuts `to`'s sending side; past [`IDLE`] or a failed read or write, both. Past
/// the stream's rate it stops reading, so that TCP holds the sender back as its buffers fill.
fn copy(mut from: TcpStream, mut to: TcpStream, open: &Open, number: u64, side: usize) {
    let _ = from.set_read_timeout(Some(IDLE));
    let _ = to.set_write_timeout(Some(IDLE));
    let rate = open.shared.limits.stream;
    let mut bucket = Bucket::full(rate, Instant::now());
    let mut held = Duration::ZERO;
    let mut buffer = [0; 16 << 10];
    let mut bytes = 0;
    let ended = loop {
        match from.read(&mut buffer) {
            Ok(0) => {
                // As the end shut its sending side: the other reads all that came, then the end,
                // closing without a reset.
                let _ = to.shutdown(Shutdown::Write);
                break Ok(());
            }
            Ok(read) => {
                let wait = bucket.owe(rate, read as u64, Instant::now());
                if !wait.is_zero() {
                    if held.is_zero() {
                        info!(
                            code = "relay.limited",
                            pair = number,
                            end = ENDS[side],
                            limit = "stream",
                            "Holding back the {} of pair {number}'s stream, past its rate",
                            ENDS[side]
                        );
                    }
                    thread::sleep(wait);
                    held += wait;
                }
                if let Err(error) = to.write_all(&buffer[..read]) {
                    break Err(error);
                }
                bytes += read as u64;
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => break Err(error),
        }
    };
    if ended.is_err() {
        let _ = from.shutdown(Shutdown::Both);
        let _ = to.shutdown(Shutdown::Both);
    }
    end(&open.shared, number, side, bytes, held);
}

/// Counts end `side` of pair `number` done, having copied `bytes`, held back for `held`. Once both
/// are, the pair, its tokens and bound sockets go, and it logs.
fn end(shared: &Shared, number: u64, side: usize, bytes: u64, held: Duration) {
    let mut state = shared.lock();
    let Some(pair) = state.pairs.get_mut(&number) else {
        return;
    };
    pair.bytes[side] = bytes;
    pair.held[side] = held;
    pair.ends -= 1;
    if pair.ends > 0 {
        return;
    }
    let pair = state.pairs.remove(&number).expect("the pair, just found");
    for token in &pair.tokens {
        state.tokens.remove(token);
    }
    for at in pair.bound.iter().flatten() {
        if state.bound.get(at).is_some_and(|&(of, _)| of == number) {
            state.bound.remove(at);
        }
    }
    drop(state);
    info!(
        code = "relay.ended",
        room = pair.room.short(),
        pair = number,
        joiner_bytes = pair.bytes[0],
        host_bytes = pair.bytes[1],
        joiner_datagrams = pair.datagrams[0],
        host_datagrams = pair.datagrams[1],
        joiner_dropped = pair.dropped[0],
        host_dropped = pair.dropped[1],
        joiner_held_secs = pair.held[0].as_secs_f64(),
        host_held_secs = pair.held[1].as_secs_f64(),
        secs = pair.made.elapsed().as_secs_f64(),
        "A pair ended: the joiner sent {} bytes and {} datagrams, the host {} and {}",
        pair.bytes[0],
        pair.datagrams[0],
        pair.bytes[1],
        pair.datagrams[1],
    );
}

/// Takes datagrams until the relay stops. A bind binds its sender to its token's end, answered with
/// the sender's address; another from a bound socket goes to the other end's, within its rates.
fn forward(udp: &UdpSocket, shared: &Shared) {
    let mut buffer = [0; MAX_DATAGRAM + 1];
    while !shared.stopping.load(Ordering::SeqCst) {
        let (size, from) = match udp.recv_from(&mut buffer) {
            Ok(came) => came,
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            // Windows' answer to a datagram sent where nothing listens, as to an end gone.
            Err(error) if error.kind() == ErrorKind::ConnectionReset => continue,
            Err(error) => {
                warn!(code = "relay.datagrams", "Can't read a datagram: {error}");
                thread::sleep(WAKE);
                continue;
            }
        };
        if size > MAX_DATAGRAM {
            continue;
        }
        let datagram = &buffer[..size];
        if datagram.starts_with(DATAGRAM) {
            let token = (datagram.strip_prefix(BIND))
                .and_then(|token| Token::parse(std::str::from_utf8(token).ok()?));
            if let Some(token) = token
                && bind(shared, &token, from)
            {
                let _ = udp.send_to(format!("gg-relay bound {from}").as_bytes(), from);
            }
            continue;
        }
        let now = Instant::now();
        let (to, limited) = {
            let mut state = shared.lock();
            let State { bound, pairs, .. } = &mut *state;
            let at = bound
                .get(&from)
                .and_then(|&(number, side)| Some((number, side, pairs.get_mut(&number)?)));
            match at {
                None => (None, None),
                Some((number, side, pair)) => {
                    match pair.datagram(side, size, &shared.limits, now) {
                        Ok(()) => (pair.bound[1 - side], None),
                        Err(limit) => {
                            let first = !std::mem::replace(&mut pair.limited[side], true);
                            (None, first.then_some((number, side, limit)))
                        }
                    }
                }
            }
        };
        if let Some((number, side, limit)) = limited {
            info!(
                code = "relay.limited",
                pair = number,
                end = ENDS[side],
                limit,
                "Dropping the {}'s datagrams of pair {number} past its rate of {limit}",
                ENDS[side]
            );
        }
        if let Some(to) = to
            && udp.send_to(datagram, to).is_ok()
        {
            shared.forwarded.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Binds `from` to the pair's end `token` names, replacing any before; whether the token names one.
fn bind(shared: &Shared, token: &Token, from: SocketAddr) -> bool {
    let mut state = shared.lock();
    let Some(&(number, side)) = state.tokens.get(token) else {
        return false;
    };
    let Some(pair) = state.pairs.get_mut(&number) else {
        return false;
    };
    let before = pair.bound[side].replace(from);
    if let Some(before) = before.filter(|&before| before != from)
        && state.bound.get(&before) == Some(&(number, side))
    {
        state.bound.remove(&before);
    }
    state.bound.insert(from, (number, side));
    true
}
