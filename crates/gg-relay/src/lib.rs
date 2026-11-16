//! The relay a session's runs meet through when routers keep them apart
//! ([Networking](../../../ARCHITECTURE.md#networking)): both ends of its protocol, and the relay
//! ([`serve`], [`run`]).
//!
//! A relay pairs a host with each joiner that comes for its [`Room`]. It carries each pair's bytes
//! and datagrams both ways, reading none: the runs seal them under keys of their own.
//!
//! Its lines, each ending `\n`, at most [`MAX_LINE`] bytes:
//!
//! ```text
//! gg-relay 1 host <room>      a host or server registers its room (32 hex digits); answered
//!                               `hosting`, or `refused <why>`, as for a room held or a relay full
//! come <ticket>               relay to host: a joiner came for its room (64 hex digits)
//! k                           host and relay, each second: still there
//! gg-relay 1 take <ticket>    a host's new connection, for the ticket's joiner
//! gg-relay 1 join <room>      a joiner or client; answered `none` while no host holds the room,
//!                               or `refused <why>`
//! paired <token>              relay to each end once the host takes the pair: the other end's
//!                               bytes follow; the token (64 hex digits) binds this end's socket
//! ```
//!
//! Its datagrams, on the same port over UDP:
//!
//! ```text
//! gg-relay bind <token>       an end's socket, binding it to the token's end of a pair
//! gg-relay bound <address>    the relay's answer: the bind's source address, as routers map it
//! ```
//!
//! Any other datagram from a bound socket goes to the other end's. One starting `gg-relay ` is the
//! relay's own, never forwarded; no sealed packet starts so, as its first byte is 0x80 or more.
//!
//! A relay holds each end of a pair to its [`Limits`]' rates. It refuses a connection past the most
//! it holds, in all or from one address, answering `refused <why>` before its first line comes.
//!
//! Both ends read lines a byte at a time, so that no byte after `paired` is lost in a buffer.

mod relay;

use std::ffi::OsString;
use std::fmt;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;
use std::time::{Duration, Instant};

pub use relay::{Relay, serve};

/// What logs call a pair's ends, by side.
const ENDS: [&str; 2] = ["joiner", "host"];

/// The version each connection's first line names; any change to the lines or datagrams bumps it.
const VERSION: &str = "gg-relay 1";

/// What a relay's datagrams start with, and their two kinds.
const DATAGRAM: &[u8] = b"gg-relay ";
const BIND: &[u8] = b"gg-relay bind ";
const BOUND: &[u8] = b"gg-relay bound ";

/// The most bytes a line may have, its `\n` included; the reader drops the connection past it.
pub const MAX_LINE: usize = 256;

/// How long a first line may take to reach a relay, and its answer to come back; a joiner's answer,
/// [`TAKE`] more, as it waits for its host.
const ANSWER: Duration = Duration::from_secs(5);

/// How long a relay waits for a host to take a joiner.
const TAKE: Duration = Duration::from_secs(5);

/// How long each side of a host's connection goes quiet before it says `k`, and unheard before it
/// drops the other.
const KEEP: Duration = Duration::from_secs(1);
const SILENT: Duration = Duration::from_secs(10);

/// How long a pair's end may send nothing before the relay ends the pair. Sessions' links say `k`
/// each second.
const IDLE: Duration = Duration::from_secs(30);

/// The most bytes a datagram may have, a path's MTU, past which the relay drops it.
const MAX_DATAGRAM: usize = 1500;

/// A relay's defaults: where it listens, and the most connections, in all and from one address.
const LISTEN: &str = "0.0.0.0:7800";
const MOST: usize = 4096;
const PER_ADDRESS: usize = 16;

