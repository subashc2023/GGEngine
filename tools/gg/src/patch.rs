//! `gg run`'s patches, as ARCHITECTURE.md's Game code describes: building a game's development
//! build with dx, so that it can take patches; watching its sources; and as they change, building a
//! patch of an edit inside function bodies of its own modules and having the running game apply it
//! through the tool API, or rebuilding the game, which the run then starts again from a snapshot;
//! and telling the game, through the API's `note`, what it should log of them, so that a client
//! following its logs learns of a patch or a rebuild that failed, or of a patch that won't show.
//! Without dx, or where patches can't reach the game's code, it builds the game with cargo and
//! watches its sources all the same, every change rebuilding it.

use std::collections::BTreeMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::iter;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gg_api::{Client, Failure, Note, NoteLevel, Patch, Patched, Session, UNBUILT};
use serde_json::{Value, json};

use crate::build::{self, Package, fit};
use crate::classify::{self, Edit};
use crate::dx::{self, Dx, Fat};
use crate::run::{self, Run, Stamp, event, millis};
use crate::runs::State;

/// How long the sources must stay the same after a change before it's acted on, as an editor or an
/// agent may save several files at once.
const QUIET: Duration = Duration::from_millis(200);

/// How long connecting to the game for a patch may take. Its answer is waited for as long as the
/// process lives, as a file dialog or the window menu may hold its main thread, which applies it,
/// for minutes; a process that hangs is ended, which closes the connection.
const CONNECT: Duration = Duration::from_secs(10);

/// Why a change to a file that isn't one of the game's modules rebuilds it.
const OUTSIDE: &str = "a change outside the game's crate";

/// Why an edit a patch would carry rebuilds a game built without patches.
const UNPATCHED: &str =
    "the run builds the game with cargo, without patches, as `patches_off` said";

/// The stamp of each file watched, by its path.
type Stamps = BTreeMap<PathBuf, Stamp>;

/// A hash of each of the game's modules' text, by its path.
type Hashes = BTreeMap<PathBuf, u64>;

/// A run's patches: how dx builds the game, and what's watched; the build the game runs, which
/// patches are built against, and its sources as it started; a rebuild that succeeded, which the
/// game runs once it starts again; whether one failed since, and whether the game has started from
/// the build; the processes that serve, and what of the sources their code holds; the sources as
/// last scanned and as last looked at; the build or patch being made; and the libraries the run's
/// patches made.
pub(crate) struct Patches {
    builder: Builder,
    sources: Sources,
    fat: Fat,
    baseline: Baseline,
    rebuilt: Option<(Fat, Baseline)>,
    /// Whether a rebuild failed since the build the game runs, which may have left its packages'
    /// libraries, which patches build with, other than those the build linked: until one succeeds,
    /// a change that would patch the game rebuilds it.
    stale: bool,
    started: bool,
    /// The game's processes, once every one serves the API, a lockstep session's peers each one,
    /// and the hashes of the game's modules as of the code the processes last started run or ran:
    /// the build's, with the patches they applied, all alike.
    processes: Vec<u32>,
    applied: Hashes,
    /// Whether the sources are to be looked at as soon as they're quiet, changed or not: as a
    /// process starts serving, or the run starts waiting for a rebuild.
    fresh: bool,
    /// The stamps last scanned, when they last changed, and the stamps last looked at.
    seen: Stamps,
    since: Instant,
    looked: Stamps,
    job: Option<Job>,
    libs: Vec<PathBuf>,
    /// The failure of the last rebuild or patch, until one succeeds, or a look at the sources
    /// finds the game's processes run what they hold.
    failed: Option<Failure>,
}

/// How a run's builds are made: with dx, which patches are built against; or with cargo, run in a
/// directory, optimized or not, for a game patches can't reach, whose every change rebuilds it.
#[derive(Clone)]
enum Builder {
    Dx(Dx),
    Cargo {
        package: String,
        release: bool,
        dir: Option<PathBuf>,
    },
}

impl Builder {
    /// Builds the game, as [`Dx::build`] or [`build::build`] does.
    fn build(&self) -> Result<Fat, Failure> {
        match self {
            Builder::Dx(dx) => dx.build(),
            Builder::Cargo {
                package,
                release,
                dir,
            } => {
                let args: &[&str] = if *release { &["--release"] } else { &[] };
                build::build(package, args, dir.as_deref()).map(Fat::unpatchable)
            }
        }
    }
}

/// What patches see of the game's process: its ID, and its session once it serves the API.
#[derive(Clone, Copy)]
pub(crate) struct Process<'a> {
    pub(crate) pid: u32,
    pub(crate) session: Option<&'a Session>,
}

impl Patches {
    /// Builds `run`'s game with dx and watches its sources, if patches are on for it: dx is on the
    /// PATH, of the release of the subsecond the game's build uses, the game's package has no
    /// library beside its binary, whose code no patch would reach, as subsecond patches the
    /// binary's crate alone, and the run's own options don't turn them off, as [`Run::unpatched`]
    /// says. If they're off, prints `patches_off`, saying why; and then, unless the run's options
    /// turn restarts off, or `--no-patch` asks to be left to build the game itself, builds it with
    /// cargo and watches its sources all the same, each change rebuilding it; else returns `None`,
    /// for the game to be built with cargo.
    ///
    /// Fails with `gg.cargo` if cargo can't say what the package is, and `gg.build` if the
    /// workspace has no such package, or it doesn't build.
    pub(crate) fn new(run: &Run) -> Result<Option<Self>, Failure> {
        let off = |code: &str, why: &str| {
            event(&json!({"event": "patches_off", "code": code, "why": why}));
        };
        let watches = run.patches && run.restarts();
        if let Some(why) = run.unpatched()
            && !(watches && run.release)
        {
            off("gg.no_patch", why);
            return Ok(None);
        }
        let package = build::package(&run.package, run.dir())?;
        let unpatchable = if run.release {
            run.unpatched()
                .map(|why| ("gg.no_patch".to_owned(), why.to_owned()))
        } else if package.library {
            let why = format!(
                "package `{}` has a library beside its binary, and subsecond patches the binary's \
                 crate alone",
                run.package
            );
            Some(("gg.library".to_owned(), why))
        } else {
            match &package.subsecond {
                None => {
                    let why = format!(
                        "package `{}` doesn't build with subsecond, which applies patches",
                        run.package
                    );
                    Some(("gg.no_subsecond".to_owned(), why))
                }
                Some(subsecond) => dx::check(subsecond)
                    .err()
                    .map(|failure| (failure.code, failure.message)),
            }
        };
        let builder = match unpatchable {
            None => Builder::Dx(Dx::new(&run.package, &package)),
            Some((code, why)) => {
                off(&code, &why);
                Builder::Cargo {
                    package: run.package.clone(),
                    release: run.release,
                    dir: run.dir().map(Path::to_owned),
                }
            }
        };
        let sources = Sources::new(&package);
        let patches = matches!(builder, Builder::Dx(_));
        event(&json!({"event": "watching", "paths": sources.paths(), "patches": patches}));
        let mut baseline = Baseline::new(&sources);
        let fat = builder.build().map_err(|failure| {
            let message = match &builder {
                Builder::Dx(_) => format!("{}; `--no-patch` builds with cargo", failure.message),
                Builder::Cargo { .. } => failure.message,
            };
            Failure::new(failure.code, message)
        })?;
        baseline.refresh(&sources.lock);
        let seen = baseline.stamps();
        Ok(Some(Self {
            builder,
            sources,
            fat,
            rebuilt: None,
            stale: false,
            started: false,
            processes: Vec::new(),
            applied: baseline.modules.clone(),
            fresh: false,
            looked: seen.clone(),
            seen,
            since: Instant::now(),
            job: None,
            libs: Vec::new(),
            failed: None,
            baseline,
        }))
    }

