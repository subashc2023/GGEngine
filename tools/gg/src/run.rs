//! Running a game's development build: supervised, patched as its code changes, and started again
//! from a snapshot after a crash, a hang or a rebuild, until it exits, alone or as the two peers of a
//! lockstep session; or headless for a set number of frames, as a check.

use std::fs::{self, TryLockError};
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

use crate::patch::{Patches, Process, Watcher};
use crate::runs::{self, State, Told};
use crate::{Scope, build, game};

/// What `gg run` runs: a package, the project gg works in, if any, the package's profile, whether
/// it's patched, unless `--no-patch`, how long its frames may stop before it has hung, `None` for
/// as long as they stop, how often it's snapshotted, zero for never, how many peers of a lockstep
/// session it runs, 1 for a game alone, whether it's embedded in the tool that started gg, and the
/// arguments each is given.
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
    /// Why the run's options turn patches off, if they do: `--no-patch`; `--release`, as subsecond
    /// patches debug builds alone; or restarts' being off, as an edit no patch carries rebuilds the
    /// game and starts it again, and a recording would replay without the patches its run took.
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

    /// Whether the game serves the tool API: not given `--no-api`, nor replaying a recording.
    fn serves(&self) -> bool {
        !self.given("--no-api") && !self.given("--replay")
    }

    /// Whether the game starts again after a crash, a hang or a rebuild: only if it's snapshotted,
    /// which takes the API, and doesn't record, as a restart would truncate the recording.
    pub(crate) fn restarts(&self) -> bool {
        self.serves() && !self.given("--record") && !self.snapshot.is_zero()
    }

    /// Whether the game's arguments give flag `flag`, as `--flag` or `--flag=value`.
    pub(crate) fn given(&self, flag: &str) -> bool {
        self.args.iter().any(|arg| {
            (arg.strip_prefix(flag)).is_some_and(|rest| rest.is_empty() || rest.starts_with('='))
        })
    }

    /// The directory cargo runs in: the project's, inside one.
    pub(crate) fn dir(&self) -> Option<&Path> {
        self.project.as_ref().map(Scope::dir)
    }
}

/// What `gg check` runs: a package, the project gg works in, if any, the package's profile, how
/// many frames, how long it may take, and the arguments it's given.
pub(crate) struct Check {
    pub(crate) package: String,
    pub(crate) project: Option<Scope>,
    pub(crate) release: bool,
    pub(crate) frames: u64,
    pub(crate) timeout: Duration,
    pub(crate) args: Vec<String>,
}

/// How long a game has to run its first frame, as long as it waits for nothing the user does,
/// before it counts as hung: a debug build that loads much as it starts takes a while.
const STARTUP: Duration = Duration::from_secs(120);

/// How often a supervised game is pinged.
const PING: Duration = Duration::from_secs(1);

/// How long a ping's answer may take, which the game's connection's own thread gives.
const PULSE: Duration = Duration::from_secs(5);

/// How long a snapshot's answer may take: the game copies its world in about a frame, but a thread
/// of its own writes it, which takes a while for a large world in a debug build.
const SNAPSHOT: Duration = Duration::from_secs(60);

/// Why an embedded run ends as its tool does, as its `closed` event says.
const CLOSED: &str = "gg's stdin closed, as the tool that started it ended";

/// How long a game asked to quit, as its executable has been rebuilt, has to end before it's
/// killed.
const QUIT: Duration = Duration::from_secs(10);

/// How long an executable that has changed must stay the same before it counts as rebuilt: cargo
/// may still be writing it.
const SETTLE: Duration = Duration::from_secs(1);

/// How long copying the executable, or removing the copy or a patch's library, is tried for:
/// Windows may hold a file a process ran or loaded for a moment after it ends.
const COPYING: Duration = Duration::from_secs(5);

/// Prints `event`, a line of JSON.
pub(crate) fn event(event: &Value) {
    crate::print(&event.to_string());
}

