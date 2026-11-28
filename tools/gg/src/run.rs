//! Running a game's development build: supervised, patched as its code changes, and restarted from
//! a snapshot after a crash, a hang or a rebuild, until it exits. Alone, or as the two peers of a
//! lockstep session; or headless for a set number of frames, as a check.

use std::fs::{self, TryLockError};
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use gg_api::{Client, Failure, Pulse, Session, Snapshot, Snapshotted};
use serde_json::{Value, json};

use crate::patch::{self, Patches, Process, Watcher};
use crate::runs::{self, Asked, State, Told};
use crate::{Scope, build, game, settings};

/// What `gg run` runs. `hang`: how long frames may stop before the game has hung, `None` for as
/// long as they stop. `snapshot`: how often, zero for never. `peers`: 1 for a game alone.
pub(crate) struct Run {
    pub(crate) package: String,
    pub(crate) project: Option<Scope>,
    pub(crate) release: bool,
    pub(crate) patches: bool,
    pub(crate) hang: Option<Duration>,
    pub(crate) snapshot: Duration,
    pub(crate) peers: usize,
    pub(crate) embedded: bool,
    pub(crate) args: Vec<String>,
}

impl Run {
    /// Why the run's options turn patches off, if they do. Restarts off does: an edit no patch
    /// carries restarts the game, and a recording would replay without the patches its run took.
    pub(crate) fn unpatched(&self) -> Option<&'static str> {
        let why = if !self.patches {
            "`--no-patch` turns them off"
        } else if self.release {
            "subsecond patches debug builds alone, and `--release` makes an optimized one"
        } else if !self.serves() {
            "the game doesn't serve the tool API, which patches go through"
        } else if self.given("--record") {
            "the recording would replay without the patches the run took"
        } else if self.snapshot.is_zero() {
            "an edit no patch carries restarts the game, which `--snapshot 0` turns off"
        } else {
            return None;
        };
        Some(why)
    }

    fn serves(&self) -> bool {
        !self.given("--no-api") && !self.given("--replay")
    }

    /// Whether the game restarts: only if snapshotted (which takes the API) and not recording: a
    /// restart would truncate the recording.
    pub(crate) fn restarts(&self) -> bool {
        self.serves() && !self.given("--record") && !self.snapshot.is_zero()
    }

    /// Whether the arguments give `flag`, as `--flag` or `--flag=value`.
    pub(crate) fn given(&self, flag: &str) -> bool {
        self.args.iter().any(|arg| {
            (arg.strip_prefix(flag)).is_some_and(|rest| rest.is_empty() || rest.starts_with('='))
        })
    }

    pub(crate) fn dir(&self) -> Option<&Path> {
        self.project.as_ref().map(Scope::dir)
    }
}

/// What `gg check` runs.
pub(crate) struct Check {
    pub(crate) package: String,
    pub(crate) project: Option<Scope>,
    pub(crate) release: bool,
    pub(crate) frames: u64,
    pub(crate) timeout: Duration,
    pub(crate) args: Vec<String>,
}

/// How long a game has to run its first frame, unless it waits for the user, before it's hung: a
/// debug build loading much takes a while. A load later, which `ping` says is `loading`, gets as
/// long.
const STARTUP: Duration = Duration::from_secs(120);

/// What `ping` says while the game loads a scene file, a scene's assets or a model.
const LOADING: &str = "loading";

const PING: Duration = Duration::from_secs(1);

/// A ping's answer comes from the connection's own thread.
const PULSE: Duration = Duration::from_secs(5);

/// The world is copied in a frame, but a thread writes it, slowly if large in a debug build.
const SNAPSHOT: Duration = Duration::from_secs(60);

/// The `closed` event's reason.
const CLOSED: &str = "gg's stdin closed, as the tool that started it ended";

/// The `quit` event's reason.
const QUIT_ASKED: &str = "`gg quit` asked, while no game ran";

/// How long a game asked to quit has to end before it's killed.
const QUIT: Duration = Duration::from_secs(10);

/// A changed executable must stay the same this long to count as rebuilt: cargo may be writing it.
const SETTLE: Duration = Duration::from_secs(1);

/// How long copying the executable, or removing the copy or a patch's library, is tried for:
/// Windows holds a file briefly after the process that ran or loaded it ends.
const COPYING: Duration = Duration::from_secs(5);

/// How long a game may take from its launch to serving, in ms, before `serving` says why on macOS.
/// A game serves within ~40 ms of `main`; macOS scans a new executable before `main`, about 3 s
/// for a 200 MB debug build, unless the launching app is one of its Developer Tools. Gatekeeper
/// also scans the bundle `dx` builds patches in as the first loads, 2.4 s for a new game on an M3
/// Pro, and the provenance it records spares the rest.
const SCANNED: u64 = 1500;

const SCAN_HINT: &str = "the game took this long to launch, as macOS scans each new build as it \
                         first launches: about 3 s a restart, and 2.4 s more for a new game's \
                         first patch. Adding the terminal, or the app running gg, under System \
                         Settings › Privacy & Security › Developer Tools skips it";

/// Prints `event`, a line of JSON.
pub(crate) fn event(event: &Value) {
    crate::print(&event.to_string());
}

pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// `event` with `queued_ms` if the build it tells of waited `queued` for dx's lock; its `build_ms`
/// leaves that wait out.
pub(crate) fn with_queued(mut event: Value, queued: Duration) -> Value {
    if !queued.is_zero() {
        event["queued_ms"] = json!(millis(queued));
    }
    event
}

/// Builds `run`'s package ([`Patches::new`]), prints `built` with the time, and supervises a copy
/// of its executable, so the original can be rebuilt meanwhile, until the game exits cleanly.
/// Returns whether it did, printing a line per event.
///
/// - Restarts on: snapshots the game as it serves the API, and restarts it from a snapshot after a
///   crash, a hang or a rebuild ([`next`]). Off: the game's first end ends the run.
/// - Patches on: patches or rebuilds the game as its sources change ([`Patches::tend`]).
/// - Two peers run as one game: the host (`--host`, at a loopback port the OS picks) and the
///   joiner (`--join` there), each a copy of its own from its own snapshot, the joiner going on
///   from the host's world as it joins. They are patched, stopped and restarted together, so they
///   always run the same code.
/// - Embedded: each game gets `--embedded`. The run ends cleanly, stopping its games, once gg's
///   stdin closes; a build under way finishes first.
///
/// Each game gets `--supervised` and a stdin gg holds, which closes as gg ends, however it ends,
/// and ends the game. A game still running as the run fails is killed.
///
/// # Errors
///
/// Fails as [`check_game`] does, and with:
/// - `gg.running` if another `gg run` of the package runs ([`lock`])
/// - `gg.build` if it doesn't build
/// - `gg.cargo` if cargo can't say what the package is
/// - `gg.run` if it can't be copied, started or waited for, or no port is free for the peers
pub(crate) fn run(run: &Run) -> Result<bool, Failure> {
    check_game(&run.package, run.project.as_ref())?;
    let (_lock, target) = lock(&run.package, run.dir())?;
    let pid = std::process::id();
    // Before the build: gg's other commands find no game while it builds.
    let manifest = project_of(run);
    // As `Patches::new` builds and watches.
    let rebuilds = run.patches && run.restarts();
    let told = Told::start(
        runs::path(&target, &run.package),
        pid,
        &run.package,
        manifest.as_deref(),
        rebuilds,
        &State::new("building"),
    );
    // Before the build: the tool may end under way.
    let closed = run.embedded.then(closes);
    let building = Instant::now();
    let patches = match first_build(run, &told, closed.as_deref())? {
        First::Built(patches) => patches,
        First::Closed => {
            event(&json!({"event": "closed", "why": CLOSED}));
            return Ok(true);
        }
        First::Quit => {
            event(&json!({"event": "quit", "why": QUIT_ASKED}));
            return Ok(true);
        }
    };
    let exe = match &patches {
        Some(patches) => patches.exe().to_owned(),
        None => build::build(&run.package, release(run.release), run.dir())?.exe,
    };
    // The wait for dx's lock isn't the build's time.
    let queued = patches
        .as_ref()
        .map_or(Duration::ZERO, |patches| patches.queued());
    let build_ms = millis(building.elapsed().saturating_sub(queued));
    event(&with_queued(
        json!({"event": "built", "exe": exe, "build_ms": build_ms}),
        queued,
    ));
    let dir = exe.parent().unwrap_or(Path::new(".")).to_owned();
    // A directory of its own, which the game's pruning of `snapshots` leaves alone.
    let snapshots = dir.join("snapshots").join("gg");
    let restarts = run.restarts();
    if restarts {
        // A snapshot that can't be written says why.
        let _ = fs::create_dir_all(&snapshots);
    }
    remove_leftovers(&exe, pid);
    remove_snapshots_left(&snapshots, &run.package, pid);
    let peers = peers_args(run.peers)?.into_iter().enumerate();
    let peers = peers.map(|(peer, mut args)| {
        args.insert(0, "--supervised".to_owned());
        if run.embedded {
            args.insert(0, "--embedded".to_owned());
        }
        Peer {
            args,
            copy: running_copy(&exe, pid, peer),
            snapshot: snapshots.join(snapshot_name(&run.package, pid, peer)),
            latest: None,
        }
    });
    if closed
        .as_ref()
        .is_some_and(|closed| closed.load(Ordering::Relaxed))
    {
        event(&json!({"event": "closed", "why": CLOSED}));
        return Ok(true);
    }
    let peers: Vec<Peer> = peers.collect();
    let mut patches = patches.map(|patches| *patches);
    // A lone game restarts in place into its own snapshot, which the run restarts it from.
    if let (Some(patches), [peer]) = (&mut patches, &peers[..])
        && restarts
    {
        patches.restart_into(peer.snapshot.clone());
    }
    let mut supervisor = Supervisor {
        run,
        peers,
        exe,
        dir,
        serves: run.serves(),
        restarts,
        served: false,
        scan_told: false,
        failed: None,
        patches,
        closed,
        told: Some(told),
    };
    let supervised = supervisor.supervise();
    if let Some(patches) = &mut supervisor.patches {
        patches.finish();
    }
    // Once its games have ended; one still running keeps its copy and the libraries it loaded.
    let copies = supervisor.peers.iter();
    let kept = copies
        .filter(|peer| retrying(|| remove(&peer.copy)).is_err())
        .count();
    if kept == 0
        && let Some(patches) = &supervisor.patches
    {
        patches.remove_libs();
    }
    supervised
}

