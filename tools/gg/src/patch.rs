//! `gg run`'s patches, as ARCHITECTURE.md's Game code describes: building a game's development
//! build with dx, so that it can take patches; watching its sources; and as they change, building a
//! patch of an edit inside function bodies of its own modules and having the running game apply it
//! through the tool API, or rebuilding the game, which the run then starts again from a snapshot.

use std::collections::BTreeMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::iter;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gg_api::{Client, Failure, Patch, Patched, Session};
use serde_json::{Value, json};

use crate::build::{self, Package};
use crate::classify::{self, Edit};
use crate::dx::{self, Dx, Fat};
use crate::run::{self, Run, Stamp, event};

/// How long the sources must stay the same after a change before it's acted on, as an editor or an
/// agent may save several files at once.
const QUIET: Duration = Duration::from_millis(200);

/// How long connecting to the game for a patch may take. Its answer is waited for as long as the
/// process lives, as a file dialog or the window menu may hold its main thread, which applies it,
/// for minutes; a process that hangs is ended, which closes the connection.
const CONNECT: Duration = Duration::from_secs(10);

/// The directories at a package's root whose files no build of the package reads.
const UNBUILT: [&str; 4] = ["target", "tests", "benches", "examples"];

/// Why a change to a file that isn't one of the game's modules rebuilds it.
const OUTSIDE: &str = "a change outside the game's crate";

/// The stamp of each file watched, by its path.
type Stamps = BTreeMap<PathBuf, Stamp>;

/// A hash of each of the game's modules' text, by its path.
type Hashes = BTreeMap<PathBuf, u64>;

/// A run's patches: how dx builds the game, and what's watched; the build the game runs, which
/// patches are built against, and its sources as it started; a rebuild that succeeded, which the
/// game runs once it starts again; whether one failed since, and whether the game has started from
/// the build; the process that serves, and what of the sources its code holds; the sources as last
/// scanned and as last looked at; the build or patch being made; and the libraries the run's
/// patches made.
pub(crate) struct Patches {
    dx: Dx,
    sources: Sources,
    fat: Fat,
    baseline: Baseline,
    rebuilt: Option<(Fat, Baseline)>,
    /// Whether a rebuild failed since the build the game runs, which may have left its packages'
    /// libraries, which patches build with, other than those the build linked: until one succeeds,
    /// a change that would patch the game rebuilds it.
    stale: bool,
    started: bool,
    /// The game's process that serves the API, if one does, and the hashes of the game's modules as
    /// of the code the last process to start runs or ran: the build's, with the patches it applied.
    process: Option<u32>,
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
    /// says. If they're off, prints `patches_off`, saying why, and returns `None`, for the game to
    /// be built with cargo.
    ///
    /// Fails with `gg.cargo` if cargo can't say what the package is, and `gg.build` if the
    /// workspace has no such package, or it doesn't build.
    pub(crate) fn new(run: &Run) -> Result<Option<Self>, Failure> {
        let off = |code: &str, why: &str| {
            event(&json!({"event": "patches_off", "code": code, "why": why}));
            Ok(None)
        };
        if let Some(why) = run.unpatched() {
            return off("gg.no_patch", why);
        }
        let package = build::package(&run.package)?;
        if package.library {
            let why = format!(
                "package `{}` has a library beside its binary, and subsecond patches the binary's \
                 crate alone",
                run.package
            );
            return off("gg.library", &why);
        }
        let Some(subsecond) = &package.subsecond else {
            let why = format!(
                "package `{}` doesn't build with subsecond, which applies patches",
                run.package
            );
            return off("gg.no_subsecond", &why);
        };
        if let Err(failure) = dx::check(subsecond) {
            return off(&failure.code, &failure.message);
        }
        let (sources, dx) = (Sources::new(&package), Dx::new(&run.package, &package));
        event(&json!({"event": "watching", "paths": sources.paths()}));
        let mut baseline = Baseline::new(&sources);
        let fat = dx.build().map_err(|failure| {
            let message = format!("{}; `--no-patch` builds with cargo", failure.message);
            Failure::new(failure.code, message)
        })?;
        baseline.refresh(&sources.lock);
        let seen = baseline.stamps();
        Ok(Some(Self {
            dx,
            sources,
            fat,
            rebuilt: None,
            stale: false,
            started: false,
            process: None,
            applied: baseline.modules.clone(),
            fresh: false,
            looked: seen.clone(),
            seen,
            since: Instant::now(),
            job: None,
            libs: Vec::new(),
            baseline,
        }))
    }