/// `duration` in whole milliseconds, as events give times.
pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Builds `run`'s package, with dx while patches are on, as [`Patches::new`] says, printing `built`
/// with how long that took, and runs a copy of its executable, so that it can be built again
/// meanwhile, supervising the game until it exits cleanly: returns whether it did, having printed
/// a line for each event. While restarts are on, it snapshots the game every so often as it serves
/// the API, and starts it again from a snapshot after a crash or a hang, and as its executable is
/// rebuilt, as [`next`] says; with them off, the game's first end ends it. While patches are on, it
/// patches the game, or rebuilds it, as its sources change, as [`Patches::tend`] says. Two peers
/// run as one game: the host, given `--host` at a port of the loopback the OS picks, and the
/// joiner, given `--join` there, each from a copy of its own, patched together, and stopped and
/// started again together, as any of them ends, so that they always run the same code; each starts
/// from its own snapshot, the joiner going on from the host's world as it joins.
///
/// Embedded, each game is given `--embedded`, for the tool that started gg, which shows its view,
/// and the run ends cleanly once gg's stdin closes, as the tool ends, stopping its games, though a
/// build under way finishes first, as one does as the run ends however it ends.
///
/// Each game is given `--supervised`, and a stdin gg holds, which closes as gg ends, however it
/// ends, being killed too, ending the game then; one still running as the run fails is killed.
///
/// Fails as [`check_game`] does, with `gg.running` if another `gg run` of the package runs, as
/// [`lock`] says, `gg.build` if it doesn't build, `gg.cargo` if cargo can't say what the package
/// is, and `gg.run` if it can't be copied, started or waited for, or no port is free for the
/// peers.
pub(crate) fn run(run: &Run) -> Result<bool, Failure> {
    check_game(&run.package, run.project.as_ref())?;
    let (_lock, target) = lock(&run.package, run.dir())?;
    let pid = std::process::id();
    // Told before the build, as gg's other commands find no game while it builds.
    let manifest = run.project.as_ref().map(|scope| scope.manifest.as_path());
    let told = Told::start(
        runs::path(&target, &run.package),
        pid,
        &run.package,
        manifest,
        &State::new("building"),
    );
    // Before the build, which the tool may end under way.
    let closed = run.embedded.then(closes);
    let building = Instant::now();
    let First::Built(patches) = first_build(run, &told, closed.as_deref())? else {
        event(&json!({"event": "closed", "why": CLOSED}));
        return Ok(true);
    };
    let exe = match &patches {
        Some(patches) => patches.exe().to_owned(),
        None => build::build(&run.package, release(run.release), run.dir())?,
    };
    let build_ms = millis(building.elapsed());
    event(&json!({"event": "built", "exe": exe, "build_ms": build_ms}));
    let dir = exe.parent().unwrap_or(Path::new(".")).to_owned();
    // A directory of its own, which the game's own pruning of `snapshots` leaves alone.
    let snapshots = dir.join("snapshots").join("gg");
    let restarts = run.restarts();
    if restarts {
        // A snapshot that can't be written there says why.
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
    let mut supervisor = Supervisor {
        run,
        peers: peers.collect(),
        exe,
        dir,
        serves: run.serves(),
        restarts,
        served: false,
        failed: None,
        patches: patches.map(|patches| *patches),
        closed,
        told: Some(told),
    };
    let supervised = supervisor.supervise();
    if let Some(patches) = &mut supervisor.patches {
        patches.finish();
    }
    // Once its games have ended; one still running keeps its copy, and the patches' libraries it
    // loaded.
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

/// What the first build of a run's game came to.
enum First {
    /// It built, with these patches, if they're on.
    Built(Option<Box<Patches>>),
    /// The tool an embedded run is for ended as it waited for a change to build it again.
    Closed,
}

/// Builds `run`'s game the first time, as [`Patches::new`] does, and returns its patches. While it watches the game's sources, as without `--no-patch`
/// and with restarts on, a build that fails doesn't end the run: it prints `build_failed`, with
/// rustc's errors, and tells it, as `waiting`, with them, until the sources change, and builds the
/// game again then; [`First::Closed`] if the tool an embedded run is for ends meanwhile, which
/// `closed`, if it's given, says.
///
/// Fails as `Patches::new` does, but with `gg.build` for a build that fails while it watches.
fn first_build(run: &Run, told: &Told, closed: Option<&AtomicBool>) -> Result<First, Failure> {
    let watches = run.patches && run.restarts();
    loop {
        let failure = match Patches::new(run) {
            Ok(patches) => return Ok(First::Built(patches.map(Box::new))),
            Err(failure) if watches && failure.code == "gg.build" => failure,
            Err(failure) => return Err(failure),
        };
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
            if watcher.changed(now) {
                break;
            }
            told.tell(&waiting, looked);
            thread::sleep(Duration::from_millis(100));
        }
        told.tell(&State::new("building"), runs::now_ms());
    }
}

/// Takes the lock of `package`'s `gg run`, a file in the target directory cargo builds the
/// workspace into, as `cargo metadata` run in `dir` says, which the process holds until it ends,
/// however it ends, so that a second `gg run` of the game is refused: the two would build, patch
/// and restart it over each other.
///
/// Returns it with the target directory.
///
/// Fails with `gg.running` if another process holds it, `gg.cargo` if cargo can't say where the
/// target directory is, and `gg.run` if the file can't be opened or locked.
fn lock(package: &str, dir: Option<&Path>) -> Result<(fs::File, PathBuf), Failure> {
    let target = build::target_dir(dir)?;
    let _ = fs::create_dir_all(&target);
    let lock = lock_at(&target.join(format!("gg-run-{package}.lock")), package)?;
    Ok((lock, target))
}

/// Takes the lock of `package`'s `gg run` at `path`, as [`lock`] says.
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

/// Watches gg's stdin on a thread of its own, and returns what it sets once stdin closes, as the
/// tool that started gg ends, whatever it wrote.
fn closes() -> Arc<AtomicBool> {
    let closed = Arc::new(AtomicBool::new(false));
    let setting = closed.clone();
    thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
        setting.store(true, Ordering::Relaxed);
    });
    closed
}

