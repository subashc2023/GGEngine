//! Running a game's development build: supervised, patched as its code changes, and started again
//! from a snapshot after a crash, a hang or a rebuild, until it exits; or headless for a set number
//! of frames, as a check.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use gg_api::{Client, Failure, Pulse, Session, Snapshot, Snapshotted};
use serde_json::{Value, json};

use crate::patch::{Patches, Process};
use crate::{build, game};

/// What `gg run` runs: a package, its profile, whether it's patched, unless `--no-patch`, how long
/// its frames may stop before it has hung, how often it's snapshotted, zero for never, and the
/// arguments it's given.
pub(crate) struct Run {
    pub(crate) package: String,
    pub(crate) release: bool,
    pub(crate) patches: bool,
    pub(crate) hang: Duration,
    pub(crate) snapshot: Duration,
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
    fn restarts(&self) -> bool {
        self.serves() && !self.given("--record") && !self.snapshot.is_zero()
    }

    /// Whether the game's arguments give flag `flag`, as `--flag` or `--flag=value`.
    fn given(&self, flag: &str) -> bool {
        self.args.iter().any(|arg| {
            (arg.strip_prefix(flag)).is_some_and(|rest| rest.is_empty() || rest.starts_with('='))
        })
    }
}

/// What `gg check` runs: a package, its profile, how many frames, how long it may take, and the
/// arguments it's given.
pub(crate) struct Check {
    pub(crate) package: String,
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

/// Builds `run`'s package, with dx while patches are on, as [`Patches::new`] says, and runs a copy
/// of its executable, so that it can be built again meanwhile, supervising the game until it exits
/// cleanly: returns whether it did, having printed a line for each event. While restarts are on, it
/// snapshots the game every so often as it serves the API, and starts it again from a snapshot
/// after a crash or a hang, and as its executable is rebuilt, as [`next`] says; with them off, the
/// game's first end ends it. While patches are on, it patches the game, or rebuilds it, as its
/// sources change, as [`Patches::tend`] says.
///
/// Fails with `gg.build` if it doesn't build, `gg.cargo` if cargo can't say what the package is,
/// and `gg.run` if it can't be copied, started or waited for.
pub(crate) fn run(run: &Run) -> Result<bool, Failure> {
    let patches = Patches::new(run)?;
    let exe = match &patches {
        Some(patches) => patches.exe().to_owned(),
        None => build::build(&run.package, run.release)?,
    };
    let dir = exe.parent().unwrap_or(Path::new(".")).to_owned();
    let snapshots = dir.join("snapshots");
    let restarts = run.restarts();
    if restarts {
        // A snapshot that can't be written there says why.
        let _ = fs::create_dir_all(&snapshots);
    }
    let pid = std::process::id();
    remove_leftovers(&exe, pid);
    let name = format!("{}-{pid}.snapshot", run.package);
    let mut supervisor = Supervisor {
        run,
        copy: running_copy(&exe, pid),
        exe,
        dir,
        snapshot: snapshots.join(name),
        serves: run.serves(),
        restarts,
        served: false,
        latest: None,
        failed: None,
        patches,
    };
    let supervised = supervisor.supervise();
    // Once its game has ended; one still running keeps it, and the patches' libraries it loaded.
    if retrying(|| remove(&supervisor.copy)).is_ok()
        && let Some(patches) = &supervisor.patches
    {
        patches.remove_libs();
    }
    supervised
}

/// What `gg run` keeps across the game's restarts: what it runs, the executable and the copy of it
/// that runs, where the game is snapshotted, whether a process of the game has served the API, the
/// last snapshot that succeeded, the code of the last snapshot failure printed, until one
/// succeeds, and the run's patches, while they're on.
struct Supervisor<'a> {
    run: &'a Run,
    exe: PathBuf,
    copy: PathBuf,
    /// The executable's directory, where the game writes its logs and session files.
    dir: PathBuf,
    snapshot: PathBuf,
    serves: bool,
    restarts: bool,
    served: bool,
    latest: Option<Snapshotted>,
    failed: Option<String>,
    patches: Option<Patches>,
}