/// What a relay holds and forwards at most, as `gg-relay`'s flags set them. The default sits well
/// above what a game's session sends, so that only what isn't one meets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Connections at once, past which it refuses them (`full`).
    pub most: usize,
    /// Connections at once from one address, past which it refuses them; an IPv6 /64 is one
    /// address. A session takes three: its host's room and the pair's two ends. A server takes one,
    /// and two for each client.
    pub per_address: usize,
    /// Each end's bytes on its connection, past which the relay reads it more slowly. TCP holds
    /// the sender meanwhile, so none are lost.
    pub stream: Rate,
    /// Each end's datagrams, and their bytes, past which it drops them as a lossy link would;
    /// sessions send them again.
    pub datagrams: Rate,
    pub datagram_bytes: Rate,
}

/// A token bucket's rate: `per_second`, 1 or more, in bursts of up to `burst`. A datagram of more
/// bytes than the burst never passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub per_second: u64,
    pub burst: u64,
}

impl Default for Limits {
    /// Four times the most datagrams a second, and three times their bytes, of a 3D session of two
    /// skinned characters and a ball, a server's to each client. The stream's burst passes a
    /// joiner's world at once; its rate is 30 times a session's stream after.
    fn default() -> Self {
        Self {
            most: MOST,
            per_address: PER_ADDRESS,
            stream: Rate {
                per_second: 128 << 10,
                burst: 1 << 20,
            },
            datagrams: Rate {
                per_second: 500,
                burst: 1000,
            },
            datagram_bytes: Rate {
                per_second: 256 << 10,
                burst: 512 << 10,
            },
        }
    }
}

/// A session's room at a relay: 16 bytes, as 32 hex digits. Runs derive it from the session's code,
/// so that only they find it; the relay and the path see it, but can't find the code from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Room(pub [u8; 16]);

impl Room {
    /// The room's first 8 hex digits, which logs name it by.
    fn short(&self) -> String {
        self.to_string()[..8].to_owned()
    }
}

impl fmt::Display for Room {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(&self.0, f)
    }
}

impl FromStr for Room {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        read_hex(text)
            .map(Self)
            .ok_or_else(|| "a room is 32 hex digits".to_owned())
    }
}

/// A relay's secret that binds a socket to an end of a pair: 256 bits from the OS's secure
/// generator, compared in constant time, and shown by `Debug` as none of it.
#[derive(Clone, Copy, Eq)]
pub struct Token([u8; 32]);

/// A relay's secret, as a [`Token`] is, by which a host's new connection takes a joiner.
#[derive(Clone, Copy, Eq)]
pub struct Ticket([u8; 32]);

/// [`Token`]'s and [`Ticket`]'s impls: 64 hex digits, and a hash of the bytes for the relay's maps.
macro_rules! secret {
    ($type:ident) => {
        impl $type {
            /// A new one from the OS's secure generator. Fails if the OS gives no random numbers.
            fn random() -> io::Result<Self> {
                let mut bytes = [0; 32];
                getrandom::fill(&mut bytes).map_err(|error| {
                    io::Error::other(format!("the OS gave no random numbers: {error}"))
                })?;
                Ok(Self(bytes))
            }

            fn parse(text: &str) -> Option<Self> {
                read_hex(text).map(Self)
            }
        }

        // By its bytes, as equality compares them.
        impl std::hash::Hash for $type {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                self.0.hash(state);
            }
        }

        impl PartialEq for $type {
            fn eq(&self, other: &Self) -> bool {
                let differ =
                    (self.0.iter().zip(&other.0)).fold(0, |differ, (a, b)| differ | (a ^ b));
                differ == 0
            }
        }

        impl fmt::Display for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write_hex(&self.0, f)
            }
        }

        impl fmt::Debug for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($type), "(..)"))
            }
        }
    };
}

secret!(Token);
secret!(Ticket);

/// Writes `bytes` as lowercase hex.
fn write_hex(bytes: &[u8], f: &mut impl fmt::Write) -> fmt::Result {
    bytes.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
}

/// The `N` bytes `text` holds as hex of either case; `None` unless it's exactly that.
fn read_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != 2 * N || !text.is_ascii() {
        return None;
    }
    let mut bytes = [0; N];
    for (byte, pair) in bytes.iter_mut().zip(text.as_bytes().chunks(2)) {
        let pair = std::str::from_utf8(pair).ok()?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(bytes)
}

