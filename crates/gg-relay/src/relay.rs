//! The relay, as [`serve`] runs it: TCP and UDP on one port, the rooms its hosts hold, the joiners
//! waiting for their hosts, and the pairs it copies the bytes and forwards the datagrams of.
//!
//! A corner cut: a thread for each connection, a host's two, one reading and one writing, a waiting
//! joiner's one, and each end of a pair's one, which copies what it sends to the other end; one for
//! every datagram; and one more that closes the connections it refuses for being full. Its ceiling is
//! some thousands of connections, which [`MOST`](super::MOST) keeps it under by default, and the
//! datagrams one thread forwards a second. The upgrade is polling the sockets from a few threads, as
//! `mio` does.

use std::collections::HashMap;
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
    ANSWER, BIND, DATAGRAM, IDLE, KEEP, MAX_DATAGRAM, Room, SILENT, TAKE, Ticket, Token, VERSION,
    answer, read_line, write_line,
};

/// How long the thread of datagrams waits for one before it looks whether the relay is stopping.
const WAKE: Duration = Duration::from_millis(250);

/// Where [`listen`] picks a port for an address of port 0, IANA's dynamic ports, 0xC000 and up; and
/// how many it tries: where Hyper-V excludes ranges for one protocol, about one pick in six fails,
/// so all 32 fail about once in 10^25.
const DYNAMIC: u16 = 0xC000;
const PICKS: u32 = 32;

/// How many refused connections wait to be closed, past which they're closed at once; and how long
/// each is read for, so that it closes without a reset, which could lose the refusal.
const CLOSING: usize = 64;
const LINGER: Duration = Duration::from_secs(1);

/// A relay that [`serve`] started, which runs until [`stop`](Self::stop): dropping it alone doesn't
/// stop it, as std's blocking accept can't be interrupted otherwise.
pub struct Relay {
    address: SocketAddr,
    shared: Arc<Shared>,
    accepting: JoinHandle<()>,
    forwarding: JoinHandle<()>,
}

/// What a relay's threads share.
struct Shared {
    state: Mutex<State>,
    /// The most connections it holds at once.
    most: usize,
    /// The datagrams it has forwarded.
    forwarded: AtomicU64,
    stopping: AtomicBool,
}

/// The relay's rooms, joiners, pairs and connections: one lock over them, never held across I/O.
#[derive(Default)]
struct State {
    /// Each connection open, by its number, with a clone of its stream, which [`Relay::stop`]
    /// shuts; their count is the connections the relay holds.
    open: HashMap<u64, TcpStream>,
    /// The next number a connection or a pair takes.
    next: u64,
    /// Whether the relay has refused a connection for being full since it last took one, which it
    /// logs once.
    full: bool,
    rooms: HashMap<Room, Hosted>,
    waiting: HashMap<Ticket, Waiting>,
    pairs: HashMap<u64, Pair>,
    /// The end of the pair each token binds; and the end each bound socket's address is of.
    tokens: HashMap<Token, (u64, usize)>,
    bound: HashMap<SocketAddr, (u64, usize)>,
}