/// The arguments each of `peers` peers is given beside the run's: none for a game alone; for two,
/// the host's `--host` and the joiner's `--join`, at a port of the loopback free as the run starts.
///
/// Fails with `gg.run` if no port is.
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
    let address = address.to_string();
    Ok(vec![
        vec!["--host".to_owned(), address.clone()],
        vec!["--join".to_owned(), address],
    ])
}

/// The name of the snapshot of peer `peer`, from 0, of `package`'s game under the `gg run` of
/// process `pid`, as `sandbox-42.snapshot`, or `shards-42-1.snapshot` for a joiner.
fn snapshot_name(package: &str, pid: u32, peer: usize) -> String {
    match peer {
        0 => format!("{package}-{pid}.snapshot"),
        peer => format!("{package}-{pid}-{peer}.snapshot"),
    }
}

/// What `gg run` keeps across the game's restarts: what it runs, the executable, each peer's, one
/// for a game alone, whether a process of the game has served the API, the code of the last
/// snapshot failure printed, until one succeeds, and the run's patches, while they're on.
struct Supervisor<'a> {
    run: &'a Run,
    exe: PathBuf,
    peers: Vec<Peer>,
    /// The executable's directory, where the game writes its logs and session files.
    dir: PathBuf,
    serves: bool,
    restarts: bool,
    served: bool,
    failed: Option<String>,
    patches: Option<Patches>,
    /// Set once gg's stdin closes, while the run's embedded.
    closed: Option<Arc<AtomicBool>>,
    /// What the run tells of itself, for gg's other commands to read while no game answers.
    told: Option<Told>,
}

/// A peer of the game under `gg run`, or the game alone: the arguments it's given beside the run's,
/// the copy of the executable it runs, where it's snapshotted, and its last snapshot that
/// succeeded.
struct Peer {
    args: Vec<String>,
    copy: PathBuf,
    snapshot: PathBuf,
    latest: Option<Snapshotted>,
}

/// A game's process under `gg run`, and what's known of it: the snapshot it started from, its
/// session once it serves, its frames and when they last moved, and its snapshots. Its process is
/// killed as it drops, if it hasn't ended, as on an error that ends the run.
struct Game {
    /// Its process, whose stdin it holds, which closes as gg ends, however it ends, ending a game
    /// given `--supervised`.
    child: Child,
    pid: u32,
    restored: Option<Snapshotted>,
    session: Option<Session>,
    /// The connection pings go through.
    client: Option<Client>,
    frames: u64,
    moved: Instant,
    pinged: Instant,
    /// What frames had paused for at the last ping answered.
    waiting: Option<String>,
    /// When it was last asked for a snapshot, or its first frame was seen: the first comes a
    /// snapshot's interval of running after where it started, so that one of a game that ends the
    /// same way soon after each start doesn't start it again.
    snapped: Instant,
    /// Whether a snapshot of it has succeeded.
    snapshotted: bool,
    /// A snapshot being taken, on a thread of its own, so that pings go on meanwhile.
    snapshotting: Option<JoinHandle<Result<Snapshotted, Failure>>>,
}