    /// What the run is doing, as far as its patches tell, given the game's processes, as
    /// [`State`] has it: rebuilding, patching, restarting once rebuilt, starting while a process
    /// doesn't serve yet, `changed` once the sources have changed and it hasn't acted on it, and
    /// else running, the processes' code current but for a rebuild or a patch that failed; or
    /// waiting while there are none, as for a rebuild.
    pub(crate) fn state(&self, games: &[Process]) -> State {
        let mut state = State::new("running");
        state.pids = games.iter().map(|game| game.pid).collect();
        state.error.clone_from(&self.failed);
        state.phase = match &self.job {
            Some(Job::Rebuild { why, .. }) => {
                state.why = Some(why.clone());
                "rebuilding"
            }
            Some(Job::Patch { .. }) => "patching",
            None if self.rebuilt.is_some() => "restarting",
            None if games.is_empty() => "waiting",
            None if games.iter().any(|game| game.session.is_none()) => "starting",
            None if self.fresh || self.seen != self.looked => "changed",
            None => "running",
        };
        state.current = Some(state.phase == "running" && self.failed.is_none());
        state
    }

    /// The executable of the build the game runs.
    pub(crate) fn exe(&self) -> &Path {
        &self.fat.exe
    }

    /// Looks after the patches for a turn of the run's loop, given the game's processes, none while
    /// it waits for a rebuild: notes the build or patch that's done, scans the sources, and looks at
    /// them, as [`look`](Self::look) says, once they've stayed the same for [`QUIET`] since they
    /// changed, or the processes all came to serve. Returns whether a rebuild succeeded, which the
    /// run then starts the game again from.
    pub(crate) fn tend(&mut self, games: &[Process], now: Instant) -> bool {
        let rebuilt = self.job.as_ref().is_some_and(Job::done) && self.land(games);
        let stamps = self.sources.scan();
        if stamps != self.seen {
            (self.seen, self.since) = (stamps, now);
        }
        let serving = games.iter().all(|game| game.session.is_some());
        if !games.is_empty()
            && serving
            && !games
                .iter()
                .map(|game| game.pid)
                .eq(self.processes.iter().copied())
        {
            self.processes = games.iter().map(|game| game.pid).collect();
            self.fresh = true;
        }
        let due = self.fresh || self.seen != self.looked;
        let quiet = now.duration_since(self.since) >= QUIET;
        if due && quiet && self.job.is_none() && self.rebuilt.is_none() {
            self.look(games);
        }
        rebuilt
    }

    /// Notes that the game starts, as new processes, which run the build's code: the rebuild's that
    /// succeeded, if one did, whose sources edits are compared with from then on. Returns whether
    /// the build is another than the last processes ran, as at the first start, whose executable the
    /// run copies again; processes started again from the same build run the copies the last ones
    /// ran, as a rebuild under way may be writing the build's executable.
    pub(crate) fn starting(&mut self) -> bool {
        let mut built = !std::mem::replace(&mut self.started, true);
        if let Some((fat, baseline)) = self.rebuilt.take() {
            (self.fat, self.baseline) = (fat, baseline);
            self.stale = false;
            built = true;
        }
        self.processes.clear();
        self.applied = self.baseline.modules.clone();
        built
    }

    /// Notes that the run waits for a rebuild, the game's processes having ended: the sources are
    /// looked at as soon as they're quiet, and the game rebuilt if they differ from what those
    /// processes ran.
    pub(crate) fn waiting(&mut self) {
        self.processes.clear();
        self.fresh = true;
    }

    /// Waits for the build or patch being made, if one is, as the run ends: so that dx removes what
    /// it wrote of the environment as it built, and no build outlives the run's lock.
    pub(crate) fn finish(&mut self) {
        match self.job.take() {
            Some(Job::Rebuild { thread, .. }) => drop(thread.join()),
            Some(Job::Patch { thread, .. }) => drop(thread.join()),
            None => {}
        }
    }

    /// Removes the libraries the run's patches made, once the processes that loaded them have
    /// ended, with what the linker wrote beside each on Windows: its import library, its exports
    /// and its PDB, which is the largest.
    pub(crate) fn remove_libs(&self) {
        for lib in &self.libs {
            let beside = ["lib", "exp", "pdb"].map(|extension| lib.with_extension(extension));
            for path in iter::once(lib.clone()).chain(beside) {
                let _ = run::retrying(|| run::remove(&path));
            }
        }
    }

    /// Looks at the sources as they are, and builds a patch for the game's processes or rebuilds the
    /// game, as [`act`] says.
    fn look(&mut self, games: &[Process]) {
        self.looked = self.seen.clone();
        self.fresh = false;
        let diff = diff(&self.sources, &self.baseline, &self.seen);
        let patches = matches!(self.builder, Builder::Dx(_));
        match act(diff, games, &self.applied, self.stale, patches) {
            Act::Nothing => {
                // The processes run what the sources hold, once each serves.
                if !games.is_empty()
                    && games.iter().all(|game| game.session.is_some())
                    && let Some(failure) = self.failed.take()
                {
                    event(&recovered(&failure));
                }
            }
            Act::Rebuild(file, why) => self.rebuild(&file, why),
            Act::Patch(files, hashes, unreadable, attach) => {
                let serving = games
                    .iter()
                    .filter_map(|game| Some((game.pid, game.session?.clone())));
                self.patch(serving.collect(), files, hashes, unreadable, attach);
            }
        }
    }