impl State {
    fn number(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

/// A room's host: its connection's number, and where the joiners' tickets go to the thread that
/// writes to it.
struct Hosted {
    connection: u64,
    come: Sender<Ticket>,
}

/// A joiner waiting for its host to take it: its room, a clone of its stream, and where the thread
/// that pairs it sends its own thread the pair's number and the host's new connection.
struct Waiting {
    room: Room,
    joiner: TcpStream,
    paired: Sender<(u64, TcpStream)>,
}

/// A pair: its room; each end's socket, as bound, the joiner's first; the bytes and datagrams each
/// end has sent; how many of its two connections' threads haven't ended; and when it was made.
struct Pair {
    room: Room,
    tokens: [Token; 2],
    bound: [Option<SocketAddr>; 2],
    bytes: [u64; 2],
    datagrams: [u64; 2],
    ends: u8,
    made: Instant,
}

/// A connection the relay holds, which it no longer does once this is dropped.
struct Open {
    shared: Arc<Shared>,
    number: u64,
}

impl Drop for Open {
    fn drop(&mut self) {
        self.shared.lock().open.remove(&self.number);
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A thread that panicked holding it left the maps whole: each change is one call.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Starts a relay at `address`, TCP and UDP on one port, any free for both given port 0, as tests
/// give, holding at most `most` connections at once, as the crate's root says.
///
/// Fails if it can't bind both at `address`, or given port 0 at any of the ports it tries, or start
/// its threads.
pub fn serve(address: &str, most: usize) -> io::Result<Relay> {
    let (listener, udp) = listen(address)?;
    let address = listener.local_addr()?;
    udp.set_read_timeout(Some(WAKE))?;
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        most,
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
    info!(code = "relay.start", address = %address, most, "Relaying at {address}");
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

    /// How many datagrams it has forwarded.
    pub fn forwarded(&self) -> u64 {
        self.shared.forwarded.load(Ordering::Relaxed)
    }

    /// How many of each it holds: connections, rooms, joiners waiting, pairs, tokens and bound
    /// sockets; for tests, which see that it keeps nothing of what has gone.
    #[cfg(test)]
    pub(crate) fn holds(&self) -> [usize; 6] {
        let state = self.shared.lock();
        [
            state.open.len(),
            state.rooms.len(),
            state.waiting.len(),
            state.pairs.len(),
            state.tokens.len(),
            state.bound.len(),
        ]
    }

    /// Waits until it stops, which it does only as [`stop`](Self::stop) has it: for the `gg-relay`
    /// executable, which relays until the process ends.
    pub fn wait(self) {
        let _ = self.accepting.join();
        let _ = self.forwarding.join();
    }

    /// Stops it: shuts every connection it holds, which ends their threads, and ends the threads
    /// that accept connections and forward datagrams, closing its port.
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

/// TCP's listener and UDP's socket at `address`, on one port. Port 0 can't be left to the OS: it
/// picks each protocol's port by a count of its own, in turn on Windows, where the one it gives TCP
/// can be excluded for UDP, as stretches of hundreds are, failing every relay started until the
/// count passes. So it's picked here, at random among the dynamic ports, until one is free for
/// both, [`PICKS`] times at most, as the `portpicker` crate does.
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

/// Accepts each connection to `listener`, refusing it past `shared`'s most, and starts the thread
/// that serves it, until the relay stops; those refused for being full go to `close`.
fn accept(listener: TcpListener, shared: &Arc<Shared>, close: &SyncSender<TcpStream>) {
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
        let admitted = {
            let mut state = shared.lock();
            // Read under the lock, so that `stop`, which sets it before it shuts every connection
            // open, shuts any taken before it.
            if shared.stopping.load(Ordering::SeqCst) {
                return;
            }
            if state.open.len() >= shared.most {
                if !std::mem::replace(&mut state.full, true) {
                    warn!(
                        code = "relay.refused",
                        most = shared.most,
                        "Refusing connections: the relay holds the most it holds, {}, as those \
                         after are until one closes",
                        shared.most
                    );
                }
                None
            } else {
                state.full = false;
                let number = state.number();
                stream.try_clone().ok().map(|clone| {
                    state.open.insert(number, clone);
                    Open {
                        shared: Arc::clone(shared),
                        number,
                    }
                })
            }
        };
        let Some(open) = admitted else {
            let _ = stream.write_all(b"refused full\n");
            let _ = stream.shutdown(Shutdown::Write);
            // Closed at once, past those waiting.
            let _ = close.try_send(stream);
            continue;
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

/// Closes each connection that comes through `closing`, refused, once its other end has closed
/// it, or [`LINGER`] has passed, reading what it sent meanwhile: so that it closes without a reset,
/// which could have the other end lose the refusal.
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

/// Serves a connection, `stream`, by its first line, which comes within [`ANSWER`]: a host's, a
/// joiner's or a host's new connection's. Ends once it does, closing it, `open` with it.
fn serve_connection(mut stream: TcpStream, open: Open) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(ANSWER));
    let line = match answer(&mut stream, ANSWER) {
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
        // Read to its end, so that the close sends no reset, which could lose the refusal.
        let _ = stream.set_read_timeout(Some(ANSWER));
        let _ = io::copy(&mut stream.take(64 << 10), &mut io::sink());
    }
}

/// A host of `room`, on `stream`, `open`'s: holds the room while it stands, the first host's alone,
/// telling its host of each joiner that comes, and saying `k` each second, on a thread that writes
/// to it; and dropping it once nothing has come from it for [`SILENT`], or what came isn't `k`.
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
    // Its `come`'s end, which ends the writing thread.
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

/// Writes to a host's `stream` each joiner's ticket that comes through `tickets`, and `k` after
/// [`KEEP`] with none, until the room's host is gone, or a write fails.
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

/// A joiner of `room`, on `stream`, `open`'s: answers `none` while no host holds the room, and
/// otherwise tells its host of it and waits [`TAKE`] for the host to take it, refusing it if it
/// doesn't, and copying what it sends to the host once it does.
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
    let pair = match pairing.recv_timeout(TAKE) {
        Ok(pair) => pair,
        Err(_) => {
            // The thread that takes it may have just now; whichever takes it from `waiting` has it.
            let gone = open.shared.lock().waiting.remove(&ticket).is_some();
            if gone {
                let secs = TAKE.as_secs();
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

/// A host's new connection, on `stream`, `open`'s, taking the joiner `ticket` names: pairs the two,
/// tells each end its token, and copies what the host sends to the joiner, the joiner's thread
/// copying what the joiner sends.
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
        state.pairs.insert(
            number,
            Pair {
                room: waiting.room,
                tokens,
                bound: [None; 2],
                bytes: [0; 2],
                datagrams: [0; 2],
                ends: 2,
                made: Instant::now(),
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
        end(&open.shared, number, 0, 0);
    }
    copy(stream, joiner, &open, number, 1);
}

/// Copies what comes from `from`, end `side` of pair `number`, to `to`, the other end, until `from`
/// ends, which it then ends `to`'s sending side at, as `from`'s end closed its own, or nothing has
/// come from it for [`IDLE`], or a read or a write fails, which shut both; then counts the end done.
fn copy(mut from: TcpStream, mut to: TcpStream, open: &Open, number: u64, side: usize) {
    let _ = from.set_read_timeout(Some(IDLE));
    let _ = to.set_write_timeout(Some(IDLE));
    let mut buffer = [0; 16 << 10];
    let mut bytes = 0;
    let ended = loop {
        match from.read(&mut buffer) {
            Ok(0) => {
                // As the end closed its own sending side: the other reads all that came, then its
                // end, which closes the pair without a reset.
                let _ = to.shutdown(Shutdown::Write);
                break Ok(());
            }
            Ok(read) => {
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
    end(&open.shared, number, side, bytes);
}

/// Counts end `side` of pair `number` done, having copied `bytes`: once both are, the pair ends,
/// and its tokens and bound sockets with it, which it logs.
fn end(shared: &Shared, number: u64, side: usize, bytes: u64) {
    let mut state = shared.lock();
    let Some(pair) = state.pairs.get_mut(&number) else {
        return;
    };
    pair.bytes[side] = bytes;
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
        secs = pair.made.elapsed().as_secs_f64(),
        "A pair ended: the joiner sent {} bytes and {} datagrams, the host {} and {}",
        pair.bytes[0],
        pair.datagrams[0],
        pair.bytes[1],
        pair.datagrams[1],
    );
}

/// Takes each datagram that comes to `udp` until the relay stops: a bind, which binds the socket it
/// came from to the end of the pair its token names, answering where it came from; or another, from
/// a bound socket, which goes to the other end's.
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
            // Windows' answer to a datagram sent to a port nothing listens at, as an end's after
            // it has gone.
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
        let to = {
            let mut state = shared.lock();
            let State { bound, pairs, .. } = &mut *state;
            bound.get(&from).and_then(|&(number, side)| {
                let pair = pairs.get_mut(&number)?;
                pair.datagrams[side] += 1;
                pair.bound[1 - side]
            })
        };
        if let Some(to) = to
            && udp.send_to(datagram, to).is_ok()
        {
            shared.forwarded.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Binds the socket at `from` to the end of the pair `token` names, in place of the one bound
/// before, if any; whether the token names one.
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