/// A host's or server's hold on its room at a relay.
pub struct Hosting {
    stream: TcpStream,
    /// A line partly read, as a read's timeout may cut one off.
    partial: Vec<u8>,
    /// When something last came from the relay, and when this end last said `k`.
    heard: Instant,
    said: Instant,
}

impl Hosting {
    /// Registers `room` at the relay at `relay`.
    ///
    /// Fails with `ConnectionRefused`, saying why, if the relay refuses, as for a room held; or as
    /// connecting, writing or reading the answer within 5 s does.
    pub fn start(relay: SocketAddr, room: Room) -> io::Result<Self> {
        let mut stream = open(relay)?;
        write_line(&mut stream, &format!("{VERSION} host {room}"))?;
        match answer(&mut stream, ANSWER)?.as_str() {
            "hosting" => {
                let now = Instant::now();
                Ok(Self {
                    stream,
                    partial: Vec::new(),
                    heard: now,
                    said: now,
                })
            }
            line => Err(refused(line)),
        }
    }

    /// Waits up to a second for the next joiner or client, returning its ticket for [`take`], or
    /// `None` so that the caller can stop between calls. Says `k` each second.
    ///
    /// Fails once the relay, which says `k` each second, is silent for 10 s, closes, or sends an
    /// unknown line; or as a write does.
    pub fn joiner(&mut self) -> io::Result<Option<Ticket>> {
        let now = Instant::now();
        if now.saturating_duration_since(self.said) >= KEEP {
            write_line(&mut self.stream, "k")?;
            self.said = now;
        }
        let Some(line) = read_line(&mut self.stream, &mut self.partial, now + KEEP)? else {
            if self.heard.elapsed() >= SILENT {
                let silent = SILENT.as_secs();
                let why = format!("nothing came from the relay for {silent} s");
                return Err(io::Error::new(ErrorKind::TimedOut, why));
            }
            return Ok(None);
        };
        self.heard = Instant::now();
        match line.split_once(' ') {
            None if line == "k" => Ok(None),
            Some(("come", ticket)) => Ticket::parse(ticket)
                .map(Some)
                .ok_or_else(|| unknown(&line)),
            _ => Err(unknown(&line)),
        }
    }
}

/// What a joiner or a client finds at a room.
pub enum Joined {
    /// The room's host took it.
    Paired(Paired),
    /// No host holds the room, perhaps not yet.
    Unhosted,
}

/// An end of a pair at a relay: the connection, whose next bytes are the other end's, and the
/// token for [`bind`].
pub struct Paired {
    pub stream: TcpStream,
    pub token: Token,
}

/// Comes for `room` at the relay at `relay`, as a joiner or a client, waiting the relay's 5 s for
/// the host to take it.
///
/// Fails with `ConnectionRefused`, saying why, if the relay refuses, as when the host doesn't take
/// it or the relay is full; or as connecting, writing or reading the answer within 10 s does.
pub fn join(relay: SocketAddr, room: Room) -> io::Result<Joined> {
    let mut stream = open(relay)?;
    write_line(&mut stream, &format!("{VERSION} join {room}"))?;
    let line = answer(&mut stream, TAKE + ANSWER)?;
    if line == "none" {
        return Ok(Joined::Unhosted);
    }
    paired(stream, &line).map(Joined::Paired)
}

/// Takes the joiner or client that `ticket`, from [`Hosting::joiner`], names, on a new connection
/// to the relay at `relay`.
///
/// Fails with `ConnectionRefused`, saying why, if the relay refuses, as once the joiner has gone;
/// or as connecting, writing or reading the answer within 5 s does.
pub fn take(relay: SocketAddr, ticket: &Ticket) -> io::Result<Paired> {
    let mut stream = open(relay)?;
    write_line(&mut stream, &format!("{VERSION} take {ticket}"))?;
    let line = answer(&mut stream, ANSWER)?;
    paired(stream, &line)
}