    /// Rebuilds the game on a thread of its own, having said why: a change to `file`, for `why`.
    fn rebuild(&mut self, file: &Path, why: &str) {
        event(&json!({"event": "rebuilding", "file": file, "why": why}));
        let (builder, sources) = (self.builder.clone(), self.sources.clone());
        let thread = thread::spawn(move || {
            // The sources as the build starts, which edits made as it runs then differ from, but for
            // the lock file, which the build itself writes as a manifest's dependencies change.
            let mut baseline = Baseline::new(&sources);
            let fat = builder.build()?;
            baseline.refresh(&sources.lock);
            Ok((fat, baseline))
        });
        let started = Instant::now();
        let why = format!("a change to {}: {why}", file.display());
        self.job = Some(Job::Rebuild {
            started,
            why,
            thread,
        });
    }

    /// Builds a patch of edits to `files` for each of `processes`, each its ID and its session, as
    /// each is built against where its process's `main` is, and has the processes apply them, at
    /// once, as a lockstep session's joiner waits for its own at the tick its host's applies
    /// before, on a thread of its own; once they have, their modules' hashes are `hashes`. One whose sources changed as it
    /// built is dropped, as it may hold what wasn't looked at; and with `unreadable`, a module whose
    /// text gg can't read, one that builds isn't applied. `attach` names the modules among `files`
    /// whose layers' `attach` differs from the build's, which the patch tells of as it applies.
    fn patch(
        &mut self,
        processes: Vec<(u32, Session)>,
        files: Vec<PathBuf>,
        hashes: Hashes,
        unreadable: Option<PathBuf>,
        attach: Vec<PathBuf>,
    ) {
        let Builder::Dx(dx) = &self.builder else {
            let file = files.first().cloned().unwrap_or_default();
            return self.rebuild(&file, UNPATCHED);
        };
        let (dx, fat) = (dx.clone(), self.fat.clone());
        let (sources, looked, checked) = (
            self.sources.clone(),
            self.looked.clone(),
            unreadable.is_none(),
        );
        let pids = processes.iter().map(|(pid, _)| *pid).collect();
        let thread = thread::spawn(move || {
            let start = Instant::now();
            let mut tables = Vec::new();
            let mut libs = Vec::new();
            let mut failed = None;
            for (pid, session) in processes {
                let table = dx.hotpatch(&fat, session.aslr_reference);
                let lib = table.as_ref().ok().and_then(|table| table["lib"].as_str());
                libs.extend(lib.map(PathBuf::from));
                match table {
                    Ok(table) => tables.push((pid, session, table)),
                    Err(failure) => {
                        failed = Some(failure);
                        break;
                    }
                }
            }
            let built = start.elapsed();
            let outcomes = match failed {
                Some(failure) => vec![(None, Outcome::Failed(failure))],
                // dx built what the files held as it read them, which an edit saved since they were
                // looked at may have changed.
                None if sources.scan() != looked => vec![(None, Outcome::Superseded)],
                None if !checked => vec![(None, Outcome::Unreadable)],
                None => {
                    let applying = tables.into_iter().map(|(pid, session, table)| {
                        let thread = thread::spawn(move || {
                            let start = Instant::now();
                            let applied =
                                Client::connect(&session, "gg", CONNECT).and_then(|mut client| {
                                    client.set_timeout(None)?;
                                    client.call_as::<Patched>("patch", Patch { table })
                                });
                            match applied {
                                Ok(patched) => Outcome::Applied(patched, start.elapsed()),
                                Err(failure) => Outcome::Failed(failure),
                            }
                        });
                        (pid, thread)
                    });
                    let applying: Vec<_> = applying.collect();
                    let panicked = || Failure::new("gg.patch", "the thread sending it panicked");
                    let joined = applying.into_iter().map(|(pid, thread)| {
                        let outcome = thread
                            .join()
                            .unwrap_or_else(|_| Outcome::Failed(panicked()));
                        (Some(pid), outcome)
                    });
                    joined.collect()
                }
            };
            Patching {
                libs,
                built,
                outcomes,
            }
        });
        self.job = Some(Job::Patch {
            pids,
            files,
            hashes,
            unreadable,
            attach,
            thread,
        });
    }