impl Supervisor<'_> {
    /// Runs the game until it exits cleanly, or with restarts off until it ends at all, returning
    /// whether it exited cleanly. Peers go together: as one ends, the others stop, as for a
    /// rebuild, and what [`next`] says of the one that ended is what all of them do.
    fn supervise(&mut self) -> Result<bool, Failure> {
        // Taken before the first copy, so that a rebuild as it's made shows.
        let mut watch = Watch::new(stamp(&self.exe), Instant::now());
        // Empty while it waits for a rebuild.
        let mut games = self.start(vec![None; self.peers.len()])?;
        // Set once the executable is rebuilt, until the game starts again from the new build.
        let mut rebuilt = false;
        // Why the game waits for a rebuild, while it does.
        let mut waits = None;
        loop {
            let (now, looked) = (Instant::now(), runs::now_ms());
            // While patches are on, the run makes the builds itself.
            if let Some(patches) = &mut self.patches {
                let processes: Vec<Process> = games.iter().map(Game::process).collect();
                rebuilt |= patches.tend(&processes, now);
            } else if self.restarts && watch.rebuilt(stamp(&self.exe), now) {
                event(&json!({"event": "rebuilt", "exe": self.exe}));
                rebuilt = true;
            }
            self.tell(&games, waits, looked);
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
                happened = Some((0, Happened::Rebuilt));
            }
            let Some((ended, happened)) = happened else {
                thread::sleep(Duration::from_millis(100));
                continue;
            };
            // The rest stop, snapshotted first, so that they start again with the one that ended;
            // or all of them, not, as the run ends.
            let all = matches!(happened, Happened::Rebuilt | Happened::Closed);
            let snapshot = !matches!(happened, Happened::Closed);
            let stopping = if matches!(happened, Happened::Rebuilt) {
                "restarting"
            } else {
                "stopping"
            };
            self.told(State {
                pids: games.iter().map(|game| game.pid).collect(),
                ..State::new(stopping)
            });
            for (peer, game) in games.iter_mut().enumerate() {
                if all || peer != ended {
                    self.stop(peer, game, snapshot)?;
                }
            }
            games.clear();
            match next(happened, self.peers[ended].latest.as_ref(), self.restarts) {
                Next::Start(from) => {
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

    /// Tells what the run is doing, given the game's processes, `games`, none while it waits for a
    /// rebuild, for `waits`, as of a look at the sources at `looked`, as [`Patches::state`] says
    /// while patches are on; else starting until every process serves, then running.
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

    /// Tells `state`, as of now.
    fn told(&self, state: State) {
        if let Some(told) = &self.told {
            told.tell(&state, runs::now_ms());
        }
    }

    /// Starts each peer, the game alone or a lockstep session's, from its snapshot in `froms`, or
    /// afresh, as [`start_peer`](Self::start_peer) does, the executable copied again if it may have
    /// been rebuilt, as without patches it always may.
    fn start(&mut self, froms: Vec<Option<Snapshotted>>) -> Result<Vec<Game>, Failure> {
        let built = self.patches.as_mut().is_none_or(Patches::starting);
        let froms = froms.into_iter().enumerate();
        froms
            .map(|(peer, from)| self.start_peer(peer, from, built))
            .collect()
    }

    /// Starts peer `peer` from snapshot `from`, or afresh: copies the executable again if `built`,
    /// runs the copy, and prints `started`.
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
            moved: now,
            pinged: now,
            waiting: None,
            snapped: now,
            snapshotted: false,
            snapshotting: None,
        })
    }

    /// Looks after `game`, peer `peer`'s, for a turn of the loop: finds its session once it serves,
    /// snapshots it, and pings it; and returns what happened if its process ended, or hung and was
    /// ended, having printed it.
    fn tend(&mut self, peer: usize, game: &mut Game) -> Result<Option<Happened>, Failure> {
        let ended = game.child.try_wait();
        if let Some(status) = ended.map_err(|error| failed(&self.peers[peer].copy, error))? {
            return Ok(Some(self.ended(peer, game, status)));
        }
        if let Some(taking) = &game.snapshotting
            && taking.is_finished()
            && let Some(taken) = game.landed()
        {
            self.note(peer, game, taken);
        }
        if self.serves && game.session.is_none() {
            let pid = game.pid;
            let mut found = Session::list(&Session::dir(&self.dir)).into_iter();
            let found = found.find(|(_, session)| session.pid == pid);
            if let Some((file, session)) = found {
                event(&json!({
                    "event": "serving",
                    "pid": pid,
                    "app": session.app,
                    "address": session.address,
                    "session": file,
                    "project": session.project,
                    "log": session.log,
                }));
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
            && game.waiting.is_none()
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
            if pulse.frames != game.frames || pulse.waiting.is_some() {
                if game.frames == 0 {
                    game.snapped = Instant::now();
                }
                game.frames = pulse.frames;
                game.moved = Instant::now();
            }
            game.waiting = pulse.waiting;
        }
        // With hangs not told, as `--hang 0` asks, a run that never runs a frame isn't either.
        let Some(hang) = self.run.hang else {
            return Ok(None);
        };
        let limit = if game.frames == 0 { STARTUP } else { hang };
        if game.moved.elapsed() <= limit {
            return Ok(None);
        }
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
        }));
        Ok(Some(game.failed(self.served)))
    }

    /// What happened as `game`'s process ended with `status`, having printed it: it exited, or
    /// crashed, or, started from a snapshot, refused it, failing with an error about the snapshot,
    /// which it reads before it serves and loads the world of once its layers have loaded, after.
    /// One that came to its own end, as its window closed, a `quit` asked or its frames ran out,
    /// logs `engine.perf`, which one that crashed doesn't, and exited, whatever its status: 1 if
    /// validation reported anything.
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

    /// What `game` left as its process ended: its log, and the errors it holds; and removes the
    /// session file a process that was killed leaves, as [`forget`](Self::forget) says. A snapshot
    /// in flight then counts if it was written first; if it failed, the end is why.
    fn remains(&mut self, peer: usize, game: &mut Game) -> (Option<PathBuf>, Vec<Value>) {
        if let Some(Ok(taken)) = game.landed() {
            self.note(peer, game, Ok(taken));
        }
        let log = log(&self.dir, game.pid, game.session.as_ref());
        let errors = log.as_deref().map(errors).unwrap_or_default();
        self.forget(game.pid);
        (log, errors)
    }

    /// Removes the session files of process `pid`, which has ended: one that was killed, or that
    /// aborted, leaves its own, which a later process given its PID would be taken for, as its
    /// pings went to an address no game serves.
    fn forget(&self, pid: u32) {
        for (path, session) in Session::list(&Session::dir(&self.dir)) {
            if session.pid == pid {
                let _ = fs::remove_file(path);
            }
        }
    }

    /// Stops `game`, peer `peer`'s, as its executable has been rebuilt, another peer ended or the
    /// run ends: if it serves, once a snapshot in flight is done, asks it to end, snapshotted first
    /// if `snapshot`, as [`end`] does, killing it if it hasn't ended within [`QUIT`]; if it doesn't
    /// serve yet, kills it. One that waits where it can't answer, as [`holding`] says, before it's
    /// asked or while the snapshots wait, is killed at once, printing `killed`, with the snapshot it
    /// starts again from, if any, past which what changed in it is lost.
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
        // A snapshot whose answer didn't come, as one that came too late, may have been written
        // all the same, before the game ended.
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

    /// Asks `game`, peer `peer`'s, which serves as `session` says, to end, once the snapshot in
    /// flight is done, snapshotted first if `snapshot`, as [`end`] does; and returns what that
    /// snapshot gave, if one was asked, with when it was, which the caller notes once the game has
    /// ended.
    ///
    /// Fails with what the game waits for if it waits where it can't answer, as [`holding`] says,
    /// before it's asked or while either snapshot waits, as at a breakpoint a tick hits meanwhile.
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
                    // Joined as the game's killed, which ends it.
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

    /// Notes what a snapshot of `game`, peer `peer`'s, gave, `taken`: the snapshot, which restarts
    /// start from; or the failure, printed unless the last one printed had its code.
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
    /// What patches see of it.
    fn process(&self) -> Process<'_> {
        Process {
            pid: self.pid,
            session: self.session.as_ref(),
        }
    }

    /// What the snapshot in flight gave, waiting for it to be done; `None` if none was in flight,
    /// or its thread panicked.
    fn landed(&mut self) -> Option<Result<Snapshotted, Failure>> {
        self.snapshotting.take()?.join().ok()
    }

    /// What happened, as its process crashed or hung, `served` if a process of the run has served
    /// the API.
    fn failed(&self, served: bool) -> Happened {
        Happened::Failed {
            snapshotted: self.snapshotted,
            restored: self.restored.is_some(),
            served,
        }
    }
}