/// A game's process under `gg run`, and what's known of it: the snapshot it started from, its
/// session once it serves, its frames and when they last moved, and its snapshots.
struct Game {
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
    /// whether it exited cleanly.
    fn supervise(&mut self) -> Result<bool, Failure> {
        // Taken before the first copy, so that a rebuild as it's made shows.
        let mut watch = Watch::new(stamp(&self.exe), Instant::now());
        // `None` while it waits for a rebuild.
        let mut game = Some(self.start(None)?);
        // Set once the executable is rebuilt, until the game starts again from the new build.
        let mut rebuilt = false;
        loop {
            let now = Instant::now();
            // While patches are on, the run makes the builds itself.
            if let Some(patches) = &mut self.patches {
                rebuilt |= patches.tend(game.as_ref().map(Game::process), now);
            } else if self.restarts && watch.rebuilt(stamp(&self.exe), now) {
                event(&json!({"event": "rebuilt", "exe": self.exe}));
                rebuilt = true;
            }
            let happened = match game.as_mut() {
                None => rebuilt.then_some(Happened::Rebuilt),
                Some(game) => match self.tend(game)? {
                    None if rebuilt => {
                        self.stop(game)?;
                        Some(Happened::Rebuilt)
                    }
                    happened => happened,
                },
            };
            let Some(happened) = happened else {
                thread::sleep(Duration::from_millis(100));
                continue;
            };
            match next(happened, self.latest.as_ref(), self.restarts) {
                Next::Start(from) => {
                    game = Some(self.start(from)?);
                    rebuilt = false;
                }
                Next::Wait(why) => {
                    event(&json!({"event": "waiting", "for": "rebuild", "why": why}));
                    if let Some(patches) = &mut self.patches {
                        patches.waiting();
                    }
                    game = None;
                }
                Next::End(clean) => {
                    if clean {
                        let _ = fs::remove_file(&self.snapshot);
                    }
                    return Ok(clean);
                }
            }
        }
    }