    /// Notes what the job that's done gave, printing it, and returns whether it was a rebuild that
    /// succeeded; and has the game's processes, `games`, log a rebuild or a patch that failed, and
    /// a patch that changed a layer's `attach`, as [`note`] says.
    fn land(&mut self, games: &[Process]) -> bool {
        let panicked = || Failure::new("gg.build", "the thread making it panicked");
        // The sessions of the processes that serve: process `pid`'s, or all of them.
        let serving = |pid: Option<u32>| -> Vec<Session> {
            let games = games
                .iter()
                .filter(|game| pid.is_none_or(|pid| game.pid == pid));
            games.filter_map(|game| game.session.cloned()).collect()
        };
        match self.job.take() {
            Some(Job::Rebuild {
                started, thread, ..
            }) => match thread.join().unwrap_or_else(|_| Err(panicked())) {
                Ok((fat, baseline)) => {
                    let ms = millis(started.elapsed());
                    event(&json!({"event": "rebuilt", "exe": fat.exe, "build_ms": ms}));
                    self.rebuilt = Some((fat, baseline));
                    self.failed = None;
                    true
                }
                Err(failure) => {
                    event(&json!({"event": "rebuild_failed", "error": error(&failure)}));
                    let message = format!(
                        "A rebuild of the game failed, so it runs on as it was built, and each \
                             change rebuilds it until one succeeds: {}",
                        failure.message
                    );
                    let fields = json!({"error": failure.code});
                    note(serving(None), "gg.rebuild_failed", &message, &fields);
                    self.stale = true;
                    self.failed = Some(failure);
                    false
                }
            },
            Some(Job::Patch {
                pids,
                files,
                hashes,
                unreadable,
                attach,
                thread,
            }) => {
                let patching = thread.join().unwrap_or_else(|_| Patching {
                    libs: Vec::new(),
                    built: Duration::ZERO,
                    outcomes: vec![(None, Outcome::Failed(panicked()))],
                });
                self.libs.extend(patching.libs);
                let current = self.processes == pids;
                let all = patching.outcomes.len() == pids.len()
                    && (patching.outcomes.iter())
                        .all(|(_, outcome)| matches!(outcome, Outcome::Applied(..)));
                if all && current {
                    self.applied = hashes;
                    self.failed = None;
                }
                for (pid, outcome) in patching.outcomes {
                    let to = serving(pid);
                    // A failure to build is every process's: a game alone's, its ID.
                    let pid = match (pid, &pids[..]) {
                        (Some(pid), _) => json!(pid),
                        (None, [pid]) => json!(pid),
                        (None, pids) => json!(pids),
                    };
                    match outcome {
                        Outcome::Applied(patched, applied) => {
                            event(&json!({
                                "event": "patched",
                                "pid": pid,
                                "files": files,
                                "attach": attach,
                                "build_ms": millis(patching.built),
                                "apply_ms": millis(applied),
                                "ticks": patched.ticks,
                            }));
                            if !attach.is_empty() {
                                let message = format!(
                                    "A patch changed a layer's `attach` in {}, which builds the \
                                     world as a run starts afresh; this run keeps its world, as \
                                     its restarts start from snapshots, so a run started afresh, \
                                     as stopping `gg run` and starting it again starts one, shows \
                                     the change",
                                    listed(&attach)
                                );
                                let fields = json!({"files": attach});
                                note(to, "gg.attach_changed", &message, &fields);
                            }
                        }
                        Outcome::Failed(failure)
                            if matches!(failure.code.as_str(), "api.closed" | "api.timeout") =>
                        {
                            // Sent, but its answer never came: it built, and the process may yet
                            // apply it, or may have, so what it runs isn't known; the sources are
                            // looked at again at once, patching the processes, once each serves,
                            // with every edit since their build, which a process that ended gets
                            // as it starts again.
                            if current {
                                self.applied.clear();
                                self.fresh = true;
                            }
                            let message = format!(
                                "The patch of {} built, but the game's answer to it never came, so \
                                 it may or may not run it; gg patches it again with every edit \
                                 since its build: {}",
                                listed(&files),
                                failure.message
                            );
                            let lost = Failure::new("gg.patch_unanswered", message);
                            event(&json!({
                                "event": "patch_unanswered",
                                "pid": pid,
                                "files": files,
                                "error": error(&lost),
                            }));
                            let fields = json!({"files": files, "error": failure.code});
                            note(to, &lost.code, &lost.message, &fields);
                        }
                        Outcome::Failed(failure) => {
                            event(&json!({
                                "event": "patch_failed",
                                "pid": pid,
                                "files": files,
                                "error": error(&failure),
                            }));
                            let message = format!(
                                "A patch of {} failed, so the game runs its code as it was: {}",
                                listed(&files),
                                failure.message
                            );
                            let fields = json!({"files": files, "error": failure.code});
                            note(to, "gg.patch_failed", &message, &fields);
                            self.failed = Some(failure);
                        }
                        Outcome::Superseded => {}
                        Outcome::Unreadable => {
                            let file = unreadable.clone().unwrap_or_default();
                            self.rebuild(&file, "an edit gg can't read, which rustc builds");
                        }
                    }
                }
                false
            }
            None => false,
        }
    }
}

/// What a run watches of a game's sources while it has no build, its first having failed: the
/// sources as cargo says of its package, and their stamps as last scanned, and when they last
/// changed.
pub(crate) struct Watcher {
    sources: Sources,
    seen: Stamps,
    since: Instant,
    /// Whether they've changed since it started watching.
    changed: bool,
}

impl Watcher {
    /// Watches the sources of `run`'s game as they are.
    ///
    /// Fails with `gg.cargo` if cargo can't say what the package is, and `gg.build` if the
    /// workspace has no such package.
    pub(crate) fn new(run: &Run) -> Result<Self, Failure> {
        let sources = Sources::new(&build::package(&run.package, run.dir())?);
        let seen = sources.scan();
        Ok(Self {
            sources,
            seen,
            since: Instant::now(),
            changed: false,
        })
    }

    /// Scans the sources at `now`, and returns whether they've changed since it started watching
    /// and have stayed the same for [`QUIET`] since, as an edit saved whole.
    pub(crate) fn changed(&mut self, now: Instant) -> bool {
        let stamps = self.sources.scan();
        if stamps != self.seen {
            (self.seen, self.since, self.changed) = (stamps, now, true);
        }
        self.changed && now.duration_since(self.since) >= QUIET
    }
}

/// A build or a patch being made, on a thread of its own, so that the game is looked after
/// meanwhile.
enum Job {
    /// A rebuild, started at `started` for `why`, which gives the build and its sources as it
    /// started.
    Rebuild {
        started: Instant,
        why: String,
        thread: JoinHandle<Result<(Fat, Baseline), Failure>>,
    },
    /// A patch of edits to `files` for processes `pids`, after which the hashes of the game's
    /// modules are `hashes`, the module whose text gg can't read, if one is, and those whose
    /// layers' `attach` differs from the build's.
    Patch {
        pids: Vec<u32>,
        files: Vec<PathBuf>,
        hashes: Hashes,
        unreadable: Option<PathBuf>,
        attach: Vec<PathBuf>,
        thread: JoinHandle<Patching>,
    },
}

impl Job {
    /// Whether its thread is done.
    fn done(&self) -> bool {
        match self {
            Job::Rebuild { thread, .. } => thread.is_finished(),
            Job::Patch { thread, .. } => thread.is_finished(),
        }
    }
}

/// How a patch went: the libraries it made, one for each process it was built for, how long
/// building them took, and what came of it for each process, or for all of them, `None`.
struct Patching {
    libs: Vec<PathBuf>,
    built: Duration,
    outcomes: Vec<(Option<u32>, Outcome)>,
}

/// What came of a patch.
enum Outcome {
    /// The game applied it, answering so, in this long.
    Applied(Patched, Duration),
    /// It didn't build, or the game didn't apply it.
    Failed(Failure),
    /// The sources changed as it was built, so it was dropped, to be looked at again.
    Superseded,
    /// It built a module whose text gg can't read, so it wasn't applied, and the game is rebuilt.
    Unreadable,
}