impl Drop for Game {
    /// Kills its process and waits for it, so that a run that fails while games run, as when the
    /// joiner can't start after the host has, leaves none behind. One the run has waited for, as it
    /// waits for each before dropping it otherwise, is left alone: std signals no process it has
    /// reaped.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What happened to a game under `gg run`, of which [`next`] says what follows.
enum Happened {
    /// Its process came to its own end, as its window closed, a `quit` asked or its frames ran out:
    /// `clean` if it exited cleanly, rather than with what validation reported.
    Exited { clean: bool },
    /// Its process crashed or hung: `snapshotted` if a snapshot of it was taken, `restored` if it
    /// was started from one, and `served` if a process of the run has served the API.
    Failed {
        snapshotted: bool,
        restored: bool,
        served: bool,
    },
    /// Its process, started from a snapshot, refused it.
    Refused,
    /// Its executable was rebuilt, and its process, if one ran, has stopped.
    Rebuilt,
    /// gg's stdin closed, as the tool that started an embedded run ended, and its processes have
    /// stopped.
    Closed,
}

/// What a snapshot a game was asked for as it ended gave, and when it was asked, which tells a file
/// it wrote before it ended from an older one, should its answer not have come.
type SnapshotAsked = (Result<Snapshotted, Failure>, SystemTime);

/// What `gg run` does next.
#[derive(Debug, PartialEq)]
enum Next {
    /// Starts the game again: from the snapshot, or afresh.
    Start(Option<Snapshotted>),
    /// Waits for its executable to be rebuilt, for the reason given.
    Wait(&'static str),
    /// Ends, returning whether the game exited cleanly.
    End(bool),
}

/// What `gg run` does after `happened`, given the last snapshot that succeeded, `latest`, and
/// whether restarts are on. A game that crashed or hung starts again from the latest snapshot if
/// one of it was taken, which is a snapshot's interval of running or more past where it started;
/// else it would start as it did and likely end the same way again, so it waits for a rebuild. One
/// none of whose processes has served the API can't be snapshotted, nor told apart from one that
/// never will, as one given an argument it doesn't take, which no rebuild changes, so the run ends.
/// A rebuild starts it from the latest snapshot, which is one taken as it stopped if that
/// succeeded; and one that refused its snapshot starts afresh.
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
        Happened::Rebuilt => Next::Start(latest.cloned()),
        Happened::Refused => Next::Start(None),
    }
}

