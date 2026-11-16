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

/// How long a joiner waits between tries at finding its host at a relay, and a host before
/// registering again at a relay it couldn't register at or lost.
const UNHOSTED: Duration = Duration::from_secs(1);
const REGISTER: Duration = Duration::from_secs(5);

/// What a hello names the run's build by, so a host or server refuses a run of another build,
/// whose simulation may differ: the source its code is compiled from
/// ([`App::source`](crate::App::source)) and the package it reads.
#[derive(Clone, Default)]
pub(crate) struct Build {
    pub(crate) source: Option<&'static str>,
    /// The run's package, if it reads one.
    pub(crate) files: Files,
}

impl Build {
    /// The build as the hello names it. With a source: its digest and the package's
    /// ([`Package::digest`](crate::package::Package::digest)), each 16 hex digits, `-` between, as
    /// in `5c7cd8abeeed5f44-641dce7a72446a9c`; the package's is `-` for files read from the disk.
    /// Without one: the executable's digest ([`executable`]), which builds of one source on other
    /// OSes or machines don't share.
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

/// The digest of the run's executable, 16 hex digits, read once per process: the build of an app
/// with no source ([`Build::name`]). Builds of one source differ across OSes and machines, as
/// rustc writes source and debug-info paths into the executable.
///
/// Fails with `net.build` if the executable can't be read.
pub(super) fn executable() -> Result<&'static str> {
    static BUILD: OnceLock<std::result::Result<String, String>> = OnceLock::new();
    let build = BUILD.get_or_init(|| {
        let _zone = gg_core::zone!();
        // Linux's link to the executable reads even once its file is deleted, as a running one's
        // can be.
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

/// The engine as the hello names it: version, profile, and `dev` for a development build, which
/// applies patches and scene files' saves other builds don't.
pub(super) fn engine() -> String {
    let engine = crate::recording::engine();
    if cfg!(feature = "dev") {
        format!("{engine} dev")
    } else {
        engine
    }
}

pub(super) fn hello(protocol: &str, app: &str, build: &str, layers: &str, port: u16) -> String {
    format!(
        "hello\t{protocol}\t{app}\t{}\t{build}\t{layers}\t{port}\n",
        engine()
    )
}

/// The packets' port of a [`hello`], `line`, if its protocol, app, engine, build and layers are
/// `ours` (`who` speaks for them); else why it's refused.
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

/// Takes `what` where `at` says: listens at its address on one thread and registers at its relay
/// ([`register`]) on another. Returns where connections come ([`Accepted`]), each through its
/// handshake keyed by `code`, and the address listened at, which a test's port 0 picks.
///
/// Fails with `net.start` if it can't listen there or start either thread.
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

/// Registers the session's room at `relay`, for `what`, and takes each that comes for it, through
/// its handshake on its own thread, into `shaken`, until the session `kept` holds is gone.
/// Registers again [`REGISTER`] after the relay can't be reached, refuses the room (as while a
/// host it lost holds it), or is lost, logging `net.relay` once each time. Notes in `heard` whether
/// the room is registered, or why not.
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

/// Sleeps `time` in steps of 100 ms while `kept`'s session lasts; returns whether it does.
fn sleep_while_kept(time: Duration, kept: &Weak<()>) -> bool {
    let wake = Instant::now() + time;
    while kept.strong_count() > 0 && Instant::now() < wake {
        let left = wake.saturating_duration_since(Instant::now());
        thread::sleep(left.min(Duration::from_millis(100)));
    }
    kept.strong_count() > 0
}

/// What the relay last answered the thread that registers the room or comes for it, for the
/// session's status ([`SessionPhase`]).
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

#[derive(Clone, Default)]
pub(crate) enum Answer {
    /// Nothing yet, or the session has no relay.
    #[default]
    Nothing,
    /// The host's room is registered, or the joiner's was paired with its host.
    Hosted,
    /// The joiner's room, which no host holds yet.
    Unhosted,
    /// The relay can't be reached, or refused or lost the room, saying why.
    Unreachable(Arc<str>),
}

/// What a joiner or client keeps while it looks for its host or server ([`connect`]): what tells
/// its threads the session is gone, and what the relay last answered.
#[derive(Default)]
pub(crate) struct Looking {
    session: Arc<()>,
    pub(super) heard: Heard,
}

/// The address `address` names, as [`preferred`] picks it.
///
/// Fails if it names none.
pub(super) fn resolve(address: &str) -> io::Result<SocketAddr> {
    let addresses: Vec<SocketAddr> = address.to_socket_addrs()?.collect();
    preferred(&addresses).ok_or_else(|| {
        let why = format!("{address} names no address");
        io::Error::new(ErrorKind::NotFound, why)
    })
}

/// The first IPv4 address of `addresses`, else the first. A relay listens on IPv4 by default and a
/// link's socket follows its connection's family, so a host name with an IPv6 address too, which a
/// resolver may list first, would otherwise reach none.
pub(super) fn preferred(addresses: &[SocketAddr]) -> Option<SocketAddr> {
    let v4 = addresses.iter().find(|address| address.is_ipv4());
    v4.or(addresses.first()).copied()
}

pub(super) type Handshaken = io::Result<Shaken>;

pub(super) struct Shaken {
    pub(super) stream: TcpStream,
    pub(super) keys: Keys,
    pub(super) relayed: Option<Relayed>,
}

/// Where a link through a relay binds its socket: the relay's address and its end's token.
#[derive(Clone, Copy)]
pub(super) struct Relayed {
    pub(super) relay: SocketAddr,
    pub(super) token: Token,
}

/// The connections a host or server accepts ([`listen`]), each once its handshake has ended. Each
/// runs on its own thread, so a slow connection holds neither the main thread, nor another
/// connection, nor a joiner's place before it has shown it holds the code. Also tells the
/// accepting thread the session is gone; it ends as the next connection comes.
pub(super) struct Accepted {
    handshaken: Receiver<Handshaken>,
    /// What the relay last answered the thread that registers the session's room, if any.
    pub(super) heard: Heard,
    _session: Arc<()>,
}

impl Accepted {
    pub(super) fn handshaken(&self) -> Option<Handshaken> {
        self.handshaken.try_recv().ok()
    }
}

/// Runs [`secure::handshake`] on `stream`, keyed by `code`, as the end that made the connection or,
/// if `accepted`, took it, waiting at most [`HELLO`] for the other's part. Returns the stream with
/// its keys, and where it's `relayed`.
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

/// Connects to `what` where `toward` says, on its own thread, after `after`: to its address,
/// retrying every [`RETRY`], or through its relay, coming for the room `code` gives, retrying every
/// [`UNHOSTED`] until its host takes it ([`gg_relay::join`]). Each answer is noted in `looking`'s
/// [`Heard`]; starting through a relay is logged, so a join's time reads from the log. Then runs
/// the handshake ([`shake`]). Returns where the connection comes, with its keys, or why the
/// handshake failed. The thread ends once `looking` is gone.
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