/// How the sources differ from a build's.
enum Diff {
    /// In what a patch can't carry: a change to a file, for a reason.
    Rebuild(PathBuf, &'static str),
    /// In function bodies of the game's modules, if at all: the hash of each module, a module whose
    /// text gg can't read, if one is, and the modules whose layers' `attach` differs from the
    /// build's, in the order of their paths.
    Bodies(Hashes, Option<PathBuf>, Vec<PathBuf>),
}

/// What the sources call for.
#[derive(Debug, PartialEq)]
enum Act {
    /// Nothing: the game's process runs what they hold, or takes a patch of it once it serves.
    Nothing,
    /// A patch of edits to these modules, after which the hash of each module is as given, the
    /// module whose text gg can't read, if one is, and those of the modules patched whose layers'
    /// `attach` differs from the build's.
    Patch(Vec<PathBuf>, Hashes, Option<PathBuf>, Vec<PathBuf>),
    /// A rebuild, for a change to a file, for a reason.
    Rebuild(PathBuf, &'static str),
}

/// What sources that differ from the build's as `diff` says call for, given the game's processes,
/// none while it waits for a rebuild, the hashes of the game's modules as of the code they run, or
/// the last ones ran, `applied`, and whether a rebuild failed since the build, `stale`. A change a
/// patch can't carry rebuilds the game. Otherwise processes that all serve take a patch of what
/// changed since what they run, unless one can't take patches, as one whose executable doesn't
/// export its `main` can't, or the build is stale; while one doesn't serve yet, they take one once
/// it does; and a game whose processes crashed or hung, which waits for a rebuild, gets one at any
/// change since what they ran, as a patch made after they start again could come too late. A patch
/// tells of the modules it carries whose layers' `attach` differs from the build's: of each again
/// at each edit of it, as gg keeps the build's text alone, which keeping the text the processes run
/// too would tell of once. Without `patches`, as for a game built with cargo, what a patch would
/// carry rebuilds it.
fn act(diff: Diff, games: &[Process], applied: &Hashes, stale: bool, patches: bool) -> Act {
    let (hashes, unreadable, attach) = match diff {
        Diff::Rebuild(file, why) => return Act::Rebuild(file, why),
        Diff::Bodies(hashes, unreadable, attach) => (hashes, unreadable, attach),
    };
    let sessions: Option<Vec<&Session>> = games.iter().map(|game| game.session).collect();
    match sessions {
        _ if games.is_empty() => match changed(applied, &hashes).into_iter().next() {
            Some(file) => Act::Rebuild(file, "the game waits for a rebuild"),
            None => Act::Nothing,
        },
        None => Act::Nothing,
        Some(sessions) => {
            let files = changed(applied, &hashes);
            let Some(file) = files.first() else {
                return Act::Nothing;
            };
            if !patches {
                return Act::Rebuild(file.clone(), UNPATCHED);
            }
            if sessions.iter().any(|session| session.aslr_reference == 0) {
                let why = "the game can't take patches, as its executable exports no `main`";
                return Act::Rebuild(file.clone(), why);
            }
            if stale {
                let why = "a rebuild failed since the game's build, and patches would build with \
                           what it left";
                return Act::Rebuild(file.clone(), why);
            }
            let attach = attach.into_iter().filter(|file| files.contains(file));
            let attach = attach.collect();
            Act::Patch(files, hashes, unreadable, attach)
        }
    }
}

/// The modules whose hash in `to` differs from theirs in `from`.
fn changed(from: &Hashes, to: &Hashes) -> Vec<PathBuf> {
    let changed = (to.iter()).filter(|(path, hash)| from.get(*path) != Some(*hash));
    changed.map(|(path, _)| path.clone()).collect()
}

/// How the sources, whose stamps are `stamps`, differ from those of the build whose sources were
/// `baseline`. A file added or removed, or a change outside function bodies of the game's modules,
/// as [`classify::classify`] tells, needs a rebuild, the first in the order of their paths being the
/// one given; and it tells which modules' layers' `attach` differs from the build's. A file whose
/// stamp changed but not its content hasn't changed.
fn diff(sources: &Sources, baseline: &Baseline, stamps: &Stamps) -> Diff {
    let added = (stamps.keys()).filter(|path| !baseline.files.contains_key(*path));
    let removed = (baseline.files.keys()).filter(|path| !stamps.contains_key(*path));
    if let Some(file) = added.chain(removed).next() {
        let why = if sources.module(file) {
            "a module added or removed"
        } else {
            OUTSIDE
        };
        return Diff::Rebuild(file.clone(), why);
    }
    let (mut hashes, mut unreadable, mut attach) = (Hashes::new(), None, Vec::new());
    for (path, stamp) in stamps {
        let known = &baseline.files[path];
        let module = sources.module(path);
        let mut hash = known.hash;
        if *stamp != known.stamp {
            let Ok(content) = fs::read(path) else {
                return Diff::Rebuild(path.clone(), "a file gg can't read");
            };
            hash = hash_of(&content);
            if hash != known.hash {
                // Only the game's modules have their text kept.
                let edit = match &known.text {
                    Some(old) => classify::classify(old, &String::from_utf8_lossy(&content)),
                    None if module => Edit::Rebuild("an edit to a module that isn't UTF-8"),
                    None => Edit::Rebuild(OUTSIDE),
                };
                match edit {
                    Edit::Bodies { attach: false } => {}
                    Edit::Bodies { attach: true } => attach.push(path.clone()),
                    Edit::Unreadable => unreadable = Some(path.clone()),
                    Edit::Rebuild(why) => return Diff::Rebuild(path.clone(), why),
                }
            }
        }
        if module {
            hashes.insert(path.clone(), hash);
        }
    }
    Diff::Bodies(hashes, unreadable, attach)
}

/// What a run's patches watch, as cargo says of the game's package: the game's modules, the `.rs`
/// files under its binary's root module's directory, which a patch may carry an edit of; and what a
/// change to rebuilds the game: the files of the directories of the workspace's packages it depends
/// on, the workspace's manifest and lock file, and the game's manifest and build script. In either
/// directory, what's in directories no build of the package reads doesn't count, as [`walk`] says.
#[derive(Clone)]
struct Sources {
    modules: PathBuf,
    /// The game's package's directory, which its modules' directory may be.
    game: PathBuf,
    dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
    /// The workspace's lock file, among `files`.
    lock: PathBuf,
}

impl Sources {
    fn new(package: &Package) -> Self {
        let lock = package.root.join("Cargo.lock");
        let mut files = vec![
            package.root.join("Cargo.toml"),
            lock.clone(),
            package.manifest.clone(),
        ];
        files.extend(package.build_script.clone());
        files.sort();
        files.dedup();
        let game = package.manifest.parent().map(Path::to_owned);
        Self {
            modules: package.modules.clone(),
            game: game.unwrap_or_default(),
            dirs: package.deps.clone(),
            files,
            lock,
        }
    }

    /// Where it looks: the game's modules' directory, the packages' directories, and the files.
    fn paths(&self) -> Vec<&Path> {
        let paths = iter::once(&self.modules)
            .chain(&self.dirs)
            .chain(&self.files);
        paths.map(PathBuf::as_path).collect()
    }

    /// Whether the file at `path`, one it watches, is one of the game's modules.
    fn module(&self, path: &Path) -> bool {
        path.starts_with(&self.modules)
            && path.extension().is_some_and(|extension| extension == "rs")
            && !self.files.iter().any(|file| file == path)
    }

