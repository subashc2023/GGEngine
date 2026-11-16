//! How links are made: listening, connecting direct or through a relay, the handshake and hello.

use std::io::{self, ErrorKind};
use std::mem;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use gg_core::{Error, Result};
use gg_relay::{Hosting, Joined, Token};
use tracing::{info, warn};

use super::HELLO;
#[cfg(doc)]
use super::SessionPhase;
use super::secure::{self, Code, Keys};
use crate::args::{Listen, Toward};
use crate::package::Files;

/// How long a joiner waits between tries at connecting to its host.
const RETRY: Duration = Duration::from_millis(250);

/// How long a joiner through a relay waits between tries at finding its host there, as one that
/// hasn't registered yet isn't; and how long a host waits before registering again at a relay it
/// couldn't register at, or lost.
const UNHOSTED: Duration = Duration::from_secs(1);
const REGISTER: Duration = Duration::from_secs(5);

/// What a session's hello names the run's build by, so that a host or a server refuses a peer or a
/// client of another build, whose simulation may go otherwise, as one rebuilt with an edit's
/// would: the source its code is compiled from, as [`App::source`](crate::App::source) names it,
/// and the package it reads.
#[derive(Clone, Default)]
pub(crate) struct Build {
    /// The source's digest, as `App::source` names it.
    pub(crate) source: Option<&'static str>,
    /// Where the run reads its files: its package, if it reads one.
    pub(crate) files: Files,
}

impl Build {
    /// The build, as the hello names it: for an app that names its source, the source's digest
    /// and the run's package's, as [`Package::digest`](crate::package::Package::digest) has it,
    /// each 16 hex digits, `-` between them, as in `5c7cd8abeeed5f44-641dce7a72446a9c`, the
    /// package's `-` for a run that reads its files from the disk, which the hello doesn't name;
    /// for an app that names none, its executable's digest, as [`executable`] says, which builds
    /// of one source on other OSes or machines don't share.
    ///
    /// Fails with `net.build` if the package or the executable can't be read.
    pub(crate) fn name(&self) -> Result<String> {
        let Some(source) = self.source else {
            return executable().map(str::to_owned);
        };
        let package = match self.files.packaged() {
            Some(package) => {
                let digest = package.digest().map_err(|error| {
                    let why = "can't digest the run's package, which the hello names its build by";
                    Error::new("net.build", why).with_source(error)
                })?;
                format!("{digest:016x}")
            }
            None => "-".to_owned(),
        };
        Ok(format!("{source}-{package}"))
    }
}

/// The digest of the run's executable's bytes, as 16 hex digits, read once a process, which names
/// the build of an app that names no source, as [`Build::name`] says. Builds of one source on two
/// OSes differ, and on two machines, as rustc writes the paths of the sources and the debug info's
/// into the executable.
///
/// Fails with `net.build` if the executable can't be read.
pub(super) fn executable() -> Result<&'static str> {
    static BUILD: OnceLock<std::result::Result<String, String>> = OnceLock::new();
    let build = BUILD.get_or_init(|| {
        let _zone = gg_core::zone!();
        // Linux's link to the executable, which reads even once its file is deleted, as a run's
        // can be while it runs.
        let path = if cfg!(target_os = "linux") {
            PathBuf::from("/proc/self/exe")
        } else {
            std::env::current_exe().map_err(|error| error.to_string())?
        };
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("can't read {}: {error}", path.display()))?;
        Ok(format!("{:016x}", crate::ecs::bytes_digest(&bytes)))
    });
    build.as_deref().map_err(|why| {
        Error::new(
            "net.build",
            format!("can't digest the run's executable, which the hello names its build by: {why}"),
        )
    })
}

/// What the hello says the engine is: its version and profile, and whether it's a development
/// build, which applies patches and scene files' saves, as other builds don't.
pub(super) fn engine() -> String {
    let engine = crate::recording::engine();
    if cfg!(feature = "dev") {
        format!("{engine} dev")
    } else {
        engine
    }
}

/// The hello of a peer or a client that speaks `protocol`, of app `app`, build `build` and layers
/// `layers`, whose packets come from its socket's `port`: as the stream's lines name it.
pub(super) fn hello(protocol: &str, app: &str, build: &str, layers: &str, port: u16) -> String {
    format!(
        "hello\t{protocol}\t{app}\t{}\t{build}\t{layers}\t{port}\n",
        engine()
    )
}