enum First {
    /// Built, with these patches if they're on.
    Built(Option<Box<Patches>>),
    /// The embedding tool ended while the run waited for a change.
    Closed,
    /// `gg quit` asked the run to end while it waited for a change.
    Quit,
}

/// Builds `run`'s game the first time ([`Patches::new`]), returning its patches.
///
/// While it watches the sources (patches and restarts on), a failed build prints `build_failed`
/// and tells `waiting` with rustc's errors until the sources change or `gg rebuild` asks
/// ([`Told::asked_to_rebuild`]), then builds again, answering the ask.
/// [`First::Closed`] if `closed` says the tool ended meanwhile, and [`First::Quit`] if `gg quit`
/// asked ([`Told::asked_to_quit`]).
///
/// # Errors
///
/// Fails as `Patches::new` does, but not with `gg.build` while it watches.
fn first_build(run: &Run, told: &Told, closed: Option<&AtomicBool>) -> Result<First, Failure> {
    let watches = run.patches && run.restarts();
    let mut asked = None;
    loop {
        let failure = match Patches::new(run) {
            Ok(patches) => {
                // The game's first start is afresh, as `gg rebuild --fresh` asks.
                if let Some(asked) = asked {
                    told.answer(
                        Asked {
                            fresh: true,
                            ..asked
                        },
                        None,
                    );
                }
                return Ok(First::Built(patches.map(Box::new)));
            }
            Err(failure) if watches && failure.code == "gg.build" => failure,
            Err(failure) => return Err(failure),
        };
        if let Some(asked) = asked.take() {
            told.answer(asked, Some(&failure));
        }
        let error = json!({"code": failure.code, "message": failure.message});
        event(&json!({"event": "build_failed", "error": error}));
        let mut watcher = Watcher::new(run)?;
        let waiting = State {
            why: Some("its first build failed; a change to its sources builds it again".to_owned()),
            current: Some(false),
            error: Some(failure),
            ..State::new("waiting")
        };
        loop {
            let (now, looked) = (Instant::now(), runs::now_ms());
            if closed.is_some_and(|closed| closed.load(Ordering::Relaxed)) {
                return Ok(First::Closed);
            }
            if told.asked_to_quit() {
                return Ok(First::Quit);
            }
            asked = told.asked_to_rebuild();
            if asked.is_some() || watcher.changed(now) {
                break;
            }
            told.tell(&waiting, looked);
            thread::sleep(Duration::from_millis(100));
        }
        told.tell(&State::new("building"), runs::now_ms());
    }
}

/// Takes the lock of `package`'s `gg run`, a file in the target directory (`cargo metadata` in
/// `dir`), held until the process ends. A second `gg run` is refused: the two would build, patch
/// and restart the game over each other. Returns it with the target directory.
///
/// # Errors
///
/// - `gg.running` if another process holds it
/// - `gg.cargo` if cargo can't say where the target directory is
/// - `gg.run` if the file can't be opened or locked
fn lock(package: &str, dir: Option<&Path>) -> Result<(fs::File, PathBuf), Failure> {
    let target = build::target_dir(dir)?;
    let _ = fs::create_dir_all(&target);
    let lock = lock_at(&target.join(format!("gg-run-{package}.lock")), package)?;
    Ok((lock, target))
}

fn lock_at(path: &Path, package: &str) -> Result<fs::File, Failure> {
    let failed = |error: &dyn std::fmt::Display| {
        let message = format!("can't lock {}: {error}", path.display());
        Failure::new("gg.run", message)
    };
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|error| failed(&error))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(Failure::new(
            "gg.running",
            format!(
                "another `gg run` runs {package}, holding {}, and one run builds, patches and \
                 restarts a game; end it, or reach its game with gg's other commands",
                path.display()
            ),
        )),
        Err(TryLockError::Error(error)) => Err(failed(&error)),
    }
}

/// Watches gg's stdin on a thread, which sets the result once stdin closes (the embedding tool
/// ended), whatever was written.
fn closes() -> Arc<AtomicBool> {
    let closed = Arc::new(AtomicBool::new(false));
    let setting = closed.clone();
    thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
        setting.store(true, Ordering::Relaxed);
    });
    closed
}

/// Each peer's arguments beside the run's: none for a game alone; for two, `--host` and `--join`
/// at a free loopback port, and both the session's `--code` ([`session_code`]).
///
/// # Errors
///
/// - `gg.run` if no port is free
fn peers_args(peers: usize) -> Result<Vec<Vec<String>>, Failure> {
    if peers == 1 {
        return Ok(vec![Vec::new()]);
    }
    let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).and_then(|free| free.local_addr());
    let address = free.map_err(|error| {
        Failure::new(
            "gg.run",
            format!("no port of the loopback is free for the peers: {error}"),
        )
    })?;
    let (address, code) = (address.to_string(), session_code());
    let session = |flag: &str| {
        [flag, &address, "--code", &code]
            .map(str::to_owned)
            .to_vec()
    };
    Ok(vec![session("--host"), session("--join")])
}

/// A session's code, as gg-engine's `--code` reads it: 80 bits from `RandomState` (OS-seeded), as
/// 16 Crockford base-32 symbols in fours, like `7KQ2-9XMB-4TZD-P0WH`. It keeps the session from
/// this machine's other runs.
fn session_code() -> String {
    const SYMBOLS: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    // std steps its hashers' keys for each `RandomState`, so the two draws differ.
    let draw = || RandomState::new().hash_one(());
    let bits = u128::from(draw()) << 16 | u128::from(draw() as u16);
    let mut code = String::new();
    for at in 0..16 {
        if at > 0 && at % 4 == 0 {
            code.push('-');
        }
        code.push(char::from(SYMBOLS[(bits >> (75 - 5 * at)) as usize & 31]));
    }
    code
}

/// `sandbox-42.snapshot`; `shards-42-1.snapshot` for a joiner.
fn snapshot_name(package: &str, pid: u32, peer: usize) -> String {
    match peer {
        0 => format!("{package}-{pid}.snapshot"),
        peer => format!("{package}-{pid}-{peer}.snapshot"),
    }
}

/// What `gg run` keeps across the game's restarts.
struct Supervisor<'a> {
    run: &'a Run,
    exe: PathBuf,
    peers: Vec<Peer>,
    /// Where the game writes its logs and session files.
    dir: PathBuf,
    serves: bool,
    restarts: bool,
    /// Whether a process of the game has served the API.
    served: bool,
    /// Whether a `serving` event has said why its launch was slow ([`SCANNED`]).
    scan_told: bool,
    /// The code of the last snapshot failure printed, until one succeeds.
    failed: Option<String>,
    patches: Option<Patches>,
    /// Set once gg's stdin closes, while embedded.
    closed: Option<Arc<AtomicBool>>,
    /// For gg's other commands to read while no game answers.
    told: Option<Told>,
}

/// A peer, or the game alone.
struct Peer {
    args: Vec<String>,
    copy: PathBuf,
    snapshot: PathBuf,
    latest: Option<Snapshotted>,
}

/// A game's process, and what's known of it. Killed as it drops, if it hasn't ended.
struct Game {
    /// Its stdin closes as gg ends, ending a `--supervised` game.
    child: Child,
    pid: u32,
    restored: Option<Snapshotted>,
    session: Option<Session>,
    client: Option<Client>,
    frames: u64,
    launched: Instant,
    moved: Instant,
    pinged: Instant,
    /// What frames had paused for at the last ping answered.
    waiting: Option<String>,
    /// When it was first seen loading, while it is ([`going_on`]).
    loading: Option<Instant>,
    /// When it was last asked for a snapshot, or its first frame was seen: the first snapshot
    /// comes an interval of running after the start, so a game that soon ends the same way
    /// doesn't restart from a snapshot of that end.
    snapped: Instant,
    snapshotted: bool,
    /// On a thread, so pings go on meanwhile.
    snapshotting: Option<JoinHandle<Result<Snapshotted, Failure>>>,
}