    /// The stamp of each file it watches, as the file system has it.
    fn scan(&self) -> Stamps {
        let mut stamps = Stamps::new();
        let root = self.modules == self.game;
        walk(
            &self.modules,
            root,
            &|path: &Path| self.module(path),
            &mut stamps,
        );
        for dir in &self.dirs {
            walk(dir, true, &|_: &Path| true, &mut stamps);
        }
        for file in &self.files {
            if let Ok(metadata) = fs::metadata(file) {
                stamps.insert(file.clone(), run::stamped(metadata));
            }
        }
        stamps
    }
}

/// Adds the stamp of each file under directory `dir` that `keep` keeps to `stamps`, but for hidden
/// files and directories, as an editor's locks, backups whose names end in `~`, other packages'
/// directories, and with `root`, `dir` being a package's own, its directories that no build of it
/// reads, [`UNBUILT`].
fn walk(dir: &Path, root: bool, keep: &impl Fn(&Path) -> bool, stamps: &mut Stamps) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let (path, name) = (entry.path(), entry.file_name());
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if name.starts_with('.') || name.ends_with('~') {
            continue;
        }
        if kind.is_dir() {
            let unbuilt = root && UNBUILT.contains(&&*name);
            if !unbuilt && !path.join("Cargo.toml").exists() {
                walk(&path, false, keep, stamps);
            }
        } else if keep(&path) {
            stamps.insert(path, entry.metadata().ok().and_then(run::stamped));
        }
    }
}

/// The sources as of a build, as it started: each file's stamp and a hash of its content, each of
/// the game's modules' text, which edits are compared with, and the modules' hashes.
struct Baseline {
    files: BTreeMap<PathBuf, Known>,
    modules: Hashes,
}

/// A file as of a build: its stamp, a hash of its content, and if it's one of the game's modules,
/// its text, unless that isn't UTF-8.
struct Known {
    stamp: Stamp,
    hash: u64,
    text: Option<String>,
}

impl Baseline {
    /// `sources` as they are.
    fn new(sources: &Sources) -> Self {
        let (mut files, mut modules) = (BTreeMap::new(), Hashes::new());
        for (path, stamp) in sources.scan() {
            let content = fs::read(&path).unwrap_or_default();
            let hash = hash_of(&content);
            let mut text = None;
            if sources.module(&path) {
                modules.insert(path.clone(), hash);
                text = String::from_utf8(content).ok();
            }
            files.insert(path, Known { stamp, hash, text });
        }
        Self { files, modules }
    }

    /// Takes the file at `path`, one that isn't a module, as it is now, as the build wrote it.
    fn refresh(&mut self, path: &Path) {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        let hash = hash_of(&fs::read(path).unwrap_or_default());
        let (stamp, text) = (run::stamped(metadata), None);
        self.files
            .insert(path.to_owned(), Known { stamp, hash, text });
    }

    /// The stamp of each file, as of the build.
    fn stamps(&self) -> Stamps {
        let stamps = (self.files.iter()).map(|(path, known)| (path.clone(), known.stamp));
        stamps.collect()
    }
}