/// When a file was last changed, and its length, which show that it changed, as an executable that
/// was rebuilt; `None` while it can't be read, as while cargo replaces an executable.
pub(crate) type Stamp = Option<(SystemTime, u64)>;

/// Executable `exe`'s stamp, as the file system has it.
fn stamp(exe: &Path) -> Stamp {
    fs::metadata(exe).ok().and_then(stamped)
}

/// The stamp of the file whose metadata is `metadata`.
pub(crate) fn stamped(metadata: fs::Metadata) -> Stamp {
    Some((metadata.modified().ok()?, metadata.len()))
}

/// Tells from an executable's stamps when it has been rebuilt: once its stamp differs from the
/// build's, and has stayed the same for [`SETTLE`].
struct Watch {
    /// The stamp of the build last told of, or of the first.
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

    /// Takes the executable's stamp, `stamp`, at `now`, and returns whether it has been rebuilt
    /// since it last said so. One that can't be read, as once it's removed, isn't a build.
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

/// Where the `gg run` of process `pid` copies executable `exe` to run peer `peer` of it, from 0, the
/// game alone being peer 0: beside it, so that what the game writes beside its executable stays
/// where it is, and named for the process and the peer, so that two runs of one game each have
/// one, and each peer, as `gg-sandbox-running-42.exe` for `gg-sandbox.exe`, and
/// `gg-shards-running-42-1.exe` for a joiner.
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

/// Removes the copies of executable `exe` beside it that the runs of processes other than `pid`
/// left, as a run that was killed does, which no other run would remove. One that a game runs
/// from stays on Windows, which keeps it; elsewhere, its run copies it again before it next starts
/// the game.
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

/// Removes the snapshots in `dir` that the `gg run`s of `package` in processes other than `pid`
/// left, as one that was killed, or whose game crashed for good, leaves its own: no other run of
/// the package runs while this one holds its lock, so none of them is another's to start from.
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
/// Fails with `gg.run` if it still can't.
fn copy(exe: &Path, to: &Path) -> Result<(), Failure> {
    // Removed first, so that the copy is a new file: macOS keeps the code signature it checked for
    // a file that ran, and kills what runs from the file once it's written over
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

/// Does `what` until it succeeds, for at most [`COPYING`], returning how it last went.
pub(crate) fn retrying(mut what: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let start = Instant::now();
    loop {
        match what() {
            Err(_) if start.elapsed() < COPYING => thread::sleep(Duration::from_millis(100)),
            done => return done,
        }
    }
}

/// Has the game `session` names write a snapshot at `path`, through a connection of its own, which
/// waits as long as a large world's snapshot may take, and returns what it answers; and end once
/// it's written, if `quit`.
fn snapshot(session: &Session, path: &Path, quit: bool) -> Result<Snapshotted, Failure> {
    let path = Some(path.to_owned());
    Client::connect(session, "gg", SNAPSHOT)?.call_as("snapshot", Snapshot { path, quit })
}

/// Asks the game `session` names to end, snapshotted at `path` first if it's given, as one
/// request, so that nothing it's asked between the two applies, to be lost as it starts again
/// from the snapshot, as it refuses whatever it's asked after; and returns what the snapshot gave,
/// if one was asked. One the game didn't take, as before its first tick, leaves it running, so
/// it's then asked to quit alone.
fn end(session: &Session, path: Option<&Path>) -> Option<Result<Snapshotted, Failure>> {
    let taken = path.map(|path| snapshot(session, path, true));
    if !matches!(taken, Some(Ok(_)))
        && let Ok(mut client) = Client::connect(session, "gg", PULSE)
    {
        let _ = client.call("quit", Value::Null);
    }
    taken
}

/// What `taking`, a thread that asks the game `session` names for something, gives, once it's
/// done, or `None` if it panicked; pinging the game through `client` meanwhile, every [`PING`].
///
/// Fails with what the game waits for, and the thread, if it comes to wait where it can't answer,
/// as [`holding`] says, which the thread would only wait out.
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

/// The `killed` event of the game in process `pid`, killed as it waited for `waiting`, which starts
/// again from `latest`, its last snapshot, if it has one, and else afresh.
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

/// The snapshot at `path`, if it was written whole there since `since`, as one the game wrote
/// before it ended, whose answer it didn't give: its ticks, its entities, as its world's rows count
/// them, and its size.
fn written_since(path: &Path, since: SystemTime) -> Option<Snapshotted> {
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
    // The run's world's rows, before what follows it.
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

/// What the game `session` names waits for, as a ping through `client` answers, if it's what holds
/// its main thread, which then answers no request but those its connections' threads do: a script
/// stopped at a breakpoint, a file dialog, or the window menu Windows runs; `None` for anything
/// else, as a minimized window, between whose frames it answers.
fn holding(client: &mut Option<Client>, session: &Session) -> Option<String> {
    let waiting = ping(client, session)?.waiting?;
    ["breakpoint", "dialog", "menu"]
        .contains(&waiting.as_str())
        .then_some(waiting)
}

/// The pulse of the game `session` names, through `client`, which this connects again once it
/// breaks; `None` if the game doesn't answer in time.
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

/// The JSON log of the run in process `pid` of the executable in `dir`: the one its session
/// names, if it served the API, else the one named for its process.
fn log(dir: &Path, pid: u32, session: Option<&Session>) -> Option<PathBuf> {
    let named = session.and_then(|session| session.log.clone());
    named.or_else(|| game::log_of(&dir.join("logs"), pid))
}

/// Whether the run whose JSON log is at `log` came to its own end: it logged `engine.perf`, which a
/// run that crashed, hung or refused its snapshot doesn't.
fn ended_itself(log: &Path) -> bool {
    let mut events = game::events(log).into_iter();
    events.any(|event| event.get("code").and_then(Value::as_str) == Some("engine.perf"))
}

/// The errors the JSON log at `log` holds, as `gg` reports events.
fn errors(log: &Path) -> Vec<Value> {
    let events = game::events(log).into_iter();
    let errors = events.filter(|event| event.get("level").and_then(Value::as_str) == Some("ERROR"));
    errors.map(game::trimmed).collect()
}

fn failed(exe: &Path, error: std::io::Error) -> Failure {
    Failure::new("gg.run", format!("can't run {}: {error}", exe.display()))
}

/// Builds `check`'s package and runs it headless for its frames, without the tool API, and prints
/// what it did as a line of JSON: its exit status, its warnings and errors, and `engine.perf`'s
/// frame times. Returns whether it passed: it exited cleanly, in time, and logged no error.
///
/// Fails as [`check_game`] does, with `gg.build` if it doesn't build, and `gg.run` if it can't
/// start or be waited for.
pub(crate) fn check(check: &Check) -> Result<bool, Failure> {
    check_game(&check.package, check.project.as_ref())?;
    let exe = build::build(
        &check.package,
        release(check.release),
        check.project.as_ref().map(Scope::dir),
    )?;
    let frames = check.frames.to_string();
    // Its console says what its JSON log does, which is read below. Its stdin, which `child` holds,
    // closes as gg ends, however it ends, ending the game then.
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

/// Cargo's flags for a development build, optimized if `release`.
fn release(release: bool) -> &'static [&'static str] {
    if release { &["--release"] } else { &[] }
}

/// Checks that `package`, if it's the game of the project `project`, gg works in, is the package
/// beside the project's manifest, as [`build::check_project`] does.
pub(crate) fn check_game(package: &str, project: Option<&Scope>) -> Result<(), Failure> {
    match project {
        Some(scope) if scope.game == package => build::check_project(scope),
        _ => Ok(()),
    }
}

/// Waits for `child` to end, for at most `timeout`, past which it's ended and `None` returned.
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

    /// A game dropped, as a run that fails drops those running, kills its process, which its stdin's
    /// closing alone, as it drops, wouldn't end.
    #[test]
    fn a_game_dropped_kills_its_process() {
        // A process that runs for a minute, whatever its stdin, its output ending as it does.
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
            moved: now,
            pinged: now,
            waiting: None,
            snapped: now,
            snapshotted: false,
            snapshotting: None,
        });
        let _ = io::copy(&mut output, &mut io::sink());
        assert!(now.elapsed() < Duration::from_secs(30), "it ran on");
    }