impl Supervisor<'_> {
    /// Runs the game until it exits cleanly (with restarts off, ends at all). Peers go together:
    /// as one ends, the others stop, and what [`next`] says of it is what all do.
    fn supervise(&mut self) -> Result<bool, Failure> {
        // Taken before the first copy, so a rebuild as it's made shows.
        let mut watch = Watch::new(stamp(&self.exe), Instant::now());
        // Empty while it waits for a rebuild.
        let mut games = self.start(vec![None; self.peers.len()])?;
        // Until the game restarts from the new build.
        let mut rebuilt = false;
        let mut waits = None;
        loop {
            let (now, looked) = (Instant::now(), runs::now_ms());
            // With patches on, the run makes the builds itself, and `gg rebuild` may ask for one.
            if let Some(patches) = &mut self.patches {
                let told = self.told.as_ref();
                if let Some(asked) = told.and_then(Told::asked_to_rebuild) {
                    patches.ask(asked);
                }
                let processes: Vec<Process> = games.iter().map(Game::process).collect();
                rebuilt |= patches.tend(&processes, now);
                if let (Some(told), Some((asked, failure))) = (told, patches.answered()) {
                    told.answer(asked, failure.as_ref());
                }
                // A restart in place's snapshot, the newest of the game's world.
                if let Some(taken) = patches.taken() {
                    self.peers[0].latest = Some(taken);
                    if let Some(game) = games.first_mut() {
                        game.snapshotted = true;
                    }
                }
            } else if self.restarts && watch.rebuilt(stamp(&self.exe), now) {
                event(&json!({"event": "rebuilt", "exe": self.exe}));
                rebuilt = true;
            }
            self.tell(&games, waits, looked);
            if self.asked_to_quit(&games) {
                event(&json!({"event": "quit", "why": QUIT_ASKED}));
                for peer in &self.peers {
                    let _ = fs::remove_file(&peer.snapshot);
                }
                return Ok(true);
            }
            let mut happened = None;
            for (peer, game) in games.iter_mut().enumerate() {
                if let Some(what) = self.tend(peer, game)? {
                    happened = Some((peer, what));
                    break;
                }
            }
            if (self.closed.as_ref()).is_some_and(|closed| closed.load(Ordering::Relaxed)) {
                event(&json!({"event": "closed", "why": CLOSED}));
                happened = Some((0, Happened::Closed));
            } else if happened.is_none() && rebuilt {
                let fresh = self
                    .patches
                    .as_ref()
                    .is_some_and(|patches| patches.afresh());
                happened = Some((0, Happened::Rebuilt { fresh }));
            }
            let Some((ended, happened)) = happened else {
                thread::sleep(Duration::from_millis(100));
                continue;
            };
            // A restart in place that failed ends the game, its world in the snapshot.
            if let Some(patches) = &mut self.patches {
                patches.settle();
                if let Some(taken) = patches.taken() {
                    self.peers[0].latest = Some(taken);
                }
            }
            // The rest stop, snapshotted first so they restart with the one that ended; on
            // `Closed` all stop, unsnapshotted, as they do to start afresh.
            let all = matches!(happened, Happened::Rebuilt { .. } | Happened::Closed);
            let afresh = matches!(happened, Happened::Rebuilt { fresh: true });
            let snapshot = !afresh && !matches!(happened, Happened::Closed);
            let stopping = if matches!(happened, Happened::Rebuilt { .. }) {
                "restarting"
            } else {
                "stopping"
            };
            self.told(State {
                pids: games.iter().map(|game| game.pid).collect(),
                ..State::new(stopping)
            });
            if matches!(happened, Happened::Rebuilt { .. }) {
                self.keep_warnings(&games);
            }
            for (peer, game) in games.iter_mut().enumerate() {
                if all || peer != ended {
                    self.stop(peer, game, snapshot)?;
                }
            }
            games.clear();
            let failed = matches!(happened, Happened::Failed { .. });
            let mut next = next(happened, self.peers[ended].latest.as_ref(), self.restarts);
            // Its build would restore a snapshot of layouts it lacks.
            if failed
                && matches!(next, Next::Start(_))
                && (self.patches.as_mut()).is_some_and(|patches| patches.outdated())
            {
                next = Next::Wait(patch::OUTDATED);
            }
            match next {
                Next::Start(from) => {
                    if afresh {
                        // So a later restart can't take the old world back.
                        for them in &mut self.peers {
                            them.latest = None;
                            let _ = fs::remove_file(&them.snapshot);
                        }
                    }
                    let froms = (self.peers.iter().enumerate()).map(|(peer, them)| {
                        if peer == ended {
                            from.clone()
                        } else {
                            them.latest.clone()
                        }
                    });
                    games = self.start(froms.collect())?;
                    rebuilt = false;
                    waits = None;
                }
                Next::Wait(why) => {
                    event(&json!({"event": "waiting", "for": "rebuild", "why": why}));
                    waits = Some(why);
                    if let Some(patches) = &mut self.patches {
                        patches.waiting();
                    }
                }
                Next::End(clean) => {
                    if clean {
                        for peer in &self.peers {
                            let _ = fs::remove_file(&peer.snapshot);
                        }
                    }
                    return Ok(clean);
                }
            }
        }
    }

    /// Tells what the run is doing, as of a look at the sources at `looked`: [`Patches::state`]
    /// with patches on; else starting until every process serves, then running. `games` is empty
    /// while it waits (for `waits`) for a rebuild.
    fn tell(&self, games: &[Game], waits: Option<&'static str>, looked: u64) {
        let Some(told) = &self.told else {
            return;
        };
        let state = match &self.patches {
            Some(patches) => {
                let processes: Vec<Process> = games.iter().map(Game::process).collect();
                patches.state(&processes)
            }
            None => {
                let serving = games.iter().all(|game| game.session.is_some());
                let phase = match games {
                    [] => "waiting",
                    _ if serving => "running",
                    _ => "starting",
                };
                State {
                    pids: games.iter().map(|game| game.pid).collect(),
                    ..State::new(phase)
                }
            }
        };
        let why = match (state.phase, &state.why) {
            ("waiting", None) => waits.map(str::to_owned),
            (_, why) => why.clone(),
        };
        told.tell(&State { why, ..state }, looked);
    }

    /// Has [`Patches::warned`] keep what `games` warned of, which their restart on a rebuild hides.
    fn keep_warnings(&mut self, games: &[Game]) {
        let Some(patches) = &mut self.patches else {
            return;
        };
        let mut events = Vec::new();
        for session in games.iter().filter_map(|game| game.session.as_ref()) {
            if let Ok(mut client) = Client::connect(session, "gg", PULSE) {
                events.extend(runs::read_events(&mut client, json!({"level": "warn"})));
            }
        }
        patches.warned(&events);
    }

    /// Whether `gg quit` asked the run to end ([`Told::asked_to_quit`]) as it waits for a rebuild,
    /// with no game and no build under way: one under way would hold the quit until it ends.
    fn asked_to_quit(&self, games: &[Game]) -> bool {
        let waits = games.is_empty()
            && (self.patches.as_ref()).is_none_or(|patches| patches.state(&[]).phase == "waiting");
        waits && (self.told.as_ref()).is_some_and(Told::asked_to_quit)
    }

    fn told(&self, state: State) {
        if let Some(told) = &self.told {
            told.tell(&state, runs::now_ms());
        }
    }

    /// Starts each peer from its snapshot in `froms`, or afresh. Copies the executable if it may
    /// have been rebuilt (without patches, always).
    fn start(&mut self, froms: Vec<Option<Snapshotted>>) -> Result<Vec<Game>, Failure> {
        let afresh = froms.iter().all(Option::is_none);
        let built = (self.patches.as_mut()).is_none_or(|patches| patches.starting(afresh));
        let froms = froms.into_iter().enumerate();
        froms
            .map(|(peer, from)| self.start_peer(peer, from, built))
            .collect()
    }

    /// Starts peer `peer` from `from`, or afresh: copies the executable if `built`, runs the copy,
    /// prints `started`.
    fn start_peer(
        &mut self,
        peer: usize,
        from: Option<Snapshotted>,
        built: bool,
    ) -> Result<Game, Failure> {
        let them = &self.peers[peer];
        if built {
            copy(&self.exe, &them.copy)?;
        }
        let mut command = Command::new(&them.copy);
        command
            .args(&self.run.args)
            .args(&them.args)
            .stdin(Stdio::piped())
            // gg's stdout is JSON lines alone: the game's own output goes to stderr.
            .stdout(io::stderr());
        if let Some(from) = &from {
            command.arg("--restore").arg(&from.path);
        }
        let child = command.spawn().map_err(|error| failed(&them.copy, error))?;
        let pid = child.id();
        let restored =
            (from.as_ref()).map(|from| json!({"snapshot": from.path, "ticks": from.ticks}));
        let mut started = json!({
            "event": "started",
            "pid": pid,
            "exe": self.exe,
            "copy": them.copy,
            "restored": restored,
        });
        if self.peers.len() > 1 {
            started["player"] = json!(peer);
        }
        event(&started);
        let now = Instant::now();
        Ok(Game {
            child,
            pid,
            restored: from,
            session: None,
            client: None,
            frames: 0,
            launched: now,
            moved: now,
            pinged: now,
            waiting: None,
            loading: None,
            snapped: now,
            snapshotted: false,
            snapshotting: None,
        })
    }

    /// Looks after `game` for a turn of the loop: finds its session once it serves (warning if its
    /// API revision isn't gg's), snapshots it, pings it. Returns what happened, printed, if it
    /// ended, or hung and was killed.
    fn tend(&mut self, peer: usize, game: &mut Game) -> Result<Option<Happened>, Failure> {
        let ended = game.child.try_wait();
        if let Some(status) = ended.map_err(|error| failed(&self.peers[peer].copy, error))? {
            return Ok(Some(self.ended(peer, game, status)));
        }
        // A game started afresh refuses one until its first tick, as one started paused may wait
        // long for it: the next asks again, quietly. A restart's refused snapshot is still told.
        if let Some(taking) = &game.snapshotting
            && taking.is_finished()
            && let Some(taken) = game.landed()
            && !matches!(&taken, Err(failure) if failure.code == "api.not_ticked")
        {
            self.note(peer, game, taken);
        }
        if self.serves && game.session.is_none() {
            let pid = game.pid;
            let mut found = Session::list(&Session::dir(&self.dir)).into_iter();
            let found = found.find(|(_, session)| session.pid == pid);
            if let Some((file, session)) = found {
                let ms = millis(game.launched.elapsed());
                let mut serving = json!({
                    "event": "serving",
                    "pid": pid,
                    "app": session.app,
                    "address": session.address,
                    "session": file,
                    "project": session.project,
                    "log": session.log,
                    "ms": ms,
                });
                if cfg!(target_os = "macos") && ms >= SCANNED && !self.scan_told {
                    self.scan_told = true;
                    serving["hint"] = json!(SCAN_HINT);
                }
                event(&serving);
                game::warn_protocol(&file, &session);
                game.session = Some(session);
                self.served = true;
            }
        }
        let Some(session) = &game.session else {
            return Ok(None);
        };
        if self.restarts
            && game.frames > 0
            && game.snapshotting.is_none()
            // A load's snapshot waits for it: a game streaming as it plays may load often.
            && (game.waiting.as_deref()).is_none_or(|waiting| waiting == LOADING)
            && game.snapped.elapsed() >= self.run.snapshot
        {
            game.snapped = Instant::now();
            let (session, path) = (session.clone(), self.peers[peer].snapshot.clone());
            game.snapshotting = Some(thread::spawn(move || snapshot(&session, &path, false)));
        }
        if game.pinged.elapsed() < PING {
            return Ok(None);
        }
        game.pinged = Instant::now();
        // A ping that fails counts as frames that stopped.
        if let Some(pulse) = ping(&mut game.client, session) {
            let most = self.run.hang.map_or(STARTUP, |hang| STARTUP.max(hang));
            if going_on(&pulse, game.frames, &mut game.loading, most, Instant::now()) {
                if game.frames == 0 {
                    game.snapped = Instant::now();
                }
                game.frames = pulse.frames;
                game.moved = Instant::now();
            }
            game.waiting = pulse.waiting;
        }
        // `--hang 0` turns off hangs, a run that never runs a frame included.
        let Some(hang) = self.run.hang else {
            return Ok(None);
        };
        let limit = hang_limit(game.frames, hang);
        if game.moved.elapsed() <= limit {
            return Ok(None);
        }
        let stacks = self.exe.with_file_name(format!("hung-{}.txt", game.pid));
        let stacks = sample_stacks(game.pid, &stacks).then_some(stacks);
        let _ = game.child.kill();
        let _ = game.child.wait();
        let (log, errors) = self.remains(peer, game);
        event(&json!({
            "event": "hung",
            "pid": game.pid,
            "frames": game.frames,
            "seconds": limit.as_secs(),
            "log": log,
            "errors": errors,
            "stacks": stacks,
        }));
        Ok(Some(game.failed(self.served)))
    }

    /// What happened as the process ended with `status`, printed: exited, crashed, or (from a
    /// snapshot) refused it with a `snapshot.` error. A game that came to its own end logs
    /// `engine.perf` and counts as exited whatever its status (1 if validation reported anything).
    fn ended(&mut self, peer: usize, game: &mut Game, status: ExitStatus) -> Happened {
        let (log, errors) = self.remains(peer, game);
        let refusal = |error: &Value| {
            (error["code"].as_str()).is_some_and(|code| code.starts_with("snapshot."))
        };
        if let Some(restored) = &game.restored
            && !status.success()
            && errors.iter().any(refusal)
        {
            event(&json!({
                "event": "refused",
                "pid": game.pid,
                "snapshot": restored.path,
                "log": log,
                "errors": errors,
            }));
            return Happened::Refused;
        }
        let own = status.success() || log.as_deref().is_some_and(ended_itself);
        event(&json!({
            "event": if own { "exited" } else { "crashed" },
            "pid": game.pid,
            "status": status.code(),
            "log": log,
            "errors": errors,
        }));
        if own {
            Happened::Exited {
                clean: status.success(),
            }
        } else {
            game.failed(self.served)
        }
    }

    /// What `game` left: its log and errors; removes its session file. A snapshot in flight counts
    /// if written first; if it failed, the end is why.
    fn remains(&mut self, peer: usize, game: &mut Game) -> (Option<PathBuf>, Vec<Value>) {
        if let Some(Ok(taken)) = game.landed() {
            self.note(peer, game, Ok(taken));
        }
        let log = log(&self.dir, game.pid, game.session.as_ref());
        let errors = log.as_deref().map(errors).unwrap_or_default();
        self.forget(game.pid);
        (log, errors)
    }

    /// Removes the ended process `pid`'s session files. One killed or aborted leaves its own, and
    /// a later process given its PID would be mistaken for it.
    fn forget(&self, pid: u32) {
        for (path, session) in Session::list(&Session::dir(&self.dir)) {
            if session.pid == pid {
                let _ = fs::remove_file(path);
            }
        }
    }

    /// Stops `game`. If it serves, asks it to end once a snapshot in flight is done, snapshotted
    /// first if `snapshot` ([`end`]), and kills it if it hasn't ended within [`QUIT`]; if not yet,
    /// kills it. One [`holding`] before it's asked or while the snapshots wait is killed at once,
    /// printing `killed` with the snapshot it restarts from, if any; what changed since is lost.
    fn stop(&mut self, peer: usize, game: &mut Game, snapshot: bool) -> Result<(), Failure> {
        let mut asked = None;
        match game.session.clone() {
            Some(session) => match self.ask_to_end(peer, game, &session, snapshot) {
                Ok(ended) => asked = ended,
                Err(waiting) => {
                    let _ = game.child.kill();
                    // Its connection closes as it ends, which ends the snapshot in flight too.
                    if let Some(Ok(taken)) = game.landed() {
                        self.note(peer, game, Ok(taken));
                    }
                    event(&killed(
                        game.pid,
                        &waiting,
                        self.peers[peer].latest.as_ref(),
                    ));
                }
            },
            None => {
                let _ = game.child.kill();
            }
        }
        let waited = wait(&mut game.child, QUIT);
        waited.map_err(|error| failed(&self.peers[peer].copy, error))?;
        // A snapshot whose answer didn't come or came too late may have been written before the
        // game ended.
        if let Some((taken, sent)) = asked {
            let path = &self.peers[peer].snapshot;
            let taken = taken.or_else(|failure| {
                let unanswered = matches!(failure.code.as_str(), "api.closed" | "api.timeout");
                (unanswered.then(|| written_since(path, sent)).flatten()).ok_or(failure)
            });
            self.note(peer, game, taken);
        }
        self.forget(game.pid);
        Ok(())
    }

    /// Asks `game` to end once the snapshot in flight is done, snapshotted first if `snapshot`
    /// ([`end`]). Returns what that snapshot gave, if asked, with when; the caller notes it once
    /// the game has ended.
    ///
    /// # Errors
    ///
    /// - What the game waits for, if [`holding`] before it's asked or while either snapshot waits.
    fn ask_to_end(
        &mut self,
        peer: usize,
        game: &mut Game,
        session: &Session,
        snapshot: bool,
    ) -> Result<Option<SnapshotAsked>, String> {
        if let Some(waiting) = holding(&mut game.client, session) {
            return Err(waiting);
        }
        if let Some(taking) = game.snapshotting.take() {
            let taken =
                awaiting(taking, &mut game.client, session).map_err(|(waiting, taking)| {
                    // Joined once the game's killed, which ends it.
                    game.snapshotting = Some(taking);
                    waiting
                })?;
            if let Some(taken) = taken {
                self.note(peer, game, taken);
            }
        }
        let path = snapshot.then(|| self.peers[peer].snapshot.clone());
        let (ending, sent) = (session.clone(), SystemTime::now());
        let ended = thread::spawn(move || end(&ending, path.as_deref()));
        let ended = awaiting(ended, &mut game.client, session).map_err(|(waiting, _)| waiting)?;
        Ok(ended.flatten().map(|taken| (taken, sent)))
    }

    /// Notes a snapshot, which restarts start from, or its failure, printed unless the last had
    /// its code.
    fn note(&mut self, peer: usize, game: &mut Game, taken: Result<Snapshotted, Failure>) {
        match taken {
            Ok(taken) => {
                game.snapshotted = true;
                self.peers[peer].latest = Some(taken);
                self.failed = None;
            }
            Err(failure) => {
                if self.failed.as_ref() != Some(&failure.code) {
                    let error = json!({"code": failure.code, "message": failure.message});
                    event(&json!({"event": "snapshot_failed", "pid": game.pid, "error": error}));
                    self.failed = Some(failure.code);
                }
            }
        }
    }
}