/// A hash of `content`, which tells whether a file changed.
fn hash_of(content: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// The event that tells the failure of code `failure` is over.
fn recovered(failure: &Failure) -> Value {
    json!({"event": "recovered", "was": failure.code})
}

/// `failure` as the `error` of an event.
fn error(failure: &Failure) -> Value {
    json!({"code": failure.code, "message": failure.message})
}

/// Has each game of `sessions` log a warning through the API's `note`, `code`, saying `message`,
/// cut to fit as [`fit`] says, with `fields`, an object, so that a client following its logs, as an
/// agent does or the editor's Logs panel, learns what `gg run` prints of it: on a thread of its
/// own, since a game that has ended takes a while to refuse the connection on Windows. One that
/// doesn't take it, as one that has ended or whose engine is older than `note`, goes without, as
/// `gg run`'s own line tells of it all the same.
fn note(sessions: Vec<Session>, code: &str, message: &str, fields: &Value) {
    if sessions.is_empty() {
        return;
    }
    let note = Note {
        code: code.to_owned(),
        level: NoteLevel::Warn,
        message: fit(message, gg_api::MOST_NOTE),
        fields: fields.as_object().cloned(),
    };
    let note = serde_json::to_value(note).expect("a note serializes");
    thread::spawn(move || {
        for session in sessions {
            let noted = Client::connect(&session, "gg", CONNECT)
                .and_then(|mut client| client.call("note", note.clone()));
            drop(noted);
        }
    });
}

/// `files`' paths, as a note's message names them.
fn listed(files: &[PathBuf]) -> String {
    let paths: Vec<String> = (files.iter())
        .map(|file| file.display().to_string())
        .collect();
    paths.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_inside_function_bodies_of_the_games_modules_patch_and_other_changes_rebuild() {
        let root = std::env::temp_dir().join(format!("gg-sources-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("a directory")).expect("a directory");
            fs::write(path, text).expect("written");
        };
        let game = "fn main() {\n    run(1);\n}\n\nfn run(_: u32) {\n    step(1);\n}\n\n\
                    impl Layer for Game {\n    fn attach(&mut self) {\n        spawn(1);\n    \
                    }\n}\n";
        write("Cargo.toml", "[workspace]");
        write("Cargo.lock", "version = 4");
        write("game/Cargo.toml", "[package]");
        write("game/src/main.rs", game);
        write("game/src/tests/helper.rs", "fn help() {}\n");
        write("game/assets/logo.png", "a logo");
        write("engine/Cargo.toml", "[package]");
        write("engine/src/lib.rs", "pub fn tick() {}\n");
        write("engine/tests/ticks.rs", "#[test]\nfn ticks() {}\n");
        let files = ["Cargo.toml", "Cargo.lock", "game/Cargo.toml"];
        let sources = Sources {
            modules: root.join("game/src"),
            game: root.join("game"),
            dirs: vec![root.join("engine")],
            files: files.iter().map(|file| root.join(file)).collect(),
            lock: root.join("Cargo.lock"),
        };
        let baseline = Baseline::new(&sources);
        let (main, lib) = (
            root.join("game/src/main.rs"),
            root.join("engine/src/lib.rs"),
        );
        // A module in a directory a package's root would leave unbuilt is one under `src`.
        assert!(
            baseline
                .modules
                .contains_key(&root.join("game/src/tests/helper.rs"))
        );
        let now = || diff(&sources, &baseline, &sources.scan());
        // What no build reads isn't watched: assets, a package's tests, an editor's lock and backup.
        write("game/assets/logo.png", "another logo");
        write("engine/tests/ticks.rs", "#[test]\nfn ticks_twice() {}\n");
        write("game/src/.#main.rs", "user@host.1234");
        write("engine/src/lib.rs~", "pub fn tick() {}\n");
        let Diff::Bodies(hashes, None, attach) = now() else {
            panic!("bodies");
        };
        assert_eq!((hashes, attach), (baseline.modules.clone(), Vec::new()));
        // An edit inside a function body; each edit here changes the file's length, as one made
        // within the clock's resolution of the last may leave its time.
        write("game/src/main.rs", &game.replace("step(1)", "step(22)"));
        let Diff::Bodies(hashes, None, attach) = now() else {
            panic!("bodies");
        };
        assert_ne!(hashes[&main], baseline.modules[&main]);
        assert!(attach.is_empty());
        // One inside a layer's `attach` too, which the patch tells of.
        let attached = game.replace("step(1)", "step(22)");
        write(
            "game/src/main.rs",
            &attached.replace("spawn(1)", "spawn(333)"),
        );
        assert!(matches!(now(), Diff::Bodies(_, None, attach) if attach == [main.clone()]));
        // Text that doesn't parse, which a patch's build tells of.
        write("game/src/main.rs", &game.replace("step(1)", "step(1"));
        assert!(matches!(now(), Diff::Bodies(_, Some(file), _) if file == main));
        // A change outside the game's crate outweighs it, until it's undone.
        write("engine/src/lib.rs", "pub fn tick() {}\npub fn tock() {}\n");
        assert!(matches!(now(), Diff::Rebuild(file, OUTSIDE) if file == lib));
        write("engine/src/lib.rs", "pub fn tick() {}\n");
        assert!(matches!(now(), Diff::Bodies(..)));
        // An edit outside function bodies, and a module added.
        write("game/src/main.rs", &game.replace("u32", "u128"));
        let outside = "an edit outside function bodies";
        assert!(matches!(now(), Diff::Rebuild(file, why) if file == main && why == outside));
        write("game/src/main.rs", game);
        write("game/src/extra.rs", "");
        let added = "a module added or removed";
        assert!(matches!(now(), Diff::Rebuild(_, why) if why == added));
        let _ = fs::remove_dir_all(&root);
    }

    /// A watcher of a game without a build says its sources changed once a change has stayed for
    /// a moment, and not before.
    #[test]
    fn a_watcher_tells_a_change_once_it_has_settled() {
        let root = std::env::temp_dir().join(format!("gg-watcher-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("game/src")).expect("a directory");
        let main = root.join("game/src/main.rs");
        fs::write(&main, "fn main() {}\n").expect("written");
        let sources = Sources {
            modules: root.join("game/src"),
            game: root.join("game"),
            dirs: Vec::new(),
            files: Vec::new(),
            lock: root.join("Cargo.lock"),
        };
        let start = Instant::now();
        let mut watcher = Watcher {
            seen: sources.scan(),
            sources,
            since: start,
            changed: false,
        };
        let unchanged = watcher.changed(start + QUIET * 2);
        fs::write(&main, "fn main() { run(); }\n").expect("written");
        let at = start + QUIET * 3;
        let saving = watcher.changed(at);
        let saved = watcher.changed(at + QUIET);
        let _ = fs::remove_dir_all(&root);
        assert_eq!([unchanged, saving, saved], [false, false, true]);
    }

    /// What the run is doing, as its patches tell: waiting without processes, starting until each
    /// serves, then running its code current; changed once the sources change; not current after a
    /// failure, until a look finds the processes run what the sources hold; and, built with cargo,
    /// rebuilding at an edit a patch would carry.
    #[test]
    fn a_runs_state_tells_a_change_and_a_failure_from_current_code() {
        let root = std::env::temp_dir().join(format!("gg-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("game/src")).expect("a directory");
        let main = root.join("game/src/main.rs");
        fs::write(&main, "fn main() {}\n\nfn run() {\n    step(1);\n}\n").expect("written");
        let sources = Sources {
            modules: root.join("game/src"),
            game: root.join("game"),
            dirs: Vec::new(),
            files: Vec::new(),
            lock: root.join("Cargo.lock"),
        };
        let baseline = Baseline::new(&sources);
        let seen = baseline.stamps();
        let mut patches = Patches {
            // Where cargo can't run, so that the rebuild fails at once.
            builder: Builder::Cargo {
                package: "game".to_owned(),
                release: false,
                dir: Some(root.join("nowhere")),
            },
            sources,
            fat: Fat::unpatchable(root.join("gg-game")),
            rebuilt: None,
            stale: false,
            started: true,
            processes: vec![7],
            applied: baseline.modules.clone(),
            fresh: false,
            looked: seen.clone(),
            seen,
            since: Instant::now(),
            job: None,
            libs: Vec::new(),
            failed: None,
            baseline,
        };
        let session = Session {
            app: "Game".to_owned(),
            pid: 7,
            address: "127.0.0.1:4000".parse().expect("an address"),
            token: "t".to_owned(),
            log: None,
            engine: "0.1.0 debug".to_owned(),
            aslr_reference: 26064,
            player: None,
            project: None,
        };
        let serving = [Process {
            pid: 7,
            session: Some(&session),
        }];
        let starting = [Process {
            pid: 7,
            session: None,
        }];
        let told = |patches: &Patches, games: &[Process]| {
            let state = patches.state(games);
            (state.phase, state.current, state.error.is_some())
        };
        assert_eq!(told(&patches, &[]), ("waiting", Some(false), false));
        assert_eq!(told(&patches, &starting), ("starting", Some(false), false));
        assert_eq!(told(&patches, &serving), ("running", Some(true), false));
        assert_eq!(patches.state(&serving).pids, [7]);
        let looked = patches.looked.clone();
        patches.seen = Stamps::new();
        assert_eq!(told(&patches, &serving), ("changed", Some(false), false));
        patches.seen = looked;
        patches.failed = Some(Failure::new("gg.build", "error: expected `;`"));
        assert_eq!(told(&patches, &serving), ("running", Some(false), true));
        // A look that finds the processes run what the sources hold, as once an edit is undone.
        patches.fresh = true;
        patches.look(&serving);
        assert_eq!(told(&patches, &serving), ("running", Some(true), false));
        assert_eq!(
            recovered(&Failure::new("gg.build", "x")),
            json!({"event": "recovered", "was": "gg.build"})
        );
        // An edit a patch would carry rebuilds a game built with cargo, which here fails.
        fs::write(&main, "fn main() {}\n\nfn run() {\n    step(22);\n}\n").expect("written");
        patches.seen = patches.sources.scan();
        patches.look(&serving);
        let state = patches.state(&serving);
        let why = state.why.unwrap_or_default();
        assert!(
            state.phase == "rebuilding" && why.contains(UNPATCHED),
            "{why}"
        );
        while !patches.job.as_ref().is_some_and(Job::done) {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!patches.land(&[]));
        assert_eq!(told(&patches, &serving), ("running", Some(false), true));
        // A patch whose answer never came isn't a failure of its build: the run looks again at
        // once, patching every edit since the build; one that failed to build is.
        for (code, failed) in [("api.closed", false), ("gg.build", true)] {
            patches.failed = None;
            let outcome = Outcome::Failed(Failure::new(code, "it went"));
            patches.job = Some(Job::Patch {
                pids: vec![7],
                files: vec![main.clone()],
                hashes: Hashes::new(),
                unreadable: None,
                attach: Vec::new(),
                thread: thread::spawn(move || Patching {
                    libs: Vec::new(),
                    built: Duration::ZERO,
                    outcomes: vec![(Some(7), outcome)],
                }),
            });
            while !patches.job.as_ref().is_some_and(Job::done) {
                thread::sleep(Duration::from_millis(10));
            }
            patches.applied = patches.baseline.modules.clone();
            patches.fresh = false;
            assert!(!patches.land(&serving));
            assert_eq!(patches.failed.is_some(), failed, "{code}");
            assert_eq!(
                patches.fresh && patches.applied.is_empty(),
                !failed,
                "{code}"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_change_patches_a_game_that_serves_and_rebuilds_one_that_waits_or_cant_take_patches() {
        let module = |hash| Hashes::from([(PathBuf::from("game/src/main.rs"), hash)]);
        let (built, edited) = (module(1), module(2));
        let session = |aslr_reference| Session {
            app: "Sandbox".to_owned(),
            pid: 7,
            address: "127.0.0.1:4000".parse().expect("an address"),
            token: "t".to_owned(),
            log: None,
            engine: "0.1.0 debug".to_owned(),
            aslr_reference,
            player: None,
            project: None,
        };
        let (patchable, unpatchable) = (session(26064), session(0));
        let serving = |session| {
            vec![Process {
                pid: 7,
                session: Some(session),
            }]
        };
        let starting = vec![Process {
            pid: 7,
            session: None,
        }];
        let bodies = || Diff::Bodies(edited.clone(), None, Vec::new());
        // A change a patch can't carry rebuilds, whatever the game's process is doing.
        let lock = PathBuf::from("Cargo.lock");
        for games in [vec![], starting.clone(), serving(&patchable)] {
            let rebuild = Diff::Rebuild(lock.clone(), OUTSIDE);
            let acted = act(rebuild, &games, &built, false, true);
            assert_eq!(acted, Act::Rebuild(lock.clone(), OUTSIDE));
        }
        // An edit inside function bodies patches a process that serves, of what changed since what
        // it runs, which, patched already, is nothing; and one that doesn't yet once it does.
        let main = vec![PathBuf::from("game/src/main.rs")];
        let patch = Act::Patch(main.clone(), edited.clone(), None, Vec::new());
        assert_eq!(
            act(bodies(), &serving(&patchable), &built, false, true),
            patch
        );
        assert_eq!(
            act(bodies(), &serving(&patchable), &edited, false, true),
            Act::Nothing
        );
        assert_eq!(act(bodies(), &starting, &built, false, true), Act::Nothing);
        // Peers take a patch once both serve, and rebuild if either can't take one.
        let mut peers = serving(&patchable);
        peers.push(starting[0]);
        assert_eq!(act(bodies(), &peers, &built, false, true), Act::Nothing);
        peers[1].session = Some(&patchable);
        assert_eq!(act(bodies(), &peers, &built, false, true), patch);
        peers[1].session = Some(&unpatchable);
        let acted = act(bodies(), &peers, &built, false, true);
        assert!(matches!(acted, Act::Rebuild(..)), "{acted:?}");
        // What gg can't read goes with the patch, which tells of it.
        let unreadable = Diff::Bodies(edited.clone(), Some(main[0].clone()), Vec::new());
        let acted = act(unreadable, &serving(&patchable), &built, false, true);
        assert_eq!(
            acted,
            Act::Patch(
                main.clone(),
                edited.clone(),
                Some(main[0].clone()),
                Vec::new()
            )
        );
        // So does a module patched whose layers' `attach` differs from the build's, but not one
        // that the patch doesn't carry, which no edit changed since what the process runs.
        let attach = Diff::Bodies(edited.clone(), None, main.clone());
        let acted = act(attach, &serving(&patchable), &built, false, true);
        assert_eq!(
            acted,
            Act::Patch(main.clone(), edited.clone(), None, main.clone())
        );
        let ship = PathBuf::from("game/src/ship.rs");
        let modules = |main_hash, ship_hash| {
            let mut hashes = module(main_hash);
            hashes.insert(ship.clone(), ship_hash);
            hashes
        };
        let attach = Diff::Bodies(modules(2, 2), None, main.clone());
        let acted = act(attach, &serving(&patchable), &modules(2, 1), false, true);
        assert_eq!(
            acted,
            Act::Patch(vec![ship.clone()], modules(2, 2), None, Vec::new())
        );
        // One that can't take patches rebuilds, as does one whose last rebuild failed.
        for (session, stale) in [(&unpatchable, false), (&patchable, true)] {
            let acted = act(bodies(), &serving(session), &built, stale, true);
            assert!(matches!(acted, Act::Rebuild(..)), "{acted:?}");
        }
        // A game whose process waits for a rebuild gets one at a change since what it ran, an edit
        // or one undone after a patch, but not for sources as that process had them.
        for (now, ran) in [(&edited, &built), (&built, &edited)] {
            let waiting = act(
                Diff::Bodies(now.clone(), None, Vec::new()),
                &[],
                ran,
                false,
                true,
            );
            assert!(matches!(waiting, Act::Rebuild(..)), "{waiting:?}");
        }
        assert_eq!(act(bodies(), &[], &edited, false, true), Act::Nothing);
        // Without patches, as for a game built with cargo, an edit a patch would carry rebuilds it,
        // and sources as the processes run them don't.
        let acted = act(bodies(), &serving(&patchable), &built, false, false);
        assert_eq!(acted, Act::Rebuild(main[0].clone(), UNPATCHED));
        let acted = act(bodies(), &serving(&patchable), &edited, false, false);
        assert_eq!(acted, Act::Nothing);
    }
}