/// Takes a hello, `line`, which [`hello`] writes: the port its packets come from, if it's of the
/// protocol, app, engine, build and layers `ours` are, which `who` speaks for; or else why it's
/// refused.
pub(super) fn port_of(line: &str, ours: [&str; 5], who: &str) -> std::result::Result<u16, String> {
    let parts: Vec<&str> = line.split('\t').collect();
    let port = parts.get(6).and_then(|port| port.parse::<u16>().ok());
    match (parts.get(1..6), port) {
        (Some(theirs), Some(port)) if parts.len() == 7 && theirs == &ours[..] => Ok(port),
        (Some([protocol, ..]), _) if *protocol != ours[0] => {
            Err(format!("it speaks {protocol}, and {who} {}", ours[0]))
        }
        (Some([_, app, engine, build, layers]), Some(_)) if parts.len() == 7 => Err(format!(
            "it's app {app}, engine {engine}, build {build}, layers `{layers}`, and {who} app {}, \
             engine {}, build {}, layers `{}`",
            ours[1], ours[2], ours[3], ours[4]
        )),
        _ => Err("its hello isn't one".to_owned()),
    }
}

/// Takes `what` where `at` says: listening at its address, on a thread of its own that accepts
/// each connection, and registering at its relay, on another, as [`register`] says; returns where
/// they come, each through its handshake keyed by `code`, as [`Accepted`] says, and the address
/// listened at as the OS has it, which a test's port 0 picks.
///
/// Fails with `net.start` if it can't listen there, or start either thread.
pub(super) fn listen(
    at: &Listen,
    what: &'static str,
    code: Code,
) -> Result<(Accepted, Option<SocketAddr>)> {
    let (shaken, handshaken) = mpsc::channel();
    let session = Arc::new(());
    let heard = Heard::default();
    let started = |error: io::Error| {
        let message = format!("can't start the thread that takes {what}");
        Error::new("net.start", message).with_source(error)
    };
    let mut listening = None;
    if let Some(address) = &at.address {
        let failed = |error: io::Error| {
            let message = format!("can't listen at {address} for {what}");
            Error::new("net.start", message).with_source(error)
        };
        let listener = TcpListener::bind(address).map_err(failed)?;
        listening = Some(listener.local_addr().map_err(failed)?);
        let (shaken, kept) = (shaken.clone(), Arc::downgrade(&session));
        thread::Builder::new()
            .name("gg-net-accept".to_owned())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    if kept.strong_count() == 0 {
                        break;
                    }
                    let shaken = shaken.clone();
                    handshake_on_its_own(move || {
                        let _ = shaken.send(shake(stream, &code, true, None));
                    });
                }
            })
            .map_err(started)?;
    }
    if let Some(relay) = &at.relay {
        let (relay, kept, heard) = (relay.clone(), Arc::downgrade(&session), heard.clone());
        thread::Builder::new()
            .name("gg-net-relay".to_owned())
            .spawn(move || register(&relay, what, code, &shaken, &kept, &heard))
            .map_err(started)?;
    }
    let accepted = Accepted {
        handshaken,
        heard,
        _session: session,
    };
    Ok((accepted, listening))
}

/// Runs `handshake` on a thread of its own, so that a connection slow to say its part holds no
/// other.
fn handshake_on_its_own(handshake: impl FnOnce() + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("gg-net-handshake".to_owned())
        .spawn(handshake);
    if let Err(error) = spawned {
        warn!(
            code = "net.link",
            "Can't start a connection's handshake: {error}"
        );
    }
}

/// Registers the session's room, which `code` gives, at the relay at `relay`, for `what`, and takes
/// each that comes for it, each through its handshake on a thread of its own, into `shaken`, until
/// the session, which `kept` holds, is gone. Registers again [`REGISTER`] after the relay can't be
/// reached, refuses it, as it does while a host it lost holds the room, or is lost: logged once
/// each time, `net.relay`, until it's registered again. Notes in `heard` whether the room is
/// registered, or why not.
fn register(
    relay: &str,
    what: &str,
    code: Code,
    shaken: &Sender<Handshaken>,
    kept: &Weak<()>,
    heard: &Heard,
) {
    let room = code.room();
    let mut warned = false;
    while kept.strong_count() > 0 {
        let registered = resolve(relay).and_then(|at| Ok((at, Hosting::start(at, room)?)));
        let lost = match registered {
            Ok((at, mut hosting)) => {
                info!(relay, "Registered the session at its relay, for {what}");
                heard.set(Answer::Hosted);
                warned = false;
                loop {
                    if kept.strong_count() == 0 {
                        return;
                    }
                    match hosting.joiner() {
                        Ok(None) => {}
                        Ok(Some(ticket)) => {
                            let shaken = shaken.clone();
                            handshake_on_its_own(move || {
                                let made = gg_relay::take(at, &ticket).and_then(|paired| {
                                    let relayed = Relayed {
                                        relay: at,
                                        token: paired.token,
                                    };
                                    shake(paired.stream, &code, true, Some(relayed))
                                });
                                let _ = shaken.send(made);
                            });
                        }
                        Err(error) => break format!("lost it: {error}"),
                    }
                }
            }
            Err(error) => format!("can't register there: {error}"),
        };
        heard.set(Answer::Unreachable(lost.as_str().into()));
        if !mem::replace(&mut warned, true) {
            let again = REGISTER.as_secs();
            warn!(
                code = "net.relay",
                relay, "The session's relay {relay}: {lost}; registering again every {again} s"
            );
        }
        sleep_while_kept(REGISTER, kept);
    }
}