impl Game {
    fn process(&self) -> Process<'_> {
        Process {
            pid: self.pid,
            session: self.session.as_ref(),
        }
    }

    /// The snapshot in flight's result, waiting for it; `None` if none, or its thread panicked.
    fn landed(&mut self) -> Option<Result<Snapshotted, Failure>> {
        self.snapshotting.take()?.join().ok()
    }

    fn failed(&self, served: bool) -> Happened {
        Happened::Failed {
            snapshotted: self.snapshotted,
            restored: self.restored.is_some(),
            served,
        }
    }
}

impl Drop for Game {
    /// Kills its process, so a run that fails while games run (the joiner can't start after the
    /// host has) leaves none behind. A reaped one is left alone: std signals none.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What happened to a game; [`next`] says what follows.
enum Happened {
    /// It came to its own end (window closed, `quit` asked, frames ran out). `clean` if it exited
    /// cleanly, not with what validation reported.
    Exited { clean: bool },
    /// It crashed or hung. `snapshotted` if a snapshot was taken, `restored` if it started from
    /// one, `served` if a process of the run has served the API.
    Failed {
        snapshotted: bool,
        restored: bool,
        served: bool,
    },
    /// It refused the snapshot it started from.
    Refused,
    /// Its executable was rebuilt, and its process, if any, stopped; `fresh` if
    /// `gg rebuild --fresh` asked to start it afresh, so it stopped unsnapshotted.
    Rebuilt { fresh: bool },
    /// gg's stdin closed (the embedding tool ended), and its processes stopped.
    Closed,
}

/// A snapshot asked of a game as it ended, and when: tells a file written before the end from an
/// older one, should the answer not have come.
type SnapshotAsked = (Result<Snapshotted, Failure>, SystemTime);

/// What `gg run` does next.
#[derive(Debug, PartialEq)]
enum Next {
    /// Restarts from the snapshot, or afresh.
    Start(Option<Snapshotted>),
    /// Waits for a rebuild, for the reason given.
    Wait(&'static str),
    /// Ends, returning whether the game exited cleanly.
    End(bool),
}

/// What `gg run` does after `happened`, given the last successful snapshot, `latest`.
///
/// - A crash or hang restarts from the latest snapshot if one was taken (an interval of running
///   past the start). Else it would likely end the same way, so it waits for a rebuild.
/// - A game none of whose processes has served the API can't be snapshotted, nor told from one
///   that never will (given an argument it doesn't take, which no rebuild changes): the run ends.
/// - A rebuild restarts from the latest snapshot, the one taken as it stopped if that succeeded;
///   one `gg rebuild --fresh` asked for starts afresh.
/// - A refused snapshot starts afresh.
fn next(happened: Happened, latest: Option<&Snapshotted>, restarts: bool) -> Next {
    match happened {
        Happened::Exited { clean } => Next::End(clean),
        Happened::Closed => Next::End(true),
        Happened::Failed { served: false, .. } => Next::End(false),
        Happened::Failed { .. } if !restarts => Next::End(false),
        Happened::Failed { snapshotted, .. } if snapshotted => Next::Start(latest.cloned()),
        Happened::Failed { restored, .. } if restored => {
            Next::Wait("restored from a snapshot, it ended before taking another")
        }
        Happened::Failed { .. } => Next::Wait("it ended before its first snapshot"),
        Happened::Rebuilt { fresh: true } => Next::Start(None),
        Happened::Rebuilt { fresh: false } => Next::Start(latest.cloned()),
        Happened::Refused => Next::Start(None),
    }
}

/// A file's modified time and length; `None` while unreadable, as while cargo replaces it.
pub(crate) type Stamp = Option<(SystemTime, u64)>;

fn stamp(exe: &Path) -> Stamp {
    fs::metadata(exe).ok().and_then(stamped)
}

pub(crate) fn stamped(metadata: fs::Metadata) -> Stamp {
    Some((metadata.modified().ok()?, metadata.len()))
}

/// Tells when an executable was rebuilt: once its stamp differs from the build's and has stayed
/// the same for [`SETTLE`].
struct Watch {
    /// The build last told of, or the first.
    built: Stamp,
    /// The stamp last seen, and when it was first seen.
    seen: Stamp,
    since: Instant,
}

impl Watch {
    fn new(built: Stamp, now: Instant) -> Self {
        Self {
            built,
            seen: built,
            since: now,
        }
    }

