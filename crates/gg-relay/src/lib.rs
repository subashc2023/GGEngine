//! The relay that a session's runs meet through when they can't connect to each other, as two
//! behind their own routers can't, as [Networking](../../../ARCHITECTURE.md#networking) describes:
//! both ends of its protocol, what a host or a server ([`Hosting`], [`take`]), a joiner or a client
//! ([`join`]) and a link's socket ([`bind`], [`bound`]) say to a relay, and the relay itself
//! ([`serve`]), which the `gg-relay` executable runs ([`run`]).
//!
//! A relay pairs a host with each joiner that comes for its room, which the session's code gives
//! its runs alone, and copies the bytes of each pair's connection each way, and forwards the
//! datagrams of each pair's two sockets each way, reading none of them: what a session's runs send
//! each other is sealed under keys of their own. Its lines, each ending `\n`, at most [`MAX_LINE`]
//! bytes:
//!
//! ```text
//! gg-relay 1 host <room>      a host or a server, registering its room, 32 hex digits; the relay
//!                               answers `hosting`, or `refused <why>`, as for a room hosted
//!                               already or a relay full
//! come <ticket>               the relay, to a host: a joiner has come for its room, 64 hex digits
//! k                           a host and the relay, each second, to say it's there
//! gg-relay 1 take <ticket>    a host's new connection, for the joiner the ticket names
//! gg-relay 1 join <room>      a joiner or a client; the relay answers `none` while no host holds
//!                               the room, or `refused <why>`
//! paired <token>              the relay, to each end of a pair once its host has taken it: the
//!                               other end's bytes follow, and the token, 64 hex digits, binds
//!                               this end's socket
//! ```
//!
//! Its datagrams, on the same port over UDP:
//!
//! ```text
//! gg-relay bind <token>       an end's socket, binding it to its end of the pair the token names
//! gg-relay bound <address>    the relay's answer: the address the bind came from, which is the
//!                               socket's as the router before it maps it
//! ```
//!
//! Each other datagram from a bound socket goes to the other end's, whatever it holds but these:
//! a datagram that starts `gg-relay ` is the relay's own, never forwarded, which a sealed packet
//! never does, as its first byte is 0x80 or more.
//!
//! A relay holds each end of a pair to the rates its [`Limits`] give, and refuses a connection past
//! the most it holds, or past the most from one address, answering `refused <why>` before its first
//! line comes.
//!
//! Lines are read a byte at a time, at both ends, so that none of the bytes after `paired` land
//! in a buffer that whatever reads the connection next doesn't read.

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

/// Each end of a pair, as logs name it, by its side: the joiner's first.
const ENDS: [&str; 2] = ["joiner", "host"];

/// The protocol and its version, which each connection's first line names, and which a change to
/// the lines or the datagrams takes to the next.
const VERSION: &str = "gg-relay 1";

/// What a relay's datagrams start with, and their two kinds.
const DATAGRAM: &[u8] = b"gg-relay ";
const BIND: &[u8] = b"gg-relay bind ";
const BOUND: &[u8] = b"gg-relay bound ";

/// The most bytes a line may have, its line break among them, past which the end reading it drops
/// the connection.
pub const MAX_LINE: usize = 256;

/// How long a connection's first line may take to come to a relay, and a relay's answer to come
/// back, but a joiner's, which waits for its host too, [`TAKE`] more.
const ANSWER: Duration = Duration::from_secs(5);

/// How long a relay waits for a host to take a joiner that came for its room.
const TAKE: Duration = Duration::from_secs(5);

/// How long each side of a host's connection to its relay goes with nothing else to say before it
/// says `k`; and how long it goes without hearing the other before it's dropped.
const KEEP: Duration = Duration::from_secs(1);
const SILENT: Duration = Duration::from_secs(10);

/// How long a pair's connection may go with nothing from one of its ends, past which the relay
/// ends it, as a session's links say `k` each second.
const IDLE: Duration = Duration::from_secs(30);

/// The most bytes a datagram may have, a path's MTU, past which the relay drops it.
const MAX_DATAGRAM: usize = 1500;