/// Sleeps `time`, in steps of 100 ms, while `kept`'s session lasts; returns whether it still does.
fn sleep_while_kept(time: Duration, kept: &Weak<()>) -> bool {
    let wake = Instant::now() + time;
    while kept.strong_count() > 0 && Instant::now() < wake {
        let left = wake.saturating_duration_since(Instant::now());
        thread::sleep(left.min(Duration::from_millis(100)));
    }
    kept.strong_count() > 0
}

/// What a session's relay last answered the thread that registers its room there, or comes for
/// it, shared with that thread, for the session's status to say, as [`SessionPhase`] does.
#[derive(Clone, Default)]
pub(crate) struct Heard(Arc<Mutex<Answer>>);

impl Heard {
    fn set(&self, answer: Answer) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = answer;
    }

    pub(super) fn get(&self) -> Answer {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// What a relay answered, as [`Heard`] keeps it.
#[derive(Clone, Default)]
pub(crate) enum Answer {
    /// Nothing yet, or the session has no relay.
    #[default]
    Nothing,
    /// The host's room is registered; or the joiner's, which its host holds, paired it.
    Hosted,
    /// The joiner's room, which no host holds yet.
    Unhosted,
    /// The relay can't be reached, refused the room or lost it, saying why.
    Unreachable(Arc<str>),
}

/// What a joiner or a client keeps while it looks for its host or server, as [`connect`] does:
/// what tells the threads that connect it that the session is gone, which they end at, and what
/// the relay last answered them.
#[derive(Default)]
pub(crate) struct Looking {
    session: Arc<()>,
    pub(super) heard: Heard,
}

/// The address `address` names, as `host:port` or `ip:port`, as [`preferred`] picks it.
///
/// Fails if it names none.
pub(super) fn resolve(address: &str) -> io::Result<SocketAddr> {
    let addresses: Vec<SocketAddr> = address.to_socket_addrs()?.collect();
    preferred(&addresses).ok_or_else(|| {
        let why = format!("{address} names no address");
        io::Error::new(ErrorKind::NotFound, why)
    })
}

/// The first IPv4 address of `addresses`, else the first: a relay listens on IPv4 by default, and
/// a link's socket follows its connection's family, so that a host name with an IPv6 address too,
/// which a resolver may list first, would otherwise reach none.
pub(super) fn preferred(addresses: &[SocketAddr]) -> Option<SocketAddr> {
    let v4 = addresses.iter().find(|address| address.is_ipv4());
    v4.or(addresses.first()).copied()
}

/// A connection whose handshake held: its stream, the keys it made, and, for one made through a
/// relay, where the link's socket binds there; or why its handshake failed.
pub(super) type Handshaken = io::Result<Shaken>;

pub(super) struct Shaken {
    pub(super) stream: TcpStream,
    pub(super) keys: Keys,
    pub(super) relayed: Option<Relayed>,
}

/// Where a link through a relay binds its socket: the relay's address, and the token the relay
/// gave its end of the pair.
#[derive(Clone, Copy)]
pub(super) struct Relayed {
    pub(super) relay: SocketAddr,
    pub(super) token: Token,
}

/// The connections a host or a server accepts, as [`listen`] gives them, each once its handshake
/// has ended, which a thread of the connection's own runs, as the thread that accepts it starts
/// it: so that one slow to say its part holds neither the main thread, nor another, nor a
/// joiner's place before it has shown it holds the code, and a main thread busy awhile holds no
/// joiner's handshake. And what tells the thread that accepts them that the session is gone, which
/// it ends at as the next connection comes.
pub(super) struct Accepted {
    handshaken: Receiver<Handshaken>,
    /// What the relay last answered the thread that registers the session's room, if it has one.
    pub(super) heard: Heard,
    _session: Arc<()>,
}

impl Accepted {
    /// The next connection whose handshake has ended, if one has.
    pub(super) fn handshaken(&self) -> Option<Handshaken> {
        self.handshaken.try_recv().ok()
    }
}

/// Runs the handshake on `stream`, keyed by `code`, as the end that made the connection or,
/// `accepted`, the end that took it, as [`secure::handshake`] says, waiting at most [`HELLO`]
/// for the other end's part; returns the stream with the keys it made, and where it's `relayed`.
fn shake(
    mut stream: TcpStream,
    code: &Code,
    accepted: bool,
    relayed: Option<Relayed>,
) -> Handshaken {
    stream.set_read_timeout(Some(HELLO))?;
    stream.set_write_timeout(Some(HELLO))?;
    match secure::handshake(&mut stream, code, accepted) {
        Ok(keys) => Ok(Shaken {
            stream,
            keys,
            relayed,
        }),
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            let secs = HELLO.as_secs();
            let why = format!("its part of the handshake didn't come within {secs} s");
            Err(io::Error::new(error.kind(), why))
        }
        Err(error) => Err(error),
    }
}