/// The end of a pair that the answer `line` on `stream` makes, its timeouts cleared.
fn paired(stream: TcpStream, line: &str) -> io::Result<Paired> {
    let Some(token) = line.strip_prefix("paired ") else {
        return Err(refused(line));
    };
    let token = Token::parse(token).ok_or_else(|| unknown(line))?;
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    Ok(Paired { stream, token })
}

/// Writes into `datagram` a bind of its sender to the end of a pair `token` names. A link's socket
/// sends it until [`bound`] reads an answer, and again now and then, as routers' mappings change.
pub fn bind(token: &Token, datagram: &mut Vec<u8>) {
    datagram.clear();
    datagram.extend_from_slice(BIND);
    datagram.extend_from_slice(token.to_string().as_bytes());
}

/// The address a relay's answer to a bind says it came from: the socket's, as routers map it.
/// `None` if `datagram` isn't one.
pub fn bound(datagram: &[u8]) -> Option<SocketAddr> {
    std::str::from_utf8(datagram.strip_prefix(BOUND)?)
        .ok()?
        .parse()
        .ok()
}

/// Connects to the relay at `relay`, within [`ANSWER`].
fn open(relay: SocketAddr) -> io::Result<TcpStream> {
    let stream = TcpStream::connect_timeout(&relay, ANSWER)?;
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(ANSWER))?;
    Ok(stream)
}

/// Writes `line` and its line break to `stream`, in one write.
fn write_line(stream: &mut TcpStream, line: &str) -> io::Result<()> {
    stream.write_all(format!("{line}\n").as_bytes())
}

/// Reads a line within `within`, as [`read_line`] does, failing with `TimedOut` if none comes.
fn answer(stream: &mut TcpStream, within: Duration) -> io::Result<String> {
    read_line(stream, &mut Vec::new(), Instant::now() + within)?.ok_or_else(|| {
        let why = format!("no line came within {} s", within.as_secs());
        io::Error::new(ErrorKind::TimedOut, why)
    })
}

/// Reads a line, without its break, onto `partial`, a byte at a time so that nothing past it is
/// read. `None` once `deadline` passes, `partial` keeping what came.
///
/// Fails with `UnexpectedEof` once the connection closes, and `InvalidData` at a line of
/// [`MAX_LINE`] bytes or more, or one that isn't UTF-8.
fn read_line(
    stream: &mut TcpStream,
    partial: &mut Vec<u8>,
    deadline: Instant,
) -> io::Result<Option<String>> {
    let mut byte = [0];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        stream.set_read_timeout(Some(left))?;
        match stream.read(&mut byte) {
            Ok(0) => {
                let why = "the connection closed";
                return Err(io::Error::new(ErrorKind::UnexpectedEof, why));
            }
            Ok(_) if byte[0] == b'\n' => {
                let line = std::mem::take(partial);
                return String::from_utf8(line)
                    .map(Some)
                    .map_err(|error| io::Error::new(ErrorKind::InvalidData, error));
            }
            Ok(_) if partial.len() + 1 >= MAX_LINE => {
                let why = format!("a line of {MAX_LINE} bytes or more");
                return Err(io::Error::new(ErrorKind::InvalidData, why));
            }
            Ok(_) => partial.push(byte[0]),
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
    }
}

/// The error for an answer `line` not asked for: the relay's refusal, with why, or an unknown line.
fn refused(line: &str) -> io::Error {
    match line.strip_prefix("refused ") {
        Some(why) => io::Error::new(
            ErrorKind::ConnectionRefused,
            format!("the relay refused it: {why}"),
        ),
        None => unknown(line),
    }
}

/// The error for a line that isn't one of the relay's.
fn unknown(line: &str) -> io::Error {
    let line: String = line.chars().take(64).collect();
    io::Error::new(
        ErrorKind::InvalidData,
        format!("the relay sent a line that isn't one: {line}"),
    )
}