/// Where a relay listens by default, the most connections it holds at once, and the most of them
/// from one address.
const LISTEN: &str = "0.0.0.0:7800";
const MOST: usize = 4096;
const PER_ADDRESS: usize = 16;

/// What a relay holds and forwards at most, as `gg-relay`'s flags set them; the default, the
/// executable's, sits well above what a game's session sends through it, so that only what isn't
/// one meets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Connections at once, past which it refuses them (`full`).
    pub most: usize,
    /// Connections at once from one address, an IPv6 address's /64 counting as one, as one host
    /// holds a whole /64, and an IPv4 address that IPv6 maps as that address; past it, it refuses
    /// them. A session through a relay takes three, its host's room, its joiner's and its host's
    /// end of their pair; a server one, and two for each client.
    pub per_address: usize,
    /// Each end of a pair's bytes on its connection, past which the relay reads it more slowly,
    /// TCP holding its sender meanwhile, so that none are lost.
    pub stream: Rate,
    /// Each end of a pair's datagrams, and their bytes, past which it drops them, as a lossy link
    /// would, which sessions send again.
    pub datagrams: Rate,
    pub datagram_bytes: Rate,
}

/// A rate, as a token bucket keeps it: so many a second, of 1 or more, in bursts of up to so many;
/// a datagram of more bytes than its burst never passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub per_second: u64,
    pub burst: u64,
}