    /// A run came to its own end if its log has `engine.perf`, which a crash leaves out.
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

    /// The copies other runs left go, and the run's own, other games' and other files stay.
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

    /// The snapshots other runs of the game left go, and the run's own, other games' and other
    /// files stay.
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
        // Being written, then settled a second after its last change, and told of once.
        assert!(!watch.rebuilt(stamp(2, 50), at(5100)));
        assert!(!watch.rebuilt(stamp(2, 120), at(5600)));
        assert!(!watch.rebuilt(stamp(2, 120), at(6500)));
        assert!(watch.rebuilt(stamp(2, 120), at(6600)));
        assert!(!watch.rebuilt(stamp(2, 120), at(9000)));
        // Missing, as while it's replaced, which isn't a build however long it lasts, then back.
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
        // A process another test spawns meanwhile copies every open file, `first`'s too, and holds
        // them until it execs, as posix_spawn does on macOS and Linux: the lock comes free then.
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

    /// What a fake game, as [`faking`] serves it, was asked: each request's method and parameters.
    type Asked = Arc<std::sync::Mutex<Vec<(String, Value)>>>;

    /// What a fake game, as [`faking_with`] serves it, says it waits for, which a test may change.
    type Waits = Arc<std::sync::Mutex<Option<&'static str>>>;