    /// Starts the game from snapshot `from`, or afresh: copies the executable again if it may have
    /// been rebuilt, as without patches it always may, runs the copy, and prints `started`.
    fn start(&mut self, from: Option<Snapshotted>) -> Result<Game, Failure> {
        if self.patches.as_mut().is_none_or(Patches::starting) {
            copy(&self.exe, &self.copy)?;
        }
        let mut command = Command::new(&self.copy);
        command.args(&self.run.args).stdin(Stdio::null());
        if let Some(from) = &from {
            command.arg("--restore").arg(&from.path);
        }
        let child = command.spawn().map_err(|error| failed(&self.copy, error))?;
        let pid = child.id();
        let restored =
            (from.as_ref()).map(|from| json!({"snapshot": from.path, "ticks": from.ticks}));
        event(&json!({
            "event": "started",
            "pid": pid,
            "exe": self.exe,
            "copy": self.copy,
            "restored": restored,
        }));
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

    /// Looks after `game` for a turn of the loop: finds its session once it serves, snapshots it,
    /// and pings it; and returns what happened if its process ended, or hung and was ended, having
    /// printed it.
    fn tend(&mut self, game: &mut Game) -> Result<Option<Happened>, Failure> {
        let ended = game.child.try_wait();
        if let Some(status) = ended.map_err(|error| failed(&self.copy, error))? {
            return Ok(Some(self.ended(game, status)));
        }
        if let Some(taking) = &game.snapshotting
            && taking.is_finished()
            && let Some(taken) = game.landed()
        {
            self.note(game, taken);
        }
        if self.serves && game.session.is_none() {
            let pid = game.pid;
            let found = Session::list(&Session::dir(&self.dir)).into_iter();
            game.session = found
                .map(|(_, session)| session)
                .find(|session| session.pid == pid);
            if let Some(session) = &game.session {
                event(&json!({
                    "event": "serving",
                    "pid": pid,
                    "app": session.app,
                    "address": session.address,
                    "log": session.log,
                }));
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
            let (session, path) = (session.clone(), self.snapshot.clone());
            game.snapshotting = Some(thread::spawn(move || snapshot(&session, &path)));
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
        let limit = if game.frames == 0 {
            STARTUP
        } else {
            self.run.hang
        };
        if game.moved.elapsed() <= limit {
            return Ok(None);
        }
        let _ = game.child.kill();
        let _ = game.child.wait();
        let (log, errors) = self.remains(game);
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
    fn ended(&mut self, game: &mut Game, status: ExitStatus) -> Happened {
        let (log, errors) = self.remains(game);
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
    fn remains(&mut self, game: &mut Game) -> (Option<PathBuf>, Vec<Value>) {
        if let Some(Ok(taken)) = game.landed() {
            self.note(game, Ok(taken));
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

    /// Stops `game` as its executable has been rebuilt: if it serves, snapshots it first, once a
    /// snapshot in flight is done, and asks it to quit, killing it if it hasn't ended within
    /// [`QUIT`]; if it doesn't serve yet, kills it.
    fn stop(&mut self, game: &mut Game) -> Result<(), Failure> {
        match game.session.clone() {
            Some(session) => {
                if let Some(taken) = game.landed() {
                    self.note(game, taken);
                }
                let taken = snapshot(&session, &self.snapshot);
                self.note(game, taken);
                if let Ok(mut client) = Client::connect(&session, PULSE) {
                    let _ = client.call("quit", Value::Null);
                }
            }
            None => {
                let _ = game.child.kill();
            }
        }
        wait(&mut game.child, QUIT).map_err(|error| failed(&self.copy, error))?;
        self.forget(game.pid);
        Ok(())
    }

    /// Notes what a snapshot of `game` gave, `taken`: the snapshot, which restarts start from; or
    /// the failure, printed unless the last one printed had its code.
    fn note(&mut self, game: &mut Game, taken: Result<Snapshotted, Failure>) {
        match taken {
            Ok(taken) => {
                game.snapshotted = true;
                self.latest = Some(taken);
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
}

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

/// Where the `gg run` of process `pid` copies executable `exe` to run it: beside it, so that what
/// the game writes beside its executable stays where it is, and named for the process, so that
/// two runs of one game, as a networked game's peers are, each have one, as
/// `gg-sandbox-running-42.exe` for `gg-sandbox.exe`.
fn running_copy(exe: &Path, pid: u32) -> PathBuf {
    let mut name = exe.file_stem().unwrap_or_default().to_owned();
    name.push(format!("-running-{pid}"));
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
        let theirs = rest.split('.').next().and_then(|pid| pid.parse().ok());
        if let Some(theirs) = theirs.filter(|&theirs| theirs != pid)
            && running_copy(exe, theirs) == entry.path()
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
/// waits as long as a large world's snapshot may take, and returns what it answers.
fn snapshot(session: &Session, path: &Path) -> Result<Snapshotted, Failure> {
    let path = Some(path.to_owned());
    Client::connect(session, SNAPSHOT)?.call_as("snapshot", Snapshot { path })
}

/// The pulse of the game `session` names, through `client`, which this connects again once it
/// breaks; `None` if the game doesn't answer in time.
fn ping(client: &mut Option<Client>, session: &Session) -> Option<Pulse> {
    if client.is_none() {
        *client = Client::connect(session, PULSE).ok();
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
/// Fails with `gg.build` if it doesn't build, and `gg.run` if it can't start or be waited for.
pub(crate) fn check(check: &Check) -> Result<bool, Failure> {
    let exe = build::build(&check.package, check.release)?;
    let frames = check.frames.to_string();
    // Its console says what its JSON log does, which is read below.
    let mut child = Command::new(&exe)
        .args(["--headless", "--frames", &frames, "--no-api"])
        .args(&check.args)
        .stdin(Stdio::null())
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
            release: false,
            patches: true,
            hang: Duration::from_secs(10),
            snapshot: Duration::from_secs(snapshot),
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
        let copy = running_copy(Path::new("target/debug/gg-sandbox.exe"), 42);
        assert_eq!(copy, Path::new("target/debug/gg-sandbox-running-42.exe"));
        let copy = running_copy(Path::new("target/debug/gg-sandbox"), 42);
        assert_eq!(copy, Path::new("target/debug/gg-sandbox-running-42"));
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
            "gg-sandbox-running-42.exe",
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
        assert_eq!(left, [false, true, true, true, true]);
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
        // A rebuild starts it from the latest, or afresh if there's none; a refusal, afresh.
        assert_eq!(next(Happened::Rebuilt, Some(&latest), true), from_latest);
        assert_eq!(next(Happened::Rebuilt, None, true), Next::Start(None));
        assert_eq!(
            next(Happened::Refused, Some(&latest), true),
            Next::Start(None)
        );
    }
}