/// Connects to `what` where `toward` says, on a thread of its own, after `after`: to its address,
/// trying again every [`RETRY`] until it does, or through its relay, coming for the room `code`
/// gives there, again every [`UNHOSTED`] until its host takes it, as [`gg_relay::join`] says,
/// noting each answer in `looking`'s [`Heard`], and logging as it starts through a relay, so that
/// a join's time reads from its log. Then runs the handshake keyed by `code` on the connection, as
/// [`shake`] does; returns where the connection comes, with its keys, or why its handshake failed.
/// The thread ends, connecting no more, once `looking` is gone, as its session is.
pub(super) fn connect(
    toward: Toward,
    what: &'static str,
    after: Duration,
    code: Code,
    looking: &Looking,
) -> Receiver<Handshaken> {
    let (made, connection) = mpsc::channel();
    let (kept, heard) = (Arc::downgrade(&looking.session), looking.heard.clone());
    let spawned = thread::Builder::new()
        .name("gg-net-connect".to_owned())
        .spawn(move || {
            if !sleep_while_kept(after, &kept) {
                return;
            }
            if let Toward::Relay(relay) = &toward {
                info!(relay = %relay, "Looking for {what} through its relay");
            }
            let mut warned = false;
            loop {
                let (shaken, wait) = match &toward {
                    Toward::Address(address) => {
                        let stream = address.to_socket_addrs().ok().and_then(|mut addresses| {
                            addresses.find_map(|at| TcpStream::connect(at).ok())
                        });
                        let shaken = stream.map(|stream| shake(stream, &code, false, None));
                        if shaken.is_none() && !mem::replace(&mut warned, true) {
                            info!(address = %address, "Waiting for {what}");
                        }
                        (shaken, RETRY)
                    }
                    Toward::Relay(relay) => {
                        let joined = resolve(relay)
                            .and_then(|at| Ok((at, gg_relay::join(at, code.room())?)));
                        let shaken = match joined {
                            Ok((at, Joined::Paired(paired))) => {
                                heard.set(Answer::Hosted);
                                let relayed = Relayed {
                                    relay: at,
                                    token: paired.token,
                                };
                                Some(shake(paired.stream, &code, false, Some(relayed)))
                            }
                            Ok((_, Joined::Unhosted)) => {
                                heard.set(Answer::Unhosted);
                                if !mem::replace(&mut warned, true) {
                                    info!(relay = %relay, "Waiting for {what} at the relay");
                                }
                                None
                            }
                            Err(error) => {
                                heard.set(Answer::Unreachable(error.to_string().into()));
                                if !mem::replace(&mut warned, true) {
                                    warn!(
                                        code = "net.relay",
                                        relay = %relay,
                                        "Can't find {what} at the relay {relay}: {error}; trying \
                                         again every {} s",
                                        UNHOSTED.as_secs()
                                    );
                                }
                                None
                            }
                        };
                        (shaken, UNHOSTED)
                    }
                };
                if let Some(shaken) = shaken {
                    let _ = made.send(shaken);
                    return;
                }
                if !sleep_while_kept(wait, &kept) {
                    return;
                }
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