impl Default for Limits {
    /// At four times the datagrams a second, and three times their bytes, that Foxball's sessions
    /// send at the most, a server's to each client; and a stream that passes the world a joiner
    /// takes at once, in its burst, and 30 times what a session's stream sends after.
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

/// A session's room at a relay, which its host registers by and its joiners come for: 16 bytes,
/// written as 32 hex digits. A host's run derives it from the session's code, which its joiners'
/// runs are given, so that they find it, and no one else can: the relay, and anyone on the path to
/// it, see the room, but can't find the code from it.
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

/// A secret of a relay's, 256 bits from the OS's secure generator, which binds a socket to an end
/// of a pair; compared in constant time, and its `Debug` writing none of it.
#[derive(Clone, Copy, Eq)]
pub struct Token([u8; 32]);

/// A secret of a relay's, as a [`Token`] is, which a host's new connection takes the joiner it
/// names by.
#[derive(Clone, Copy, Eq)]
pub struct Ticket([u8; 32]);

/// Each of [`Token`] and [`Ticket`]: made from the OS's secure generator, written and read as 64 hex
/// digits, compared in constant time, hashed as its bytes, which a relay's maps key it by, and
/// debugged as none of them.
macro_rules! secret {
    ($type:ident) => {
        impl $type {
            /// A new one, from the OS's secure generator.
            ///
            /// Fails if the OS gives no random numbers.
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

        // As its bytes, as its equality compares them.
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

/// Writes `bytes` as hex digits, two to a byte, lowercase.
fn write_hex(bytes: &[u8], f: &mut impl fmt::Write) -> fmt::Result {
    bytes.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
}

/// The `N` bytes `text` writes as hex digits, two to a byte, either case; `None` unless it's
/// exactly that.
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

/// A host's or a server's registration of its session's room at a relay, which holds it while it
/// stands, and tells it of each joiner or client that comes for it.
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
    /// Fails with `ConnectionRefused`, saying why, where the relay refuses it, as it does a room
    /// another host holds; and as connecting to it, writing to it or reading its answer within 5 s
    /// does.
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

    /// Waits up to a second for the next joiner or client to come for the room, which it returns
    /// the ticket of, for [`take`]; `None` once the second has passed without one, having said `k`
    /// if a second has passed since this end last did, so that its caller can stop between calls.
    ///
    /// Fails once nothing has come from the relay for 10 s, as the relay says `k` each second, or
    /// the relay has closed the connection or sent what isn't a line of its; and as writing to it
    /// does.
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

/// What a joiner or a client that comes for a room finds.
pub enum Joined {
    /// The room's host took it.
    Paired(Paired),
    /// No host holds the room, as one that hasn't registered yet doesn't.
    Unhosted,
}

/// An end of a pair at a relay: the connection, whose next bytes are the other end's, and the
/// token that binds this end's socket, as [`bind`] says.
pub struct Paired {
    pub stream: TcpStream,
    pub token: Token,
}

/// Comes for `room` at the relay at `relay`, as a joiner or a client does, waiting for the room's
/// host to take it, which a relay waits 5 s for.
///
/// Fails with `ConnectionRefused`, saying why, where the relay refuses it, as it does once the
/// host hasn't taken it in time, or a relay full does; and as connecting to it, writing to it or
/// reading its answer within 10 s does.
pub fn join(relay: SocketAddr, room: Room) -> io::Result<Joined> {
    let mut stream = open(relay)?;
    write_line(&mut stream, &format!("{VERSION} join {room}"))?;
    let line = answer(&mut stream, TAKE + ANSWER)?;
    if line == "none" {
        return Ok(Joined::Unhosted);
    }
    paired(stream, &line).map(Joined::Paired)
}

/// Takes the joiner or the client `ticket` names, which [`Hosting::joiner`] gave, on a new connection
/// to the relay at `relay`, as the room's host does.
///
/// Fails with `ConnectionRefused`, saying why, where the relay refuses it, as it does once the
/// joiner has gone; and as connecting to it, writing to it or reading its answer within 5 s does.
pub fn take(relay: SocketAddr, ticket: &Ticket) -> io::Result<Paired> {
    let mut stream = open(relay)?;
    write_line(&mut stream, &format!("{VERSION} take {ticket}"))?;
    let line = answer(&mut stream, ANSWER)?;
    paired(stream, &line)
}

/// The end of a pair the relay's answer `line` on `stream` makes, its timeouts none again.
fn paired(stream: TcpStream, line: &str) -> io::Result<Paired> {
    let Some(token) = line.strip_prefix("paired ") else {
        return Err(refused(line));
    };
    let token = Token::parse(token).ok_or_else(|| unknown(line))?;
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    Ok(Paired { stream, token })
}

/// Writes into `datagram` the bind of the socket that sends it to the end of a pair `token` names:
/// which a link's socket sends its relay until it's answered, as [`bound`] reads the answer, and
/// again now and then, as a router's mapping of it may change.
pub fn bind(token: &Token, datagram: &mut Vec<u8>) {
    datagram.clear();
    datagram.extend_from_slice(BIND);
    datagram.extend_from_slice(token.to_string().as_bytes());
}

/// The address a relay's answer to a bind, `datagram`, says the bind came from: the socket's, as
/// the routers on the way map it; `None` if it isn't one.
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

/// Reads a line from `stream` within `within`, as [`read_line`] does.
///
/// Fails with `TimedOut` if none comes in time.
fn answer(stream: &mut TcpStream, within: Duration) -> io::Result<String> {
    read_line(stream, &mut Vec::new(), Instant::now() + within)?.ok_or_else(|| {
        let why = format!("no line came within {} s", within.as_secs());
        io::Error::new(ErrorKind::TimedOut, why)
    })
}

/// Reads a line from `stream`, without its line break, a byte at a time, so that nothing past it
/// is read, onto what `partial` holds of it, by `deadline`; `None` once the deadline passes first,
/// `partial` holding what came of it.
///
/// Fails with `UnexpectedEof` once the connection closes, and with `InvalidData` at a line of
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

/// The error a relay's answer `line` that isn't the one asked for makes: its refusal, with why, or
/// a line that isn't one.
fn refused(line: &str) -> io::Error {
    match line.strip_prefix("refused ") {
        Some(why) => io::Error::new(
            ErrorKind::ConnectionRefused,
            format!("the relay refused it: {why}"),
        ),
        None => unknown(line),
    }
}

/// The error a line from a relay that isn't one of its makes.
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

/// A relay's options, as [`parse`] reads them from its arguments.
#[derive(Debug, PartialEq)]
struct Options {
    listen: String,
    limits: Limits,
    /// The JSON log's folder; `None` for `logs` beside the executable.
    logs: Option<PathBuf>,
}

/// Runs a relay as the `gg-relay` executable does, by the arguments after its name, `args`:
/// logging to the console and to a JSON log in the folder `--logs` names, else `logs` beside the
/// executable, as runs of the engine do, and relaying until the process ends.
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

/// Parses a relay's arguments, as [`run`] takes them; `None` for `--help`.
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