    /// How a fake game answers `snapshot`.
    #[derive(Clone, Copy)]
    enum Snapshots {
        /// With a snapshot where it's asked.
        Taken,
        /// Refusing it with `api.not_ticked`.
        Refused,
        /// Never, as a game whose main thread a breakpoint holds.
        Never,
    }

    /// As [`faking_with`], waiting for `waiting` for good, and answering `snapshot` with a snapshot
    /// if `snapshots`, else refusing it.
    fn faking(waiting: Option<&'static str>, snapshots: bool) -> (Session, Asked) {
        let snapshots = if snapshots {
            Snapshots::Taken
        } else {
            Snapshots::Refused
        };
        faking_with(Arc::new(waiting.into()), snapshots)
    }

    /// Serves the tool API on the loopback as a game would, on a thread for each connection, while
    /// the tests run: welcoming every `hello`, answering `ping` as `waits` says as it comes,
    /// `snapshot` as `snapshots` says, and anything else with `{}`; and returns its session and
    /// what it's asked.
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
        };
        (session, asked)
    }

    /// The methods `asked` holds, but `hello`, each with whether it asked to quit.
    fn methods(asked: &Asked) -> Vec<(String, bool)> {
        let asked = asked.lock().expect("noted");
        let asked = asked.iter().filter(|(method, _)| method != "hello");
        let quits = |params: &Value| params["quit"].as_bool().unwrap_or(false);
        asked
            .map(|(method, params)| (method.clone(), quits(params)))
            .collect()
    }

    /// A game stopped to start again from a snapshot is snapshotted and asked to quit in one
    /// request, so that no edit lands between the two to be lost; one that refuses the snapshot is
    /// asked to quit alone after, as one stopped without a snapshot is.
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

    /// A game waiting at a breakpoint, in a file dialog or in the window menu holds its main thread,
    /// which answers neither a snapshot nor a quit; one minimized, or waiting for nothing, doesn't.
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
        // One that doesn't answer a ping at all, as one that has ended, isn't.
        let (mut gone, _) = faking(None, true);
        gone.address = "127.0.0.1:1".parse().expect("an address");
        assert_eq!(holding(&mut None, &gone), None);
    }

    /// A script that stops at a breakpoint while the game is asked for a snapshot ends the wait for
    /// it, which would otherwise wait out its time.
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

    /// A game waiting where it can't answer is killed at once as it's stopped, the snapshot in
    /// flight joined, and noted as the one it starts again from, which `killed` names.
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
            moved: now,
            pinged: now,
            waiting: None,
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

    /// A snapshot written whole since a time counts, as one whose answer was lost as the game
    /// ended; one written before, or cut short, doesn't.
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
        // Its own end ends the run, clean or not, as when validation reported something; a crash
        // or a hang does only with restarts off, or if no process of the run ever served.
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
        // The tool an embedded run is for ending ends it, cleanly.
        assert_eq!(next(Happened::Closed, Some(&latest), true), Next::End(true));
        // A rebuild starts it from the latest, or afresh if there's none; a refusal, afresh.
        assert_eq!(next(Happened::Rebuilt, Some(&latest), true), from_latest);
        assert_eq!(next(Happened::Rebuilt, None, true), Next::Start(None));
        assert_eq!(
            next(Happened::Refused, Some(&latest), true),
            Next::Start(None)
        );
    }
}