    /// The executable of the build the game runs.
    pub(crate) fn exe(&self) -> &Path {
        &self.fat.exe
    }

    /// Looks after the patches for a turn of the run's loop, given the game's process, if one runs:
    /// notes the build or patch that's done, scans the sources, and looks at them, as
    /// [`look`](Self::look) says, once they've stayed the same for [`QUIET`] since they changed, or
    /// a process started serving. Returns whether a rebuild succeeded, which the run then starts
    /// the game again from.
    pub(crate) fn tend(&mut self, game: Option<Process>, now: Instant) -> bool {
        let rebuilt = self.job.as_ref().is_some_and(Job::done) && self.land();
        let stamps = self.sources.scan();
        if stamps != self.seen {
            (self.seen, self.since) = (stamps, now);
        }
        if let Some(Process {
            pid,
            session: Some(_),
        }) = game
            && self.process != Some(pid)
        {
            self.process = Some(pid);
            self.fresh = true;
        }
        let due = self.fresh || self.seen != self.looked;
        let quiet = now.duration_since(self.since) >= QUIET;
        if due && quiet && self.job.is_none() && self.rebuilt.is_none() {
            self.look(game);
        }
        rebuilt
    }

    /// Notes that the game starts, as a new process, which runs the build's code: the rebuild's that
    /// succeeded, if one did, whose sources edits are compared with from then on. Returns whether
    /// the build is another than the last process ran, as at the first start, whose executable the
    /// run copies again; a process started again from the same build runs the copy the last one
    /// ran, as a rebuild under way may be writing the build's executable.
    pub(crate) fn starting(&mut self) -> bool {
        let mut built = !std::mem::replace(&mut self.started, true);
        if let Some((fat, baseline)) = self.rebuilt.take() {
            (self.fat, self.baseline) = (fat, baseline);
            self.stale = false;
            built = true;
        }
        self.process = None;
        self.applied = self.baseline.modules.clone();
        built
    }