    /// Takes the executable's `stamp` at `now`; returns whether it was rebuilt since it last said
    /// so. An unreadable one (removed) isn't a build.
    fn rebuilt(&mut self, stamp: Stamp, now: Instant) -> bool {
        if stamp != self.seen {
            (self.seen, self.since) = (stamp, now);
        }
        let settled = now.duration_since(self.since) >= SETTLE;
        let rebuilt = stamp.is_some() && stamp != self.built && settled;
        if rebuilt {
            self.built = stamp;
        }
        rebuilt
    }
}

/// Where process `pid`'s `gg run` copies `exe` to run peer `peer` (the game alone is 0). Beside
/// `exe`, so what the game writes there stays put; named for the process and peer, so each has
/// its own: `gg-sandbox-running-42.exe`, `gg-shards-running-42-1.exe` for a joiner.
fn running_copy(exe: &Path, pid: u32, peer: usize) -> PathBuf {
    let mut name = exe.file_stem().unwrap_or_default().to_owned();
    name.push(format!("-running-{pid}"));
    if peer > 0 {
        name.push(format!("-{peer}"));
    }
    if let Some(extension) = exe.extension() {
        name.push(".");
        name.push(extension);
    }
    exe.with_file_name(name)
}

/// Removes the copies of `exe` that other processes' runs left (a killed run). One a game runs
/// from stays on Windows; elsewhere its run copies it again first.
fn remove_leftovers(exe: &Path, pid: u32) {
    let Some(dir) = exe.parent() else {
        return;
    };
    let stem = exe.file_stem().unwrap_or_default().to_string_lossy();
    let prefix = format!("{stem}-running-");
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|name| name.strip_prefix(&prefix)) else {
            continue;
        };
        let theirs = rest
            .split(['.', '-'])
            .next()
            .and_then(|pid| pid.parse().ok());
        if let Some(theirs) = theirs.filter(|&theirs| theirs != pid)
            && (0..2).any(|peer| running_copy(exe, theirs, peer) == entry.path())
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Removes the snapshots in `dir` that other processes' `gg run`s of `package` left (a killed run,
/// or a game that crashed for good). None is another's to start from: no other run holds the lock.
fn remove_snapshots_left(dir: &Path, package: &str, pid: u32) {
    let prefix = format!("{package}-");
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|name| name.strip_prefix(&prefix)) else {
            continue;
        };
        let theirs = rest
            .split(['.', '-'])
            .next()
            .and_then(|pid| pid.parse().ok());
        if let Some(theirs) = theirs.filter(|&theirs| theirs != pid)
            && (0..2).any(|peer| dir.join(snapshot_name(package, theirs, peer)) == entry.path())
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Copies executable `exe` to `to`, trying again for [`COPYING`] while it fails.
///
/// # Errors
///
/// - `gg.run` if it still can't
fn copy(exe: &Path, to: &Path) -> Result<(), Failure> {
    // Removed first, so the copy is a new file: macOS keeps the code signature it checked for a
    // file that ran, and kills what runs from it once it's written over
    // (https://golang.org/issue/54132).
    retrying(|| remove(to).and_then(|()| fs::copy(exe, to).map(drop))).map_err(|error| {
        let message = format!("can't copy {} to {}: {error}", exe.display(), to.display());
        Failure::new("gg.run", message)
    })
}

/// Removes the file at `path`, if there's one.
pub(crate) fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

/// Does `what` until it succeeds, for at most [`COPYING`].
pub(crate) fn retrying(mut what: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let start = Instant::now();
    loop {
        match what() {
            Err(_) if start.elapsed() < COPYING => thread::sleep(Duration::from_millis(100)),
            done => return done,
        }
    }
}

/// Has the game write a snapshot at `path`, through a connection that waits [`SNAPSHOT`]; ends
/// the game once it's written, if `quit`.
fn snapshot(session: &Session, path: &Path, quit: bool) -> Result<Snapshotted, Failure> {
    let path = Some(path.to_owned());
    Client::connect(session, "gg", SNAPSHOT)?.call_as("snapshot", Snapshot { path, quit })
}

/// Asks the game to end, snapshotted at `path` first if given, as one request: one in between
/// would apply and be lost as the game restarts from the snapshot. Returns the snapshot's result,
/// if asked. A snapshot the game didn't take (before its first tick) leaves it running, so it's
/// then asked to quit alone.
fn end(session: &Session, path: Option<&Path>) -> Option<Result<Snapshotted, Failure>> {
    let taken = path.map(|path| snapshot(session, path, true));
    if !matches!(taken, Some(Ok(_)))
        && let Ok(mut client) = Client::connect(session, "gg", PULSE)
    {
        let _ = client.call("quit", Value::Null);
    }
    taken
}

/// What `taking`, a thread asking the game for something, gives, or `None` if it panicked; pings
/// the game through `client` every [`PING`] meanwhile.
///
/// # Errors
///
/// - What the game waits for, with the thread, if it comes to be [`holding`]: the thread would
///   only wait it out.
fn awaiting<T>(
    taking: JoinHandle<T>,
    client: &mut Option<Client>,
    session: &Session,
) -> Result<Option<T>, (String, JoinHandle<T>)> {
    let mut pinged = Instant::now();
    while !taking.is_finished() {
        if pinged.elapsed() >= PING {
            pinged = Instant::now();
            if let Some(waiting) = holding(client, session) {
                return Err((waiting, taking));
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(taking.join().ok())
}

/// The `killed` event of process `pid`, killed as it waited for `waiting`. It restarts from
/// `latest`, else afresh.
fn killed(pid: u32, waiting: &str, latest: Option<&Snapshotted>) -> Value {
    let from = latest.map(|latest| json!({"snapshot": latest.path, "ticks": latest.ticks}));
    let lost = match latest {
        Some(_) => "what changed in it after its last snapshot, `from`, is lost",
        None => "it has no snapshot, so what changed in it is lost, and it starts afresh",
    };
    json!({
        "event": "killed",
        "pid": pid,
        "waiting": waiting,
        "from": from,
        "why": format!(
            "it waits where it can't answer a snapshot or a quit, which would only wait out their \
             time; {lost}"
        ),
    })
}

/// The snapshot at `path`, if written whole since `since`, as by a game that ended unanswering.
pub(crate) fn written_since(path: &Path, since: SystemTime) -> Option<Snapshotted> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.modified().ok()? < since {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if !lines.next()?.starts_with("gg-snapshot ") || text.lines().last() != Some("end") {
        return None;
    }
    let ticks = text.lines().find_map(|line| line.strip_prefix("ticks "))?;
    // The world's rows, before what follows them.
    let ends = ["stopped", "paused", "end"];
    let world = text
        .lines()
        .take_while(|line| !ends.contains(line) && !line.starts_with("edited "));
    Some(Snapshotted {
        path: path.to_owned(),
        ticks: ticks.parse().ok()?,
        entities: world.filter(|line| line.starts_with("row ")).count(),
        bytes: metadata.len(),
    })
}

/// What the game waits for, if it holds its main thread, so only its connections' threads answer:
/// a breakpoint, a file dialog, or the window menu Windows runs. `None` for anything else, as a
/// minimized window, which answers between frames.
fn holding(client: &mut Option<Client>, session: &Session) -> Option<String> {
    let waiting = ping(client, session)?.waiting?;
    ["breakpoint", "dialog", "menu"]
        .contains(&waiting.as_str())
        .then_some(waiting)
}

/// Whether `pulse`, answered at `now`, shows a game that had run `frames` frames going on: running
/// more, waiting for the user, or loading for at most `most` since `loading`, when it was first
/// seen loading, which this keeps. A load isn't the user's doing: one that outlasts `most` is
/// stopped frames, and so a hang once they outlast [`hang_limit`]. Each pulse while it loads
/// counts as a move, so the frame that follows has the whole limit.
fn going_on(
    pulse: &Pulse,
    frames: u64,
    loading: &mut Option<Instant>,
    most: Duration,
    now: Instant,
) -> bool {
    let loads = pulse.waiting.as_deref() == Some(LOADING);
    *loading = loads.then(|| loading.unwrap_or(now));
    let waits = pulse.waiting.is_some() && !loads;
    let within = loading.is_some_and(|since| now.duration_since(since) <= most);
    pulse.frames != frames || waits || within
}

/// How long frames may stop before a game is hung, given `hang`: [`STARTUP`], if longer, before its
/// first frame (`frames` 0).
fn hang_limit(frames: u64, hang: Duration) -> Duration {
    if frames == 0 { STARTUP.max(hang) } else { hang }
}

/// The game's pulse through `client`, reconnected once it breaks; `None` if no answer in time.
fn ping(client: &mut Option<Client>, session: &Session) -> Option<Pulse> {
    if client.is_none() {
        *client = Client::connect(session, "gg", PULSE).ok();
    }
    let pulse = client.as_mut()?.call_as("ping", Value::Null);
    if pulse.is_err() {
        *client = None;
    }
    pulse.ok()
}

/// Process `pid`'s JSON log: its session's, if it served the API, else the one named for it.
fn log(dir: &Path, pid: u32, session: Option<&Session>) -> Option<PathBuf> {
    let named = session.and_then(|session| session.log.clone());
    named.or_else(|| game::log_of(&dir.join("logs"), pid))
}

/// Whether the run came to its own end: it logged `engine.perf`, which a crash, hang or refusal
/// doesn't.
fn ended_itself(log: &Path) -> bool {
    let mut events = game::events(log).into_iter();
    events.any(|event| event.get("code").and_then(Value::as_str) == Some("engine.perf"))
}

fn errors(log: &Path) -> Vec<Value> {
    let events = game::events(log).into_iter();
    let errors = events.filter(|event| event.get("level").and_then(Value::as_str) == Some("ERROR"));
    errors.map(game::trimmed).collect()
}

fn failed(exe: &Path, error: std::io::Error) -> Failure {
    Failure::new("gg.run", format!("can't run {}: {error}", exe.display()))
}

/// Builds `check`'s package, runs it headless for its frames without the tool API, and prints a
/// line of JSON: exit status, warnings, errors and `engine.perf`'s frame times. Returns whether it
/// passed: exited cleanly, in time, with no error. [`warn_settings`] warns first, without failing.
///
/// # Errors
///
/// Fails as [`check_game`] and [`warn_settings`] do, and with:
/// - `gg.build` if it doesn't build
/// - `gg.run` if it can't start or be waited for
pub(crate) fn check(check: &Check) -> Result<bool, Failure> {
    check_game(&check.package, check.project.as_ref())?;
    warn_settings(&check.package, check.project.as_ref())?;
    let exe = build::build(
        &check.package,
        release(check.release),
        check.project.as_ref().map(Scope::dir),
    )?
    .exe;
    let frames = check.frames.to_string();
    // Its console repeats its JSON log, read below. Its stdin closes as gg ends, ending it.
    let mut child = Command::new(&exe)
        .args([
            "--headless",
            "--frames",
            &frames,
            "--no-api",
            "--supervised",
        ])
        .args(&check.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| failed(&exe, error))?;
    let pid = child.id();
    let status = wait(&mut child, check.timeout).map_err(|error| failed(&exe, error))?;
    let dir = exe.parent().unwrap_or(Path::new("."));
    let log = game::log_of(&dir.join("logs"), pid);
    let events = log.as_deref().map(game::events).unwrap_or_default();
    let (mut warnings, mut errors, mut perf) = (Vec::new(), Vec::new(), Value::Null);
    for event in events {
        match event.get("level").and_then(Value::as_str) {
            Some("ERROR") => errors.push(game::trimmed(event)),
            Some("WARN") => warnings.push(game::trimmed(event)),
            _ if event.get("code").and_then(Value::as_str) == Some("engine.perf") => {
                perf = game::trimmed(event);
            }
            _ => {}
        }
    }
    let code = status.and_then(|status| status.code());
    let passed = code == Some(0) && errors.is_empty();
    let checked = json!({
        "package": check.package,
        "exe": exe,
        "passed": passed,
        "status": code,
        "timed_out": status.is_none(),
        "log": log,
        "perf": perf,
        "warnings": warnings,
        "errors": errors,
    });
    event(&checked);
    Ok(passed)
}

/// Warns `gg.settings`, on stderr, of each setting of `package` that isn't the engine's it builds
/// against ([`settings::drifts`]). Finds clippy.toml as clippy does (`CLIPPY_CONF_DIR`, else the
/// package's directory) and rust-toolchain.toml as rustup does (the directory cargo runs in). A
/// package against no engine has nothing to warn of.
///
/// # Errors
///
/// Fails as [`build::package`] does, and with:
/// - `gg.run` if gg can't tell its working directory
fn warn_settings(package: &str, project: Option<&Scope>) -> Result<(), Failure> {
    let dir = project.map(Scope::dir);
    let package = build::package(package, dir)?;
    let Some(engine) = package.engine else {
        return Ok(());
    };
    let crate_dir = (package.manifest.parent()).expect("cargo names a manifest by its file's path");
    let clippy_from =
        std::env::var_os("CLIPPY_CONF_DIR").map_or(crate_dir.to_owned(), PathBuf::from);
    let cargo_dir = match dir {
        Some(dir) => dir.to_owned(),
        None => std::env::current_dir().map_err(|error| {
            Failure::new(
                "gg.run",
                format!("can't tell gg's working directory: {error}"),
            )
        })?,
    };
    for drift in settings::drifts(&clippy_from, &cargo_dir, &engine) {
        crate::warn("gg.settings", &drift.message, &drift.path);
    }
    Ok(())
}

fn release(release: bool) -> &'static [&'static str] {
    if release { &["--release"] } else { &[] }
}

/// The manifest of the project whose game `run` runs, which its state names for gg's other
/// commands to find: the scope's, if its game; else the one beside the package's own manifest, if
/// that's its game's, as for `gg run showcase` from the workspace's root.
fn project_of(run: &Run) -> Option<PathBuf> {
    if let Some(scope) = &run.project
        && scope.game == run.package
    {
        return Some(scope.manifest.clone());
    }
    let package = build::package(&run.package, run.dir()).ok()?;
    let dir = package.manifest.parent()?;
    let project = gg_api::Project::read(dir).ok()?;
    (project.game == run.package).then(|| dir.join(gg_api::Project::FILE))
}

/// Checks that `package`, if it's `project`'s game, is the package beside the project's manifest.
pub(crate) fn check_game(package: &str, project: Option<&Scope>) -> Result<(), Failure> {
    match project {
        Some(scope) if scope.game == package => build::check_project(scope),
        _ => Ok(()),
    }
}

/// Waits for `child` to end for at most `timeout`, past which it's killed and `None` returned.
fn wait(child: &mut Child, timeout: Duration) -> std::io::Result<Option<ExitStatus>> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if start.elapsed() > timeout {
            child.kill()?;
            child.wait()?;
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// The longest a hung game's stacks take to sample before it's killed anyway.
const SAMPLING: Duration = Duration::from_secs(10);

/// Writes where each of process `pid`'s threads is to `path`, as the platform's sampler sees it:
/// `sample` on macOS, `gdb` on Linux where it's installed and may attach. Returns whether it did.
/// A hang's stacks say what it waits on, which its log can't.
fn sample_stacks(pid: u32, path: &Path) -> bool {
    let pid = pid.to_string();
    let mut command = if cfg!(target_os = "macos") {
        let mut sample = Command::new("/usr/bin/sample");
        sample
            .args([pid.as_str(), "1", "-mayDie", "-file"])
            .arg(path);
        sample.stdout(Stdio::null());
        sample
    } else if cfg!(target_os = "linux") {
        let Ok(file) = fs::File::create(path) else {
            return false;
        };
        let mut gdb = Command::new("gdb");
        gdb.args(["-p", &pid, "-batch", "-ex", "thread apply all bt"]);
        gdb.stdout(file);
        gdb
    } else {
        return false;
    };
    let Ok(mut sampler) = command.stdin(Stdio::null()).stderr(Stdio::null()).spawn() else {
        let _ = fs::remove_file(path);
        return false;
    };
    let started = Instant::now();
    let done = loop {
        match sampler.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if started.elapsed() < SAMPLING => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = sampler.kill();
                let _ = sampler.wait();
                break false;
            }
        }
    };
    let written = done && fs::metadata(path).is_ok_and(|file| file.len() > 0);
    if !written {
        let _ = fs::remove_file(path);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str], snapshot: u64) -> Run {
        Run {
            package: "sandbox".to_owned(),
            project: None,
            release: false,
            patches: true,
            hang: Some(Duration::from_secs(10)),
            snapshot: Duration::from_secs(snapshot),
            peers: 1,
            embedded: false,
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
        }
    }

    /// A run names the project whose game it runs, for gg's other commands run there to find:
    /// given by package from outside, as the workspace's root, it's the one beside the package's
    /// manifest. A package that's no project's game, as the sandbox, names none.
    #[test]
    fn a_run_names_its_games_project_wherever_it_starts() {
        let showcase = Run {
            package: "showcase".to_owned(),
            ..run(&[], 5)
        };
        let manifest = project_of(&showcase).expect("the showcase's project");
        assert!(
            manifest.ends_with("games/showcase/project.ron"),
            "{manifest:?}"
        );
        let inside = Run {
            project: Some(Scope {
                manifest: manifest.clone(),
                game: "showcase".to_owned(),
            }),
            ..showcase
        };
        assert_eq!(project_of(&inside), Some(manifest));
        assert_eq!(project_of(&run(&[], 5)), None);
    }

    #[test]
    fn two_peers_are_given_their_session_s_code() {
        assert_eq!(peers_args(1).expect("a game's"), [Vec::<String>::new()]);
        let [host, join] = <[_; 2]>::try_from(peers_args(2).expect("the peers'")).expect("two");
        assert_eq!((host[0].as_str(), join[0].as_str()), ("--host", "--join"));
        assert_eq!((&host[1..], host[2].as_str()), (&join[1..], "--code"));
        let code = &host[3];
        let groups: Vec<&str> = code.split('-').collect();
        assert!(groups.iter().all(|group| group.len() == 4), "{code}");
        let symbols = |symbol: char| symbol.is_ascii_digit() || symbol.is_ascii_uppercase();
        assert!(
            groups.len() == 4 && groups.concat().chars().all(symbols),
            "{code}"
        );
        assert!(!code.contains(['I', 'L', 'O', 'U']), "{code}");
        assert_ne!(session_code(), session_code());
    }

    /// A game goes on while its frames count or it waits for the user, and while it loads, until
    /// the load outlasts its most: then its stopped frames count toward a hang. A pulse after a
    /// load, frames not yet counted on, finds it went on a moment ago. Before its first frame it
    /// has [`STARTUP`], or `--hang`'s if longer.
    #[test]
    fn a_game_goes_on_while_it_loads_for_as_long_as_a_start() {
        let pulse = |frames, waiting: Option<&str>| Pulse {
            frames,
            waiting: waiting.map(str::to_owned),
        };
        let (start, most) = (Instant::now(), STARTUP);
        let at = |seconds| start + Duration::from_secs(seconds);
        let mut loading = None;
        let mut going = |frames, waiting, seconds| {
            going_on(&pulse(frames, waiting), 10, &mut loading, most, at(seconds))
        };
        assert!(going(11, None, 0));
        assert!(!going(10, None, 1));
        assert!(going(10, Some("dialog"), 2));
        assert!(going(10, Some("breakpoint"), 3));
        assert!(going(10, Some(LOADING), 4));
        assert!(going(10, Some(LOADING), 4 + 120));
        assert!(
            !going(10, Some(LOADING), 4 + 121),
            "the load outlasted its most"
        );
        assert!(!going(10, None, 130));
        assert!(going(10, Some(LOADING), 131), "a new load");
        assert!(
            !going(10, None, 132),
            "the frame after it hasn't counted yet"
        );
        assert_eq!(loading, None);
        let hang = Duration::from_secs(10);
        assert_eq!(hang_limit(10, hang), hang);
        assert_eq!(hang_limit(0, hang), STARTUP);
        assert_eq!(
            hang_limit(0, Duration::from_secs(300)),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn restarts_are_off_for_runs_without_snapshots_or_the_api_or_that_record() {
        assert!(run(&["--vsync", "--recorded"], 5).restarts());
        assert!(!run(&[], 0).restarts());
        for args in [
            &["--record", "run.recording"][..],
            &["--record=run.recording"],
            &["--replay", "run.recording"],
            &["--no-api"],
        ] {
            assert!(!run(args, 5).restarts(), "{args:?}");
        }
        assert!(!run(&["--replay=run.recording"], 5).serves());
    }

    #[test]
    fn patches_are_off_for_release_builds_and_runs_without_restarts() {
        assert_eq!(run(&["--vsync"], 5).unpatched(), None);
        let unpatched = [
            Run {
                patches: false,
                ..run(&[], 5)
            },
            Run {
                release: true,
                ..run(&[], 5)
            },
            run(&[], 0),
            run(&["--no-api"], 5),
            run(&["--record", "run.recording"], 5),
        ];
        for run in unpatched {
            assert!(run.unpatched().is_some(), "{:?}", run.args);
        }
    }

    #[test]
    fn the_copy_that_runs_is_beside_the_executable() {
        let copy = running_copy(Path::new("target/debug/gg-sandbox.exe"), 42, 0);
        assert_eq!(copy, Path::new("target/debug/gg-sandbox-running-42.exe"));
        let copy = running_copy(Path::new("target/debug/gg-sandbox"), 42, 0);
        assert_eq!(copy, Path::new("target/debug/gg-sandbox-running-42"));
        let copy = running_copy(Path::new("target/debug/gg-shards.exe"), 42, 1);
        assert_eq!(copy, Path::new("target/debug/gg-shards-running-42-1.exe"));
        assert_eq!(snapshot_name("shards", 42, 1), "shards-42-1.snapshot");
    }

    #[test]
    fn a_game_dropped_kills_its_process() {
        // Runs for a minute whatever its stdin; closing stdin alone wouldn't end it.
        #[cfg(windows)]
        let mut command = Command::new("ping");
        #[cfg(windows)]
        command.args(["-n", "60", "127.0.0.1"]);
        #[cfg(not(windows))]
        let mut command = Command::new("sleep");
        #[cfg(not(windows))]
        command.arg("60");
        let command = command.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = command.spawn().expect("it starts");
        let mut output = child.stdout.take().expect("piped");
        let (pid, now) = (child.id(), Instant::now());
        drop(Game {
            child,
            pid,
            restored: None,
            session: None,
            client: None,
            frames: 0,
            launched: now,
            moved: now,
            pinged: now,
            waiting: None,
            loading: None,
            snapped: now,
            snapshotted: false,
            snapshotting: None,
        });
        let _ = io::copy(&mut output, &mut io::sink());
        assert!(now.elapsed() < Duration::from_secs(30), "it ran on");
    }

    #[test]
    fn a_run_that_logged_its_frame_times_ended_itself() {
        let dir = std::env::temp_dir().join(format!("gg-ended-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("a directory");
        let (ended, crashed) = (dir.join("ended.jsonl"), dir.join("crashed.jsonl"));
        let error = r#"{"level":"ERROR","code":"engine.validation","fields":{}}"#;
        let perf = r#"{"level":"INFO","code":"engine.perf","fields":{}}"#;
        fs::write(&ended, format!("{perf}\n{error}\n")).expect("written");
        fs::write(&crashed, format!("{error}\n")).expect("written");
        let told = [ended_itself(&ended), ended_itself(&crashed)];
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(told, [true, false]);
    }

    #[test]
    fn a_run_removes_the_copies_others_left() {
        let dir = std::env::temp_dir().join(format!("gg-leftovers-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("a directory");
        let exe = dir.join("gg-sandbox.exe");
        let names = [
            "gg-sandbox-running-7.exe",
            "gg-sandbox-running-7-1.exe",
            "gg-sandbox-running-42.exe",
            "gg-sandbox-running-42-1.exe",
            "gg-editor-running-7.exe",
            "gg-sandbox-running-7.exe.bak",
            "gg-sandbox.exe",
        ];
        for name in names {
            fs::write(dir.join(name), b"").expect("written");
        }
        remove_leftovers(&exe, 42);
        let left: Vec<bool> = names.iter().map(|name| dir.join(name).exists()).collect();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(left, [false, false, true, true, true, true, true]);
    }

    #[test]
    fn a_run_removes_the_snapshots_others_left() {
        let dir = std::env::temp_dir().join(format!("gg-snapshots-left-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("a directory");
        let names = [
            "shards-7.snapshot",
            "shards-7-1.snapshot",
            "shards-42.snapshot",
            "shards-42-1.snapshot",
            "foxball-7.snapshot",
            "shards-7.snapshot.tmp",
            "shards-notes.txt",
        ];
        for name in names {
            fs::write(dir.join(name), b"").expect("written");
        }
        remove_snapshots_left(&dir, "shards", 42);
        let left: Vec<bool> = names.iter().map(|name| dir.join(name).exists()).collect();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(left, [false, false, true, true, true, true, true]);
    }

    #[test]
    fn an_executable_is_rebuilt_once_its_change_has_settled() {
        let start = Instant::now();
        let at = |millis| start + Duration::from_millis(millis);
        let stamp =
            |modified, len| Some((SystemTime::UNIX_EPOCH + Duration::from_secs(modified), len));
        let mut watch = Watch::new(stamp(1, 100), start);
        assert!(!watch.rebuilt(stamp(1, 100), at(5000)), "unchanged");
        // Settled a second after its last change, and told of once.
        assert!(!watch.rebuilt(stamp(2, 50), at(5100)));
        assert!(!watch.rebuilt(stamp(2, 120), at(5600)));
        assert!(!watch.rebuilt(stamp(2, 120), at(6500)));
        assert!(watch.rebuilt(stamp(2, 120), at(6600)));
        assert!(!watch.rebuilt(stamp(2, 120), at(9000)));
        // Missing while replaced isn't a build, however long.
        assert!(!watch.rebuilt(None, at(10_000)));
        assert!(!watch.rebuilt(None, at(12_000)));
        assert!(!watch.rebuilt(stamp(3, 120), at(12_100)));
        assert!(watch.rebuilt(stamp(3, 120), at(13_100)));
    }

    #[test]
    fn a_second_run_of_a_game_is_refused_until_the_first_ends() {
        let dir = std::env::temp_dir().join(format!("gg-run-lock-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gg-run-shards.lock");
        let first = lock_at(&path, "shards").expect("the first run's lock");
        let refused = lock_at(&path, "shards").err().map(|failure| failure.code);
        assert_eq!(refused.as_deref(), Some("gg.running"));
        drop(first);
        // Another test's spawn holds copies of every open file, `first`'s too, until it execs
        // (posix_spawn on macOS and Linux): the lock comes free then.
        let deadline = Instant::now() + Duration::from_secs(10);
        let again = loop {
            let again = lock_at(&path, "shards").map(drop);
            let held = matches!(&again, Err(failure) if failure.code == "gg.running");
            if !held || Instant::now() >= deadline {
                break again;
            }
            thread::sleep(Duration::from_millis(10));
        };
        let _ = fs::remove_dir_all(&dir);
        again.expect("the lock, once the first run has ended");
    }

    /// Each request a fake game was asked: method and parameters.
    type Asked = Arc<std::sync::Mutex<Vec<(String, Value)>>>;

    /// What a fake game says it waits for; a test may change it.
    type Waits = Arc<std::sync::Mutex<Option<&'static str>>>;

    /// How a fake game answers `snapshot`.
    #[derive(Clone, Copy)]
    enum Snapshots {
        Taken,
        /// With `api.not_ticked`.
        Refused,
        /// Never, as a game whose main thread a breakpoint holds.
        Never,
    }

    /// As [`faking_with`], waiting for `waiting` for good.
    fn faking(waiting: Option<&'static str>, snapshots: bool) -> (Session, Asked) {
        let snapshots = if snapshots {
            Snapshots::Taken
        } else {
            Snapshots::Refused
        };
        faking_with(Arc::new(waiting.into()), snapshots)
    }

    /// Serves the tool API on the loopback as a game would, a thread per connection: `ping` as
    /// `waits` says as it comes, `snapshot` as `snapshots` says, anything else with `{}`. Returns
    /// its session and what it's asked.
    fn faking_with(waits: Waits, snapshots: Snapshots) -> (Session, Asked) {
        use std::io::{BufReader, Write as _};

        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = listener.local_addr().expect("its address");
        let asked = Asked::default();
        let noting = asked.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (noting, waits) = (noting.clone(), waits.clone());
                thread::spawn(move || {
                    let mut writer = stream.try_clone().expect("a writer");
                    let mut reader = BufReader::new(stream);
                    while let Ok(Some(line)) = gg_api::read_line(&mut reader) {
                        let request: gg_api::Request =
                            serde_json::from_str(&line).expect("a request");
                        let (method, params) = (request.method.as_str(), &request.params);
                        noting
                            .lock()
                            .expect("noted")
                            .push((method.to_owned(), params.clone()));
                        let refused = Failure::new("api.not_ticked", "it hasn't ticked");
                        let answer = match method {
                            "hello" => {
                                Ok(r#"{"app":"Sandbox","pid":1,"engine":"0.1.0","log":null}"#
                                    .to_owned())
                            }
                            "ping" => {
                                let waiting = *waits.lock().expect("set");
                                Ok(json!({"frames": 3, "waiting": waiting}).to_string())
                            }
                            "snapshot" => match snapshots {
                                Snapshots::Taken => Ok(json!({
                                    "path": params["path"],
                                    "ticks": 3,
                                    "entities": 1,
                                    "bytes": 10,
                                })
                                .to_string()),
                                Snapshots::Refused => Err(&refused),
                                Snapshots::Never => {
                                    thread::sleep(Duration::from_secs(120));
                                    return;
                                }
                            },
                            _ => Ok("{}".to_owned()),
                        };
                        let answer = answer
                            .as_ref()
                            .map(String::as_str)
                            .map_err(|&failure| failure);
                        let line = gg_api::answer_line(request.id, answer);
                        if writer.write_all(line.as_bytes()).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        let session = Session {
            app: "Sandbox".to_owned(),
            pid: 1,
            address,
            token: "t".to_owned(),
            log: None,
            engine: "0.1.0 debug".to_owned(),
            aslr_reference: 0,
            player: None,
            project: None,
            protocol: Some(gg_api::PROTOCOL),
            hot: false,
        };
        (session, asked)
    }

    /// The methods asked but `hello`, each with whether it asked to quit.
    fn methods(asked: &Asked) -> Vec<(String, bool)> {
        let asked = asked.lock().expect("noted");
        let asked = asked.iter().filter(|(method, _)| method != "hello");
        let quits = |params: &Value| params["quit"].as_bool().unwrap_or(false);
        asked
            .map(|(method, params)| (method.clone(), quits(params)))
            .collect()
    }

    /// Snapshot and quit are one request, so no edit lands between to be lost; a refused snapshot
    /// is followed by a quit alone, as is a stop without one.
    #[test]
    fn a_game_is_snapshotted_and_quits_in_one_request() {
        let path = std::env::temp_dir().join("gg-ends.snapshot");
        let (session, asked) = faking(None, true);
        let taken = end(&session, Some(&path)).map(|taken| taken.map(|taken| taken.path));
        assert_eq!(taken, Some(Ok(path.clone())));
        assert_eq!(methods(&asked), [("snapshot".to_owned(), true)]);
        let (session, asked) = faking(None, false);
        let refused = end(&session, Some(&path)).map(|taken| taken.map_err(|failure| failure.code));
        assert_eq!(refused, Some(Err("api.not_ticked".to_owned())));
        let both = [("snapshot".to_owned(), true), ("quit".to_owned(), false)];
        assert_eq!(methods(&asked), both);
        let (session, asked) = faking(None, true);
        assert!(end(&session, None).is_none());
        assert_eq!(methods(&asked), [("quit".to_owned(), false)]);
    }

    /// A breakpoint, file dialog or window menu holds the main thread, which answers neither a
    /// snapshot nor a quit; a minimized or idle game doesn't.
    #[test]
    fn a_game_waiting_where_it_cant_answer_is_told_apart() {
        for (waiting, holds) in [
            (Some("breakpoint"), true),
            (Some("dialog"), true),
            (Some("menu"), true),
            (Some("minimized"), false),
            (None, false),
        ] {
            let (session, _) = faking(waiting, true);
            let held = holding(&mut None, &session);
            assert_eq!(held.is_some(), holds, "{waiting:?}");
        }
        // One that doesn't answer a ping, as an ended one, isn't.
        let (mut gone, _) = faking(None, true);
        gone.address = "127.0.0.1:1".parse().expect("an address");
        assert_eq!(holding(&mut None, &gone), None);
    }

    #[test]
    fn a_breakpoint_hit_while_a_snapshot_waits_ends_the_wait() {
        let waits = Waits::default();
        let (session, _) = faking_with(waits.clone(), Snapshots::Never);
        let path = std::env::temp_dir().join("gg-held.snapshot");
        let ending = session.clone();
        let asked = thread::spawn(move || end(&ending, Some(&path)));
        let start = Instant::now();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            *waits.lock().expect("set") = Some("breakpoint");
        });
        let held = awaiting(asked, &mut None, &session)
            .err()
            .map(|(waiting, _)| waiting);
        assert_eq!(held.as_deref(), Some("breakpoint"));
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "it waited it out"
        );
    }

    /// The snapshot in flight is joined and noted as the one it restarts from, which `killed`
    /// names.
    #[test]
    fn a_game_held_at_a_breakpoint_is_killed_as_its_stopped() {
        let dir = std::env::temp_dir().join(format!("gg-killed-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("a directory");
        #[cfg(windows)]
        let mut command = Command::new("ping");
        #[cfg(windows)]
        command.args(["-n", "60", "127.0.0.1"]);
        #[cfg(not(windows))]
        let mut command = Command::new("sleep");
        #[cfg(not(windows))]
        command.arg("60");
        let child = command.stdin(Stdio::piped()).spawn().expect("it starts");
        let (session, asked) = faking(Some("breakpoint"), true);
        let latest = Snapshotted {
            path: dir.join("sandbox-1.snapshot"),
            ticks: 300,
            entities: 7,
            bytes: 4096,
        };
        let taken = latest.clone();
        let (pid, now) = (child.id(), Instant::now());
        let mut game = Game {
            child,
            pid,
            restored: None,
            session: Some(session),
            client: None,
            frames: 3,
            launched: now,
            moved: now,
            pinged: now,
            waiting: None,
            loading: None,
            snapped: now,
            snapshotted: false,
            snapshotting: Some(thread::spawn(move || Ok(taken))),
        };
        let run = run(&[], 5);
        let mut supervisor = Supervisor {
            run: &run,
            exe: dir.join("gg-sandbox"),
            peers: vec![Peer {
                args: Vec::new(),
                copy: dir.join("gg-sandbox-running"),
                snapshot: latest.path.clone(),
                latest: None,
            }],
            dir: dir.clone(),
            serves: true,
            restarts: true,
            served: true,
            scan_told: false,
            failed: None,
            patches: None,
            closed: None,
            told: None,
        };
        let stopped = supervisor.stop(0, &mut game, true);
        let ended = game.child.try_wait().expect("waited for");
        let _ = fs::remove_dir_all(&dir);
        stopped.expect("stopped");
        assert!(ended.is_some(), "killed");
        assert!(
            now.elapsed() < Duration::from_secs(20),
            "it waited out its time"
        );
        assert!(game.snapshotting.is_none() && game.snapshotted);
        assert_eq!(supervisor.peers[0].latest.as_ref(), Some(&latest));
        // It was asked for nothing but pings.
        assert!(methods(&asked).iter().all(|(method, _)| method == "ping"));
        let event = killed(pid, "breakpoint", Some(&latest));
        assert_eq!(
            event["from"],
            json!({"snapshot": latest.path, "ticks": 300})
        );
        assert_eq!(killed(pid, "menu", None)["from"], Value::Null);
    }

    #[test]
    fn a_snapshot_written_since_it_was_asked_counts_unanswered() {
        let dir = std::env::temp_dir().join(format!("gg-written-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("a directory");
        let path = dir.join("sandbox-1.snapshot");
        let before = SystemTime::now() - Duration::from_secs(60);
        let text = "gg-snapshot 2\napp Sandbox\nengine 0.1.0\nlayers\nticks 42\nhash 0 0\n\
                    table Transform\nrow 1 a ()\nrow 2 b ()\nstopped\npaused\nend\n";
        fs::write(&path, text).expect("written");
        let whole = written_since(&path, before);
        let late = written_since(&path, SystemTime::now() + Duration::from_secs(60));
        fs::write(&path, &text[..text.len() - 4]).expect("written");
        let cut = written_since(&path, before);
        let _ = fs::remove_dir_all(&dir);
        let whole = whole.expect("whole");
        assert_eq!((whole.ticks, whole.entities), (42, 2));
        assert!(late.is_none() && cut.is_none());
    }

    #[test]
    fn a_game_starts_again_from_its_latest_snapshot_or_waits_for_a_rebuild() {
        let latest = Snapshotted {
            path: PathBuf::from("sandbox-1.snapshot"),
            ticks: 300,
            entities: 7,
            bytes: 4096,
        };
        let from_latest = Next::Start(Some(latest.clone()));
        let failed = |snapshotted, restored| Happened::Failed {
            snapshotted,
            restored,
            served: true,
        };
        // Its own end ends the run, clean or not; a crash or hang does only with restarts off or
        // if no process ever served.
        for (restarts, clean) in [(true, true), (true, false), (false, true)] {
            assert_eq!(
                next(Happened::Exited { clean }, Some(&latest), restarts),
                Next::End(clean)
            );
        }
        assert_eq!(
            next(failed(true, false), Some(&latest), false),
            Next::End(false)
        );
        let unserved = Happened::Failed {
            snapshotted: false,
            restored: false,
            served: false,
        };
        assert_eq!(next(unserved, None, true), Next::End(false));
        // After a snapshot of it, from the latest.
        assert_eq!(next(failed(true, true), Some(&latest), true), from_latest);
        // Before one, it waits for a rebuild, whatever an earlier process took.
        for restored in [false, true] {
            let waits = next(failed(false, restored), Some(&latest), true);
            assert!(matches!(waits, Next::Wait(_)), "{waits:?}");
        }
        // The tool an embedded run is for ending ends it cleanly.
        assert_eq!(next(Happened::Closed, Some(&latest), true), Next::End(true));
        // A rebuild starts from the latest, or afresh if none or asked to; a refusal, afresh.
        let rebuilt = |fresh| Happened::Rebuilt { fresh };
        assert_eq!(next(rebuilt(false), Some(&latest), true), from_latest);
        assert_eq!(next(rebuilt(false), None, true), Next::Start(None));
        assert_eq!(next(rebuilt(true), Some(&latest), true), Next::Start(None));
        assert_eq!(
            next(Happened::Refused, Some(&latest), true),
            Next::Start(None)
        );
    }
}