/// The help text, for the program called `program`.
fn usage(program: &str) -> String {
    let Limits {
        stream,
        datagrams,
        datagram_bytes: bytes,
        ..
    } = Limits::default();
    let kib = |rate: Rate| (rate.per_second >> 10, rate.burst >> 10);
    let ((stream_rate, stream_burst), (bytes_rate, bytes_burst)) = (kib(stream), kib(bytes));
    let (datagram_rate, datagram_burst) = (datagrams.per_second, datagrams.burst);
    format!(
        "Usage: {program} [options]

Pairs a session's runs that can't connect to each other, a host or a server with each joiner or
client that comes for its room, by its code, and copies what each pair sends the other, sealed
under keys the relay doesn't hold: over TCP, and over UDP on the same port.

Options:
  --listen <ADDRESS>   Listen at ADDRESS, TCP and UDP, rather than {LISTEN}
  --most <N>           Hold at most N connections at once, rather than {MOST}, refusing those past
                       it: a host holds one, and each joiner or client one, and one more of its
                       host's while it's paired
  --per-address <N>    Hold at most N of them from one address, rather than {PER_ADDRESS}, an IPv6
                       address's /64 counting as one
  --logs <DIR>         Write its JSON log in DIR, rather than in `logs` beside its executable
  -h, --help           Print this help

Each end of a pair may send, a second and in a burst:
  its stream     {stream_rate:>4} KiB  {stream_burst:>5} KiB  read more slowly past them
  datagrams      {datagram_rate:>4}      {datagram_burst:>5}      dropped past them
  their bytes    {bytes_rate:>4} KiB  {bytes_burst:>5} KiB  dropped past them

Environment:
  RUST_LOG             The log filter, such as `debug`

Exits 1 if it can't listen, and 2 on bad arguments.
"
    )
}

/// What [`parse`] reads from a relay's arguments.
#[derive(Debug, PartialEq)]
struct Options {
    listen: String,
    limits: Limits,
    /// The JSON log's folder; `None` for `logs` beside the executable.
    logs: Option<PathBuf>,
}

/// Runs a relay as the `gg-relay` executable does, by `args`, those after its name. It logs to the
/// console and a JSON log, and relays until the process ends.
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let program = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.file_stem()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "gg-relay".to_owned());
    let Options {
        listen,
        limits,
        logs,
    } = match parse(args) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            print!("{}", usage(&program));
            return ExitCode::SUCCESS;
        }
        Err(why) => {
            eprintln!("error: {why}\n\n{}", usage(&program));
            return ExitCode::from(2);
        }
    };
    let logs = logs.unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join("logs")))
            .unwrap_or_else(|| PathBuf::from("logs"))
    });
    gg_core::log::init("relay", &logs, true);
    match serve(&listen, limits) {
        Ok(relay) => {
            relay.wait();
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(code = "relay.start", "Can't relay at {listen}: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Parses [`run`]'s arguments; `None` for `--help`.
fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Option<Options>, String> {
    let mut options = Options {
        listen: LISTEN.to_owned(),
        limits: Limits::default(),
        logs: None,
    };
    let connections = |flag: &str, given: String| {
        given
            .parse()
            .ok()
            .filter(|&most: &usize| most > 0)
            .ok_or_else(|| {
                format!("`{flag}` takes a number of connections, 1 or more, not `{given}`")
            })
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let arg = (arg.into_string()).map_err(|arg| format!("{arg:?} isn't valid Unicode"))?;
        let (flag, value) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        let mut value = || {
            value
                .clone()
                .or_else(|| args.next().and_then(|value| value.into_string().ok()))
                .ok_or_else(|| format!("`{flag}` needs a value"))
        };
        match flag {
            "-h" | "--help" => return Ok(None),
            "--listen" => options.listen = value()?,
            "--most" => options.limits.most = connections(flag, value()?)?,
            "--per-address" => options.limits.per_address = connections(flag, value()?)?,
            "--logs" => {
                let given = value()?;
                if given.is_empty() {
                    return Err("`--logs` takes a folder, not nothing".to_owned());
                }
                options.logs = Some(PathBuf::from(given));
            }
            _ => return Err(format!("unknown argument `{arg}`")),
        }
    }
    Ok(Some(options))
}

#[cfg(test)]
mod tests;