    /// Notes that the run waits for a rebuild, the game's process having ended: the sources are
    /// looked at as soon as they're quiet, and the game rebuilt if they differ from what that
    /// process ran.
    pub(crate) fn waiting(&mut self) {
        self.process = None;
        self.fresh = true;
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

    /// Looks at the sources as they are, and builds a patch for the game's process or rebuilds the
    /// game, as [`act`] says.
    fn look(&mut self, game: Option<Process>) {
        self.looked = self.seen.clone();
        self.fresh = false;
        let diff = diff(&self.sources, &self.baseline, &self.seen);
        match act(diff, game, &self.applied, self.stale) {
            Act::Nothing => {}
            Act::Rebuild(file, why) => self.rebuild(&file, why),
            Act::Patch(files, hashes, unreadable) => {
                if let Some(Process {
                    pid,
                    session: Some(session),
                }) = game
                {
                    self.patch(pid, session, files, hashes, unreadable);
                }
            }
        }
    }

    /// Rebuilds the game on a thread of its own, having said why: a change to `file`, for `why`.
    fn rebuild(&mut self, file: &Path, why: &str) {
        event(&json!({"event": "rebuilding", "file": file, "why": why}));
        let (dx, sources) = (self.dx.clone(), self.sources.clone());
        let thread = thread::spawn(move || {
            // The sources as the build starts, which edits made as it runs then differ from, but for
            // the lock file, which the build itself writes as a manifest's dependencies change.
            let mut baseline = Baseline::new(&sources);
            let fat = dx.build()?;
            baseline.refresh(&sources.lock);
            Ok((fat, baseline))
        });
        let started = Instant::now();
        self.job = Some(Job::Rebuild { started, thread });
    }

    /// Builds a patch of edits to `files` for process `pid`, whose session is `session`, and has
    /// the process apply it, on a thread of its own; once it has, its modules' hashes are `hashes`.
    /// One whose sources changed as it built is dropped, as it may hold what wasn't looked at; and
    /// with `unreadable`, a module whose text gg can't read, one that builds isn't applied.
    fn patch(
        &mut self,
        pid: u32,
        session: &Session,
        files: Vec<PathBuf>,
        hashes: Hashes,
        unreadable: Option<PathBuf>,
    ) {
        let (dx, fat, session) = (self.dx.clone(), self.fat.clone(), session.clone());
        let (sources, looked, checked) = (
            self.sources.clone(),
            self.looked.clone(),
            unreadable.is_none(),
        );
        let thread = thread::spawn(move || {
            let start = Instant::now();
            let table = dx.hotpatch(&fat, session.aslr_reference);
            let built = start.elapsed();
            let lib = table.as_ref().ok().and_then(|table| table["lib"].as_str());
            let lib = lib.map(PathBuf::from);
            let outcome = match table {
                Err(failure) => Outcome::Failed(failure),
                // dx built what the files held as it read them, which an edit saved since they were
                // looked at may have changed.
                Ok(_) if sources.scan() != looked => Outcome::Superseded,
                Ok(_) if !checked => Outcome::Unreadable,
                Ok(table) => {
                    let start = Instant::now();
                    let applied = Client::connect(&session, CONNECT).and_then(|mut client| {
                        client.set_timeout(None)?;
                        client.call_as::<Patched>("patch", Patch { table })
                    });
                    match applied {
                        Ok(patched) => Outcome::Applied(patched, start.elapsed()),
                        Err(failure) => Outcome::Failed(failure),
                    }
                }
            };
            Patching {
                lib,
                built,
                outcome,
            }
        });
        self.job = Some(Job::Patch {
            pid,
            files,
            hashes,
            unreadable,
            thread,
        });
    }

    /// Notes what the job that's done gave, printing it, and returns whether it was a rebuild that
    /// succeeded.
    fn land(&mut self) -> bool {
        let panicked = || Failure::new("gg.build", "the thread making it panicked");
        match self.job.take() {
            Some(Job::Rebuild { started, thread }) => {
                match thread.join().unwrap_or_else(|_| Err(panicked())) {
                    Ok((fat, baseline)) => {
                        let ms = millis(started.elapsed());
                        event(&json!({"event": "rebuilt", "exe": fat.exe, "build_ms": ms}));
                        self.rebuilt = Some((fat, baseline));
                        true
                    }
                    Err(failure) => {
                        event(&json!({"event": "rebuild_failed", "error": error(&failure)}));
                        self.stale = true;
                        false
                    }
                }
            }
            Some(Job::Patch {
                pid,
                files,
                hashes,
                unreadable,
                thread,
            }) => {
                let patching = thread.join().unwrap_or_else(|_| Patching {
                    lib: None,
                    built: Duration::ZERO,
                    outcome: Outcome::Failed(panicked()),
                });
                self.libs.extend(patching.lib);
                match patching.outcome {
                    Outcome::Applied(patched, applied) => {
                        if self.process == Some(pid) {
                            self.applied = hashes;
                        }
                        event(&json!({
                            "event": "patched",
                            "pid": pid,
                            "files": files,
                            "build_ms": millis(patching.built),
                            "apply_ms": millis(applied),
                            "ticks": patched.ticks,
                        }));
                    }
                    Outcome::Failed(failure) => {
                        // Sent, but its answer never came: the process may yet apply it, so what it
                        // runs isn't known, and the next look patches it with every edit since its
                        // build.
                        let unanswered =
                            matches!(failure.code.as_str(), "api.closed" | "api.timeout");
                        if unanswered && self.process == Some(pid) {
                            self.applied.clear();
                        }
                        event(&json!({
                            "event": "patch_failed",
                            "pid": pid,
                            "files": files,
                            "error": error(&failure),
                        }));
                    }
                    Outcome::Superseded => {}
                    Outcome::Unreadable => {
                        let file = unreadable.unwrap_or_default();
                        self.rebuild(&file, "an edit gg can't read, which rustc builds");
                    }
                }
                false
            }
            None => false,
        }
    }
}

/// A build or a patch being made, on a thread of its own, so that the game is looked after
/// meanwhile.
enum Job {
    /// A rebuild, started at `started`, which gives the build and its sources as it started.
    Rebuild {
        started: Instant,
        thread: JoinHandle<Result<(Fat, Baseline), Failure>>,
    },
    /// A patch of edits to `files` for process `pid`, after which the hashes of the game's modules
    /// are `hashes`, and the module whose text gg can't read, if one is.
    Patch {
        pid: u32,
        files: Vec<PathBuf>,
        hashes: Hashes,
        unreadable: Option<PathBuf>,
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

/// How a patch went: the library it made, if it was built, how long building it took, and what
/// came of it.
struct Patching {
    lib: Option<PathBuf>,
    built: Duration,
    outcome: Outcome,
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
    /// In function bodies of the game's modules, if at all: the hash of each module, and a module
    /// whose text gg can't read, if one is.
    Bodies(Hashes, Option<PathBuf>),
}

/// What the sources call for.
#[derive(Debug, PartialEq)]
enum Act {
    /// Nothing: the game's process runs what they hold, or takes a patch of it once it serves.
    Nothing,
    /// A patch of edits to these modules, after which the hash of each module is as given, and the
    /// module whose text gg can't read, if one is.
    Patch(Vec<PathBuf>, Hashes, Option<PathBuf>),
    /// A rebuild, for a change to a file, for a reason.
    Rebuild(PathBuf, &'static str),
}

/// What sources that differ from the build's as `diff` says call for, given the game's process, if
/// one runs, the hashes of the game's modules as of the code it runs, or the last one ran,
/// `applied`, and whether a rebuild failed since the build, `stale`. A change a patch can't carry
/// rebuilds the game. Otherwise a process that serves takes a patch of what changed since what it
/// runs, unless it can't take patches, as one whose executable doesn't export its `main` can't, or
/// the build is stale; one that doesn't serve yet takes one once it does; and a game whose process
/// crashed or hung, which waits for a rebuild, gets one at any change since what that process ran,
/// as a patch made after it starts again could come too late.
fn act(diff: Diff, game: Option<Process>, applied: &Hashes, stale: bool) -> Act {
    let (hashes, unreadable) = match diff {
        Diff::Rebuild(file, why) => return Act::Rebuild(file, why),
        Diff::Bodies(hashes, unreadable) => (hashes, unreadable),
    };
    match game {
        None => match changed(applied, &hashes).into_iter().next() {
            Some(file) => Act::Rebuild(file, "the game waits for a rebuild"),
            None => Act::Nothing,
        },
        Some(Process { session: None, .. }) => Act::Nothing,
        Some(Process {
            session: Some(session),
            ..
        }) => {
            let files = changed(applied, &hashes);
            let Some(file) = files.first() else {
                return Act::Nothing;
            };
            if session.aslr_reference == 0 {
                let why = "the game can't take patches, as its executable exports no `main`";
                return Act::Rebuild(file.clone(), why);
            }
            if stale {
                let why = "a rebuild failed since the game's build, and patches would build with \
                           what it left";
                return Act::Rebuild(file.clone(), why);
            }
            Act::Patch(files, hashes, unreadable)
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
/// one given. A file whose stamp changed but not its content hasn't changed.
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
    let (mut hashes, mut unreadable) = (Hashes::new(), None);
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
                    Edit::Bodies => {}
                    Edit::Unreadable => unreadable = Some(path.clone()),
                    Edit::Rebuild(why) => return Diff::Rebuild(path.clone(), why),
                }
            }
        }
        if module {
            hashes.insert(path.clone(), hash);
        }
    }
    Diff::Bodies(hashes, unreadable)
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

/// `failure`, as events give it.
fn error(failure: &Failure) -> Value {
    json!({"code": failure.code, "message": failure.message})
}

/// `duration` in whole milliseconds.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
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
        let game = "fn main() {\n    run(1);\n}\n\nfn run(_: u32) {\n    step(1);\n}\n";
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
        assert!(matches!(now(), Diff::Bodies(hashes, None) if hashes == baseline.modules));
        // An edit inside a function body; each edit here changes the file's length, as one made
        // within the clock's resolution of the last may leave its time.
        write("game/src/main.rs", &game.replace("step(1)", "step(22)"));
        let Diff::Bodies(hashes, None) = now() else {
            panic!("bodies");
        };
        assert_ne!(hashes[&main], baseline.modules[&main]);
        // Text that doesn't parse, which a patch's build tells of.
        write("game/src/main.rs", &game.replace("step(1)", "step(1"));
        assert!(matches!(now(), Diff::Bodies(_, Some(file)) if file == main));
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
        };
        let (patchable, unpatchable) = (session(26064), session(0));
        let serving = |session| {
            Some(Process {
                pid: 7,
                session: Some(session),
            })
        };
        let starting = Some(Process {
            pid: 7,
            session: None,
        });
        let bodies = || Diff::Bodies(edited.clone(), None);
        // A change a patch can't carry rebuilds, whatever the game's process is doing.
        let lock = PathBuf::from("Cargo.lock");
        for game in [None, starting, serving(&patchable)] {
            let rebuild = Diff::Rebuild(lock.clone(), OUTSIDE);
            let acted = act(rebuild, game, &built, false);
            assert_eq!(acted, Act::Rebuild(lock.clone(), OUTSIDE));
        }
        // An edit inside function bodies patches a process that serves, of what changed since what
        // it runs, which, patched already, is nothing; and one that doesn't yet once it does.
        let main = vec![PathBuf::from("game/src/main.rs")];
        let patch = Act::Patch(main.clone(), edited.clone(), None);
        assert_eq!(act(bodies(), serving(&patchable), &built, false), patch);
        assert_eq!(
            act(bodies(), serving(&patchable), &edited, false),
            Act::Nothing
        );
        assert_eq!(act(bodies(), starting, &built, false), Act::Nothing);
        // What gg can't read goes with the patch, which tells of it.
        let unreadable = Diff::Bodies(edited.clone(), Some(main[0].clone()));
        let acted = act(unreadable, serving(&patchable), &built, false);
        assert_eq!(
            acted,
            Act::Patch(main.clone(), edited.clone(), Some(main[0].clone()))
        );
        // One that can't take patches rebuilds, as does one whose last rebuild failed.
        for (session, stale) in [(&unpatchable, false), (&patchable, true)] {
            let acted = act(bodies(), serving(session), &built, stale);
            assert!(matches!(acted, Act::Rebuild(..)), "{acted:?}");
        }
        // A game whose process waits for a rebuild gets one at a change since what it ran, an edit
        // or one undone after a patch, but not for sources as that process had them.
        for (now, ran) in [(&edited, &built), (&built, &edited)] {
            let waiting = act(Diff::Bodies(now.clone(), None), None, ran, false);
            assert!(matches!(waiting, Act::Rebuild(..)), "{waiting:?}");
        }
        assert_eq!(act(bodies(), None, &edited, false), Act::Nothing);
    }
}
