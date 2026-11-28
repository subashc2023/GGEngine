//! `gg run`'s patches, as ARCHITECTURE.md's Game code describes: building the game with dx and
//! watching its sources. A change to function bodies, functions added, removed or re-signed,
//! `use`s, or constants only bodies use, is built into a patch the game applies through the tool
//! API, as [`classify`] tells. Another change to the game's modules, or to code only the game's
//! start reaches ([`reach`]), is built into a patch the game restarts in place with, if it can. The
//! rest rebuilds, as do edits to code only `main` reaches and `gg rebuild`'s asks, and the run
//! restarts the game from a snapshot. The API's `note` has the game log failed patches, restarts
//! and rebuilds, and edits to code only `attach` reaches, which won't show. Without dx, or where
//! patches can't reach the game's code, it builds with cargo and each change rebuilds.

use std::collections::BTreeMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};
use std::{iter, mem};

use gg_api::{
    Client, Failure, LogEvents, Note, NoteLevel, Patch, Patched, Session, Snapshotted, UNBUILT,
};
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::build::{self, Package, fit};
use crate::classify::{self, Edit, Uses};
use crate::dx::{self, Dx, Fat};
use crate::reach::{self, Code, Prints};
use crate::run::{self, Run, Stamp, event, millis};
use crate::runs::{self, Asked, MOST_KEPT, State};

/// How long the sources must stay the same before a change is acted on: saves come in bursts.
const QUIET: Duration = Duration::from_millis(200);

/// How long connecting to the game for a patch may take. Its answer is awaited as long as the
/// process lives: a file dialog or the window menu may hold the main thread, which applies it, for
/// minutes. A hung process is ended, closing the connection.
const CONNECT: Duration = Duration::from_secs(10);

const OUTSIDE: &str = "a change outside the game's modules";

/// Why a game that restarted in place is rebuilt before it restarts from its snapshot.
pub(crate) const OUTDATED: &str = "the game ended having restarted in place, which its build \
                                   predates: it lays out the game's types as they were";

const UNPATCHED: &str =
    "the run builds the game with cargo, without patches, as `patches_off` said";

/// Why a rebuild `gg rebuild` asked for runs.
const ASKED: &str = "`gg rebuild` asked";

/// Why a rebuild `gg rebuild --fresh` asked for runs.
const ASKED_FRESH: &str = "`gg rebuild --fresh` asked";

type Stamps = BTreeMap<PathBuf, Stamp>;

type Hashes = BTreeMap<PathBuf, u64>;

/// Modules' texts, by path.
type Texts = BTreeMap<PathBuf, String>;

/// The modules patches changed, each with its hash and text as the processes run them: an edit
/// since, of doc comments or tests alone, changes nothing they run.
type Ran = BTreeMap<PathBuf, (u64, String)>;

/// A run's patches: its builds, the sources it watches, and what the game's processes run.
pub(crate) struct Patches {
    builder: Builder,
    sources: Sources,
    fat: Fat,
    baseline: Baseline,
    /// The sources as the last restart in place built them, which the process runs since, edits
    /// being compared with them; `None` while the processes run the build's layouts. Patches stay
    /// against the build, carrying every edit since.
    layout: Option<Baseline>,
    /// Where a restart in place writes its snapshot: the one the run restarts the game from, set by
    /// [`restart_into`](Self::restart_into); `None` keeps the game from restarting in place.
    snapshot: Option<PathBuf>,
    /// The snapshot the last restart in place wrote, until the run takes it ([`taken`](Self::taken)).
    taken: Option<Snapshotted>,
    /// The rebuild that succeeded, until the game starts on it: its build, its sources, and whether
    /// `gg rebuild --fresh` asked for it, so the game starts afresh.
    rebuilt: Option<(Fat, Baseline, bool)>,
    /// Whether a rebuild failed since the game's build, which may have left its packages'
    /// libraries, which patches build with, unlike the build's: until one succeeds, a change
    /// rebuilds.
    stale: bool,
    started: bool,
    /// The game's processes once all serve (each peer of a lockstep session), and the modules'
    /// hashes as of the code they run: the build's plus the patches they applied, all alike.
    processes: Vec<u32>,
    applied: Hashes,
    ran: Ran,
    /// Whether to look at the sources once quiet, changed or not: a process starts serving, or the
    /// run starts waiting for a rebuild.
    fresh: bool,
    seen: Stamps,
    since: Instant,
    looked: Stamps,
    job: Option<Job>,
    libs: Vec<PathBuf>,
    /// The failure of the last rebuild or patch, until one succeeds or a look at the sources finds
    /// the processes run what they hold.
    failed: Option<Failure>,
    /// The last patch or rebuild that succeeded, as [`State::reload`] tells it.
    reload: Option<Value>,
    /// The latest [`MOST_KEPT`] outcomes of patches and rebuilds, failed ones too, oldest first, as
    /// [`State::reloads`] tells them.
    reloads: Vec<Value>,
    /// The `gg rebuild`s not yet acted on, as one, as
    /// [`Told::asked_to_rebuild`](runs::Told::asked_to_rebuild) gives them.
    asked: Option<Asked>,
    /// The asked rebuild that landed, not yet answered, and its failure.
    answered: Option<(Asked, Option<Failure>)>,
    /// The prints of the build the world was built by, as the game last started afresh, its layers'
    /// `attach` building it; `None` if gg couldn't read that build's modules.
    world: Option<Prints>,
    /// The code only `attach` reaches whose edits the game was told of, with their prints: each is
    /// told once.
    told: BTreeMap<Code, u64>,
}

/// With dx, which patches build against, or with cargo, for a game patches can't reach.
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
    fn build(&self) -> Result<Fat, Failure> {
        match self {
            Builder::Dx(dx) => dx.build(),
            Builder::Cargo {
                package,
                release,
                dir,
            } => {
                let args: &[&str] = if *release { &["--release"] } else { &[] };
                build::build(package, args, dir.as_deref()).map(|built| Fat::unpatchable(built.exe))
            }
        }
    }
}

/// A game process: its ID and, once it serves the API, its session.
#[derive(Clone, Copy)]
pub(crate) struct Process<'a> {
    pub(crate) pid: u32,
    pub(crate) session: Option<&'a Session>,
}

impl Patches {
    /// Builds `run`'s game with dx and watches its sources, if patches are on: dx is the game's
    /// subsecond release, the package has no library beside its binary (subsecond patches the
    /// binary's crate alone), and [`Run::unpatched`] finds nothing against. Otherwise prints
    /// `patches_off` with why; then, unless restarts are off or `--no-patch` asks to build the game
    /// itself, builds with cargo and watches, each change rebuilding; else returns `None`.
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
        let paths = sources.paths(None);
        event(&json!({"event": "watching", "paths": paths, "patches": patches}));
        let mut baseline = Baseline::new(&sources);
        let fat = builder.build().map_err(|failure| {
            let message = match &builder {
                // First: dx's own failures follow pages of the linker's warnings.
                Builder::Dx(_) => format!(
                    "`--no-patch` builds with cargo, if dx fails where cargo doesn't; {}",
                    failure.message
                ),
                Builder::Cargo { .. } => failure.message,
            };
            Failure::new(failure.code, message)
        })?;
        baseline.refresh(&sources.lock);
        baseline.narrow(&sources, &fat.exe);
        if let Some(inputs) = &baseline.inputs {
            event(&json!({
                "event": "watching",
                "paths": sources.paths(Some(inputs)),
                "read": inputs.files.len(),
                "dep_info": inputs.from,
                "patches": patches,
            }));
        }
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
            ran: Ran::new(),
            fresh: false,
            looked: seen.clone(),
            seen,
            since: Instant::now(),
            job: None,
            libs: Vec::new(),
            failed: None,
            reload: None,
            reloads: Vec::new(),
            asked: None,
            answered: None,
            world: None,
            told: BTreeMap::new(),
            baseline,
            layout: None,
            snapshot: None,
            taken: None,
        }))
    }

    /// What the run is doing as far as patches tell, as [`State`] has it; `changed` once the
    /// sources changed and it hasn't acted; running is current unless a rebuild or patch failed.
    pub(crate) fn state(&self, games: &[Process]) -> State {
        let mut state = State::new("running");
        state.pids = games.iter().map(|game| game.pid).collect();
        state.error.clone_from(&self.failed);
        state.reload.clone_from(&self.reload);
        state.reloads.clone_from(&self.reloads);
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

    pub(crate) fn exe(&self) -> &Path {
        &self.fat.exe
    }

    /// How long the build of [`exe`](Self::exe) waited for dx's lock.
    pub(crate) fn queued(&self) -> Duration {
        self.fat.queued
    }

    /// Notes an outcome, `at` now, for `wait` to list.
    fn outcome(&mut self, mut outcome: Value) {
        outcome["at"] = json!(runs::now_ms());
        if self.reloads.len() == MOST_KEPT {
            self.reloads.remove(0);
        }
        self.reloads.push(outcome);
    }

    /// Notes that the game's processes, which started from the rebuild before the newest, ended to
    /// restart from the newest, having warned as `events` say: `wait` lists them with the rebuild
    /// that started them, as the restart that follows hides them.
    pub(crate) fn warned(&mut self, events: &[Box<serde_json::value::RawValue>]) {
        let (warnings, _) = runs::about_warnings(events, runs::MOST_WARNINGS);
        let rebuilds = self.reloads.iter_mut().rev();
        let mut rebuilds = rebuilds.filter(|outcome| outcome["event"] == "rebuilt");
        if let Some(older) = rebuilds.nth(1)
            && !warnings.is_empty()
        {
            older["warnings"] = Value::Array(warnings);
        }
    }

    /// Notes that `gg rebuild` asked for a rebuild, which [`tend`](Self::tend) starts once no job
    /// is under way.
    pub(crate) fn ask(&mut self, asked: Asked) {
        self.asked = Some(self.asked.map_or(asked, |was| was.and(asked)));
    }

    /// The asked rebuild that landed since the last call, if one did, and its failure, if it
    /// failed.
    pub(crate) fn answered(&mut self) -> Option<(Asked, Option<Failure>)> {
        self.answered.take()
    }

    /// Whether the rebuild that succeeded, which the game hasn't started on yet, was asked to start
    /// it afresh.
    pub(crate) fn afresh(&self) -> bool {
        self.rebuilt.as_ref().is_some_and(|(_, _, fresh)| *fresh)
    }

    /// A turn of the run's loop: notes a finished job, scans the sources, and once they've been
    /// quiet for [`QUIET`], starts a rebuild `gg rebuild` asked for, or looks at them if they
    /// changed or the processes all serve. `games` is empty while it waits for a rebuild. Returns
    /// whether a rebuild succeeded, which the run restarts from.
    pub(crate) fn tend(&mut self, games: &[Process], now: Instant) -> bool {
        let rebuilt = self.job.as_ref().is_some_and(Job::done) && self.land(games);
        let stamps = self.sources.scan(self.baseline.inputs.as_ref());
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
        if quiet && self.job.is_none() && self.rebuilt.is_none() {
            if let Some(asked) = self.asked.take() {
                self.rebuild_asked(asked);
            } else if due {
                self.look(games);
            }
        }
        rebuilt
    }

    /// Notes that the game starts as new processes on the build: the succeeded rebuild's, if any,
    /// which edits are compared with from then on. `afresh` if none starts from a snapshot, so the
    /// build's `attach` builds the world. Returns whether the build is new, so the run copies its
    /// executable again; restarts from the same build run the last copies, as a rebuild under way
    /// may be writing the executable.
    pub(crate) fn starting(&mut self, afresh: bool) -> bool {
        let mut built = !std::mem::replace(&mut self.started, true);
        if let Some((fat, baseline, _)) = self.rebuilt.take() {
            (self.fat, self.baseline) = (fat, baseline);
            self.stale = false;
            built = true;
        }
        self.processes.clear();
        // A new process runs the build's layouts.
        self.layout = None;
        self.applied = self.baseline.modules.clone();
        self.ran.clear();
        if afresh {
            self.world = self.baseline.prints.clone();
            self.told.clear();
        }
        built
    }

    /// Notes that the processes ended: once quiet, the sources are looked at, and the game
    /// rebuilt if they differ from what ran.
    pub(crate) fn waiting(&mut self) {
        self.processes.clear();
        self.fresh = true;
    }

    /// Waits for the job under way, so dx removes the environment it wrote and no build outlives
    /// the run's lock.
    pub(crate) fn finish(&mut self) {
        match self.job.take() {
            Some(Job::Rebuild { thread, .. }) => drop(thread.join()),
            Some(Job::Patch { thread, .. }) => drop(thread.join()),
            None => {}
        }
    }

    /// Removes the patch libraries, once the processes that loaded them ended, and what the Windows
    /// linker wrote beside each: import library, exports and PDB (the largest).
    pub(crate) fn remove_libs(&self) {
        for lib in &self.libs {
            let beside = ["lib", "exp", "pdb"].map(|extension| lib.with_extension(extension));
            for path in iter::once(lib.clone()).chain(beside) {
                let _ = run::retrying(|| run::remove(&path));
            }
        }
    }

    fn look(&mut self, games: &[Process]) {
        self.looked = self.seen.clone();
        self.fresh = false;
        let running = self.layout.as_ref().unwrap_or(&self.baseline);
        let diff = diff(&self.sources, running, &self.seen, &self.ran);
        let diff = self.past_new_layers(diff);
        let patches = matches!(self.builder, Builder::Dx(_));
        // The prints and edited modules' texts of the sources as they are.
        let (prints, texts) = match &diff {
            Diff::Bodies(_, _, prints, texts) | Diff::Restart(.., prints, texts) => {
                (prints.clone(), texts.clone())
            }
            Diff::Rebuild(..) => (None, Texts::new()),
        };
        match act(diff, games, &self.applied, self.stale, patches) {
            Act::Nothing => {
                // The processes run what the sources hold, once each serves: after a restart, a
                // rebuild's edits to `attach` are told of here.
                let sessions: Option<Vec<Session>> =
                    games.iter().map(|game| game.session.cloned()).collect();
                if let Some(sessions) = sessions.filter(|sessions| !sessions.is_empty()) {
                    if let Some(failure) = self.failed.take() {
                        event(&recovered(&failure));
                    }
                    let attach = self.attached(prints.as_ref());
                    tell_attach(sessions, &attach);
                }
            }
            Act::Rebuild(file, why) => self.rebuild(&file, &why),
            Act::Patch(files, hashes, unreadable) => {
                let serving = games
                    .iter()
                    .filter_map(|game| Some((game.pid, game.session?.clone())));
                let ran = ran_after(&files, &hashes, &texts);
                self.patch(
                    serving.collect(),
                    files,
                    hashes,
                    unreadable,
                    prints,
                    ran,
                    None,
                );
            }
            Act::Restart(file, _, _, _, _) if self.snapshot.is_none() => {
                self.rebuild(&file, "restarts are off, which a restart in place needs");
            }
            Act::Restart(file, why, files, hashes, unreadable) => {
                let serving = games
                    .iter()
                    .filter_map(|game| Some((game.pid, game.session?.clone())));
                let running = self.layout.as_ref().unwrap_or(&self.baseline);
                let layout = running.restarted(&self.sources, &self.looked, &texts, prints.clone());
                let since = SystemTime::now();
                let restart = Restarting {
                    file,
                    why,
                    layout,
                    since,
                };
                let (serving, ran) = (serving.collect(), Ran::new());
                self.patch(
                    serving,
                    files,
                    hashes,
                    unreadable,
                    prints,
                    ran,
                    Some(Box::new(restart)),
                );
            }
        }
    }

    /// Has restarts in place write their snapshots at `path`: the game's own, which the run
    /// restarts it from.
    pub(crate) fn restart_into(&mut self, path: PathBuf) {
        self.snapshot = Some(path);
    }

    /// The snapshot the last restart in place wrote, if one did since the last call: the newest of
    /// the game's world, which the run restarts it from.
    pub(crate) fn taken(&mut self) -> Option<Snapshotted> {
        self.taken.take()
    }

    /// As a game's process ends: waits for a restart in place under way, as its failure may have
    /// ended it, and notes how it went, so [`taken`](Self::taken) gives its snapshot.
    pub(crate) fn settle(&mut self) {
        if matches!(
            &self.job,
            Some(Job::Patch {
                restart: Some(_),
                ..
            })
        ) {
            self.land(&[]);
        }
    }

    /// As a game's process ended to restart from its snapshot: whether it had restarted in place,
    /// running layouts its build lacks, which the snapshot may hold (a new component type, a field's
    /// value). Then the build is rebuilt first, starting now unless a rebuild is under way; a patch
    /// under way lands first. A rebuild that succeeded since, which the restart takes, is current.
    pub(crate) fn outdated(&mut self) -> bool {
        if self.layout.is_none() || self.rebuilt.is_some() {
            return false;
        }
        if matches!(self.job, Some(Job::Patch { .. })) {
            self.land(&[]);
        }
        if self.job.is_none() {
            event(&json!({"event": "rebuilding", "file": null, "why": OUTDATED}));
            self.start_rebuild(OUTDATED.to_owned(), None);
        }
        true
    }

    /// `diff`, but an edit a patch would carry restarts the game in place once a restart in place
    /// gave it a layer type the build lacks. That type's methods run the restart's code, which a
    /// later patch reaches only through functions the build has (the engine's `patch::compose`),
    /// so a patch would change nothing they run.
    fn past_new_layers(&self, diff: Diff) -> Diff {
        let Diff::Bodies(hashes, unreadable, prints, texts) = diff else {
            return diff;
        };
        let new = (self.layout.as_ref()).and_then(|layout| {
            Some(
                layout
                    .prints
                    .as_ref()?
                    .new_layers(self.baseline.prints.as_ref()?),
            )
        });
        let file = changed(&self.applied, &hashes).into_iter().next();
        match (new.unwrap_or_default().first(), file) {
            (Some((_, layer)), Some(file)) => {
                let why = format!(
                    "an edit since a restart in place added layer `{layer}`, whose methods only \
                     another restart in place reaches"
                );
                Diff::Restart(file, why, hashes, unreadable, prints, texts)
            }
            _ => Diff::Bodies(hashes, unreadable, prints, texts),
        }
    }

    /// The code only `attach` reaches that differs, as of `prints`, from what built the world, and
    /// that the game wasn't told of; it's told of from then on.
    fn attached(&mut self, prints: Option<&Prints>) -> Vec<Code> {
        match (prints, &self.world) {
            (Some(now), Some(world)) => now.attached(world, &mut self.told),
            _ => Vec::new(),
        }
    }

    fn rebuild(&mut self, file: &Path, why: &str) {
        event(&json!({"event": "rebuilding", "file": file, "why": why}));
        self.start_rebuild(format!("a change to {}: {why}", file.display()), None);
    }

    /// Rebuilds the game, as `gg rebuild` asked, with every edit since the last look: the build
    /// takes the sources as they are.
    fn rebuild_asked(&mut self, asked: Asked) {
        let why = if asked.fresh { ASKED_FRESH } else { ASKED };
        event(&json!({"event": "rebuilding", "file": null, "why": why}));
        (self.looked, self.fresh) = (self.seen.clone(), false);
        self.start_rebuild(why.to_owned(), Some(asked));
    }

    /// Starts a rebuild for `why`, as `gg rebuild` asked, if it did.
    fn start_rebuild(&mut self, why: String, asked: Option<Asked>) {
        let (builder, sources) = (self.builder.clone(), self.sources.clone());
        let thread = thread::spawn(move || {
            // The sources as the build starts, but for the lock file, which the build itself writes
            // as a manifest's dependencies change.
            let mut baseline = Baseline::new(&sources);
            let fat = builder.build()?;
            baseline.refresh(&sources.lock);
            baseline.narrow(&sources, &fat.exe);
            Ok((fat, baseline))
        });
        self.job = Some(Job::Rebuild {
            started: Instant::now(),
            why,
            asked,
            thread,
        });
    }

    /// On its own thread, builds a patch of edits to `files` for each of `processes` (ID and
    /// session), against its `main`, and has them apply it at once; a lockstep joiner waits for its
    /// own at the tick its host's applies before. Afterwards the modules' hashes are `hashes`, and
    /// their functions' prints `prints`, and the modules it changed run as `ran` says. A patch
    /// whose sources changed as it built is dropped; with `unreadable`, one that builds isn't
    /// applied. With `restart`, the lone process restarts in place with it, writing its snapshot
    /// first ([`Patch::restart`]), and the game's warnings since are kept.
    #[expect(clippy::too_many_arguments)]
    fn patch(
        &mut self,
        processes: Vec<(u32, Session)>,
        files: Vec<PathBuf>,
        hashes: Hashes,
        unreadable: Option<PathBuf>,
        prints: Option<Prints>,
        ran: Ran,
        restart: Option<Box<Restarting>>,
    ) {
        let Builder::Dx(dx) = &self.builder else {
            let file = files.first().cloned().unwrap_or_default();
            return self.rebuild(&file, UNPATCHED);
        };
        let (dx, fat) = (dx.clone(), self.fat.clone());
        let (sources, inputs, looked, checked) = (
            self.sources.clone(),
            self.baseline.inputs.clone(),
            self.looked.clone(),
            unreadable.is_none(),
        );
        let pids = processes.iter().map(|(pid, _)| *pid).collect();
        let snapshot = restart.as_ref().and(self.snapshot.clone());
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
                // dx built what the files held as it read them: a later save may differ.
                None if sources.scan(inputs.as_ref()) != looked => {
                    vec![(None, Outcome::Superseded)]
                }
                None if !checked => vec![(None, Outcome::Unreadable)],
                None => {
                    let applying = tables.into_iter().map(|(pid, session, table)| {
                        let restart = snapshot.clone();
                        let thread = thread::spawn(move || {
                            let start = Instant::now();
                            let applied =
                                Client::connect(&session, "gg", CONNECT).and_then(|mut client| {
                                    client.set_timeout(None)?;
                                    apply(&mut client, table, restart)
                                });
                            match applied {
                                Ok((patched, warned)) => {
                                    Outcome::Applied(patched, start.elapsed(), warned)
                                }
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
            prints,
            ran,
            restart,
            thread,
        });
    }

    /// Notes what the finished job gave; returns whether it was a successful rebuild. Has `games`
    /// log failures, and edits to code only `attach` reaches that a patch brought in, as [`note`]
    /// says.
    fn land(&mut self, games: &[Process]) -> bool {
        let panicked = || Failure::new("gg.build", "the thread making it panicked");
        // The serving sessions: `pid`'s, or all.
        let serving = |pid: Option<u32>| -> Vec<Session> {
            let games = games
                .iter()
                .filter(|game| pid.is_none_or(|pid| game.pid == pid));
            games.filter_map(|game| game.session.cloned()).collect()
        };
        match self.job.take() {
            Some(Job::Rebuild {
                started,
                thread,
                why,
                asked,
            }) => match thread.join().unwrap_or_else(|_| Err(panicked())) {
                Ok((fat, baseline)) => {
                    self.answered = asked.map(|asked| (asked, None));
                    // The wait for dx's lock isn't the build's time.
                    let queued = fat.queued;
                    let ms = millis(started.elapsed().saturating_sub(queued));
                    let rebuilt = json!({"event": "rebuilt", "exe": fat.exe, "build_ms": ms});
                    event(&run::with_queued(rebuilt, queued));
                    let rebuilt = json!({"event": "rebuilt", "why": why, "build_ms": ms});
                    self.outcome(run::with_queued(rebuilt, queued));
                    self.reload = self.reloads.last().cloned();
                    let fresh = asked.is_some_and(|asked| asked.fresh);
                    self.rebuilt = Some((fat, baseline, fresh));
                    self.failed = None;
                    true
                }
                Err(failure) => {
                    self.answered = asked.map(|asked| (asked, Some(failure.clone())));
                    event(&json!({"event": "rebuild_failed", "error": error(&failure)}));
                    self.outcome(json!({
                        "event": "rebuild_failed",
                        "why": why,
                        "error": error(&failure),
                    }));
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
                prints,
                restart: Some(restart),
                thread,
                ..
            }) => {
                let patching = thread.join().unwrap_or_else(|_| Patching {
                    libs: Vec::new(),
                    built: Duration::ZERO,
                    outcomes: vec![(None, Outcome::Failed(panicked()))],
                });
                self.libs.extend(patching.libs);
                let current = self.processes == pids;
                for (pid, outcome) in patching.outcomes {
                    let (prints, to) = (prints.as_ref(), serving(pid));
                    let built = patching.built;
                    self.restarted(pid, outcome, current, &files, prints, &restart, built, to);
                }
                false
            }
            Some(Job::Patch {
                pids,
                files,
                hashes,
                unreadable,
                prints,
                ran,
                restart: None,
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
                let attach = if all && current {
                    self.applied = hashes;
                    for file in &files {
                        self.ran.remove(file);
                    }
                    self.ran.extend(ran);
                    self.failed = None;
                    self.attached(prints.as_ref())
                } else {
                    Vec::new()
                };
                let attach_files = modules(&attach);
                // One outcome per patch, however many processes applied it.
                let mut told = false;
                for (pid, outcome) in patching.outcomes {
                    let to = serving(pid);
                    // A failure to build is every process's: a lone game's ID.
                    let pid = match (pid, &pids[..]) {
                        (Some(pid), _) => json!(pid),
                        (None, [pid]) => json!(pid),
                        (None, pids) => json!(pids),
                    };
                    match outcome {
                        Outcome::Applied(patched, applied, _) => {
                            let (build_ms, apply_ms) = (millis(patching.built), millis(applied));
                            event(&json!({
                                "event": "patched",
                                "pid": pid,
                                "files": files,
                                "attach": attach_files,
                                "build_ms": build_ms,
                                "apply_ms": apply_ms,
                                "ticks": patched.ticks,
                            }));
                            if !mem::replace(&mut told, true) {
                                self.outcome(json!({
                                    "event": "patched",
                                    "files": files,
                                    "build_ms": build_ms,
                                    "apply_ms": apply_ms,
                                }));
                                self.reload = self.reloads.last().cloned();
                            }
                            tell_attach(to, &attach);
                        }
                        Outcome::Failed(failure)
                            if matches!(failure.code.as_str(), "api.closed" | "api.timeout") =>
                        {
                            // Sent, but no answer came: the process may yet apply it, or may have,
                            // so what it runs isn't known. Look again at once, patching every edit
                            // since the build once each serves; an ended process gets them as it
                            // restarts.
                            if current {
                                self.applied.clear();
                                self.ran.clear();
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
                            self.outcome(json!({
                                "event": "patch_failed",
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

impl Patches {
    /// Notes how the restart in place `restart` went for process `pid` (`None` for a failure to
    /// build it), its patch of `files` having built in `built`, with the functions' prints
    /// `prints` after it; `current` if the processes are still those it was for, `to` the serving
    /// sessions it was for, which log what it told.
    ///
    /// Restarted, the process runs the sources the patch built, which edits are compared with from
    /// then on, and its snapshot is the run's newest ([`taken`](Self::taken)). A failure to build
    /// leaves the game as it was, as a patch's does. The game's refusal or failure rebuilds it
    /// instead, restoring the snapshot if the restart ended it.
    #[expect(clippy::too_many_arguments)]
    fn restarted(
        &mut self,
        pid: Option<u32>,
        outcome: Outcome,
        current: bool,
        files: &[PathBuf],
        prints: Option<&Prints>,
        restart: &Restarting,
        built: Duration,
        to: Vec<Session>,
    ) {
        let shown = match (pid, &self.processes[..]) {
            (Some(pid), _) | (None, &[pid]) => json!(pid),
            (None, pids) => json!(pids),
        };
        match outcome {
            Outcome::Applied(patched, applied, warned) => {
                let attach = if current {
                    self.layout = Some(restart.layout.clone());
                    self.applied = restart.layout.modules.clone();
                    self.ran.clear();
                    self.failed = None;
                    self.attached(prints)
                } else {
                    Vec::new()
                };
                let (build_ms, restart_ms) = (millis(built), millis(applied));
                let stages = patched.restarted.as_ref().map(|stages| json!(stages));
                event(&json!({
                    "event": "restarted",
                    "pid": shown,
                    "files": files,
                    "why": restart.why,
                    "attach": modules(&attach),
                    "build_ms": build_ms,
                    "restart_ms": restart_ms,
                    "stages": stages,
                    "ticks": patched.ticks,
                }));
                let (warnings, more) = runs::about_warnings(&warned, runs::MOST_WARNINGS);
                let mut outcome = json!({
                    "event": "restarted",
                    "files": files,
                    "why": restart.why,
                    "build_ms": build_ms,
                    "restart_ms": restart_ms,
                });
                if !warnings.is_empty() {
                    outcome["warnings"] = Value::Array(warnings);
                }
                if more > 0 {
                    outcome["more_warnings"] = json!(more);
                }
                self.outcome(outcome);
                self.reload = self.reloads.last().cloned();
                self.taken = (self.snapshot.as_deref())
                    .and_then(|path| run::written_since(path, restart.since));
                tell_attach(to, &attach);
            }
            Outcome::Failed(failure) => {
                event(&json!({
                    "event": "restart_failed",
                    "pid": shown,
                    "files": files,
                    "error": error(&failure),
                }));
                self.outcome(json!({
                    "event": "restart_failed",
                    "files": files,
                    "why": restart.why,
                    "error": error(&failure),
                }));
                let fields = json!({"files": files, "error": failure.code});
                // Unbuilt, the game runs its code as it was, as after a failed patch.
                if pid.is_none() {
                    let message = format!(
                        "A patch of {} to restart the game in place failed to build, so it runs \
                         its code as it was: {}",
                        listed(files),
                        failure.message
                    );
                    note(to, "gg.restart_failed", &message, &fields);
                    self.failed = Some(failure);
                    return;
                }
                let message = format!(
                    "A restart in place with a patch of {} failed, so gg rebuilds the game: {}",
                    listed(files),
                    failure.message
                );
                note(to, "gg.restart_failed", &message, &fields);
                // The game ended with its world in the snapshot, written before its layers ended.
                if failure.code == "api.restart_failed" {
                    self.taken = (self.snapshot.as_deref())
                        .and_then(|path| run::written_since(path, restart.since));
                }
                let why = format!("{}, after the restart in place failed", restart.why);
                self.rebuild(&restart.file, &why);
            }
            Outcome::Superseded => {}
            Outcome::Unreadable => {
                self.rebuild(&restart.file, "an edit gg can't read, which rustc builds");
            }
        }
    }
}

/// What a run watches while it has no build, its first having failed.
pub(crate) struct Watcher {
    sources: Sources,
    seen: Stamps,
    since: Instant,
    changed: bool,
}

impl Watcher {
    /// Watches the sources of `run`'s game.
    ///
    /// Fails with `gg.cargo` if cargo can't say what the package is, and `gg.build` if the
    /// workspace has no such package.
    pub(crate) fn new(run: &Run) -> Result<Self, Failure> {
        let sources = Sources::new(&build::package(&run.package, run.dir())?);
        let seen = sources.scan(None);
        Ok(Self {
            sources,
            seen,
            since: Instant::now(),
            changed: false,
        })
    }

    /// Scans at `now`; returns whether they changed since watching began and have since been quiet
    /// for [`QUIET`].
    pub(crate) fn changed(&mut self, now: Instant) -> bool {
        let stamps = self.sources.scan(None);
        if stamps != self.seen {
            (self.seen, self.since, self.changed) = (stamps, now, true);
        }
        self.changed && now.duration_since(self.since) >= QUIET
    }
}

/// A build or patch on its own thread.
enum Job {
    /// A rebuild, which `gg rebuild` asked for if `asked`; gives the build and its sources as they
    /// started.
    Rebuild {
        started: Instant,
        why: String,
        asked: Option<Asked>,
        thread: JoinHandle<Result<(Fat, Baseline), Failure>>,
    },
    /// A patch of edits to `files` for processes `pids`; `files`, `hashes` and `unreadable` are
    /// [`Act::Patch`]'s, `prints` the functions' prints after it, and `ran` how `files` run after
    /// it ([`ran_after`]). With `restart`, the process restarts in place with it.
    Patch {
        pids: Vec<u32>,
        files: Vec<PathBuf>,
        hashes: Hashes,
        unreadable: Option<PathBuf>,
        prints: Option<Prints>,
        ran: Ran,
        restart: Option<Box<Restarting>>,
        thread: JoinHandle<Patching>,
    },
}

/// A restart in place under way: the change it's for and why, the sources its patch built, which
/// its process runs once it's done, and when it started, before its snapshot.
struct Restarting {
    file: PathBuf,
    why: String,
    layout: Baseline,
    since: SystemTime,
}

impl Job {
    fn done(&self) -> bool {
        match self {
            Job::Rebuild { thread, .. } => thread.is_finished(),
            Job::Patch { thread, .. } => thread.is_finished(),
        }
    }
}

/// How a patch went: a library per process, the build time, and the outcome for each process, or
/// for all, `None`.
struct Patching {
    libs: Vec<PathBuf>,
    built: Duration,
    outcomes: Vec<(Option<u32>, Outcome)>,
}

enum Outcome {
    /// Applied, the game answering in this long; with a restart in place, its warnings since.
    Applied(Patched, Duration, Vec<Box<RawValue>>),
    Failed(Failure),
    /// The sources changed as it built: dropped, to be looked at again.
    Superseded,
    /// It built a module gg can't read: not applied, the game rebuilt.
    Unreadable,
}

enum Diff {
    /// In what a patch can't carry: a change to a file, for a reason.
    Rebuild(PathBuf, String),
    /// In what a patch carries only with a restart in place: a change to a module, for a reason;
    /// then as `Bodies`.
    Restart(
        PathBuf,
        String,
        Hashes,
        Option<PathBuf>,
        Option<Prints>,
        Texts,
    ),
    /// In function bodies of modules, if at all: each module's hash, the module gg can't read, if
    /// any, the functions' prints, unless a module doesn't parse, and the texts of the modules
    /// that differ from what the processes run.
    Bodies(Hashes, Option<PathBuf>, Option<Prints>, Texts),
}

#[derive(Debug, PartialEq)]
enum Act {
    /// The processes run what the sources hold, or take a patch of it once they serve.
    Nothing,
    /// A patch of these modules, after which each module's hash is as given; and the module gg
    /// can't read, if any.
    Patch(Vec<PathBuf>, Hashes, Option<PathBuf>),
    /// A patch and a restart in place, for a change to a module and why; then as `Patch`.
    Restart(PathBuf, String, Vec<PathBuf>, Hashes, Option<PathBuf>),
    Rebuild(PathBuf, String),
}

/// What the sources, as `diff` says, call for, given the processes (none while the game waits for
/// a rebuild), the modules' hashes as of the code they run or last ran, `applied`, and whether a
/// rebuild failed since the build, `stale`.
///
/// - A change a patch can't carry rebuilds.
/// - Processes that all serve take a patch of what changed since what they run, unless one can't
///   (its executable exports no `main`) or the build is stale; until all serve, nothing.
/// - A change a patch carries only with a restart in place restarts a game that can
///   ([`Session::hot`]), a lone process; another rebuilds.
/// - A game waiting for a rebuild (crashed or hung) gets one at any change since what its
///   processes ran, as a patch made after they restart could come too late.
/// - Without `patches`, as for a game built with cargo, what a patch would carry rebuilds.
fn act(diff: Diff, games: &[Process], applied: &Hashes, stale: bool, patches: bool) -> Act {
    let (hashes, unreadable, restart) = match diff {
        Diff::Rebuild(file, why) => return Act::Rebuild(file, why),
        Diff::Restart(file, why, hashes, unreadable, ..) => (hashes, unreadable, Some((file, why))),
        Diff::Bodies(hashes, unreadable, ..) => (hashes, unreadable, None),
    };
    let rebuild = |file: &PathBuf, why: &str| Act::Rebuild(file.clone(), why.to_owned());
    let sessions: Option<Vec<&Session>> = games.iter().map(|game| game.session).collect();
    match sessions {
        _ if games.is_empty() => match changed(applied, &hashes).first() {
            Some(file) => rebuild(file, "the game waits for a rebuild"),
            None => Act::Nothing,
        },
        None => Act::Nothing,
        Some(sessions) => {
            let files = changed(applied, &hashes);
            let Some(file) = files.first() else {
                return Act::Nothing;
            };
            if !patches {
                return rebuild(file, UNPATCHED);
            }
            if sessions.iter().any(|session| session.aslr_reference == 0) {
                let why = "the game can't take patches, as its executable exports no `main`";
                return rebuild(file, why);
            }
            if stale {
                let why = "a rebuild failed since the game's build, and patches would build with \
                           what it left";
                return rebuild(file, why);
            }
            match restart {
                None => Act::Patch(files, hashes, unreadable),
                Some((file, why)) if matches!(sessions[..], [session] if session.hot) => {
                    Act::Restart(file, why, files, hashes, unreadable)
                }
                Some((file, why)) => Act::Rebuild(file, why),
            }
        }
    }
}

/// Has the game apply `table` through `client`, restarting in place with its snapshot written at
/// `restart` if given. Returns its answer and, after a restart, what it warned of since.
fn apply(
    client: &mut Client,
    table: Value,
    restart: Option<PathBuf>,
) -> Result<(Patched, Vec<Box<RawValue>>), Failure> {
    let Some(snapshot) = restart else {
        let patched = client.call_as(
            "patch",
            Patch {
                table,
                restart: None,
            },
        )?;
        return Ok((patched, Vec::new()));
    };
    // Where the game's log stands, to read the restart's warnings from.
    let read = client.call_as::<LogEvents>("logs", json!({"last": 0}))?;
    let patch = Patch {
        table,
        restart: Some(snapshot),
    };
    let patched = client.call_as("patch", patch)?;
    let from = json!({"level": "warn", "from": read.next, "run": read.run});
    Ok((patched, runs::read_events(client, from)))
}

/// The modules whose hashes differ from `from` to `to`, and those `to` lacks, which a restart in
/// place removes.
fn changed(from: &Hashes, to: &Hashes) -> Vec<PathBuf> {
    let changed = (to.iter()).filter(|(path, hash)| from.get(*path) != Some(*hash));
    let removed = from.keys().filter(|path| !to.contains_key(*path));
    let mut changed: Vec<PathBuf> = changed
        .map(|(path, _)| path)
        .chain(removed)
        .cloned()
        .collect();
    changed.sort();
    changed
}

/// How the sources, with `stamps`, differ from what the processes run, `baseline`: the build's, or
/// the sources as of the last restart in place.
///
/// - A file outside the game's modules added, removed or changed needs a rebuild, as does an edit
///   to a function only `main` reaches ([`reach`]), given with that function's module if it
///   changed, else with the edited constant's.
/// - A module added or removed, or a change outside function bodies of modules, as
///   [`classify::classify`] tells, needs a restart in place, the first by path being the one
///   given. So does an edit to a constant used outside function bodies of the modules, as
///   [`classify::uses`] tells, given with the constant's module, and one that changes a function
///   only the game's start reaches.
/// - A new stamp with the same content is no change, nor is an edit, since the text a patch left in
///   a module (`ran`), of doc comments or tests alone.
fn diff(sources: &Sources, baseline: &Baseline, stamps: &Stamps, ran: &Ran) -> Diff {
    let added = (stamps.keys()).filter(|path| !baseline.files.contains_key(*path));
    let removed = (baseline.files.keys()).filter(|path| !stamps.contains_key(*path));
    // The first edit a patch carries only with a restart in place, and why.
    let mut restart: Option<(PathBuf, String)> = None;
    for file in added.chain(removed) {
        if !sources.module(file) {
            return Diff::Rebuild(file.clone(), OUTSIDE.to_owned());
        }
        restart.get_or_insert_with(|| (file.clone(), "a module added or removed".to_owned()));
    }
    let (mut hashes, mut unreadable) = (Hashes::new(), None);
    // The constants whose values differ, by name, with their modules; the edited modules' texts.
    let (mut consts, mut texts) = (Vec::new(), BTreeMap::new());
    for (path, stamp) in stamps {
        let module = sources.module(path);
        let Some(known) = baseline.files.get(path) else {
            // A module added, which only a restart in place carries.
            let Ok(content) = fs::read(path) else {
                return Diff::Rebuild(path.clone(), "a file gg can't read".to_owned());
            };
            hashes.insert(path.clone(), hash_of(&content));
            texts.insert(path.clone(), String::from_utf8_lossy(&content).into_owned());
            continue;
        };
        let mut hash = known.hash;
        if *stamp != known.stamp {
            let Ok(content) = fs::read(path) else {
                return Diff::Rebuild(path.clone(), "a file gg can't read".to_owned());
            };
            hash = hash_of(&content);
            if hash != known.hash {
                let text = String::from_utf8_lossy(&content).into_owned();
                // Only the game's modules have their text kept.
                let edit = match &known.text {
                    _ if module && tests_only(path, baseline) => Edit::Same,
                    Some(old) => classify::classify(old, &text),
                    None if module => Edit::Rebuild("an edit to a module that isn't UTF-8"),
                    None => Edit::Rebuild(OUTSIDE),
                };
                match edit {
                    // The build's own code is what the game runs: keep its hash.
                    Edit::Same => hash = known.hash,
                    Edit::Bodies { consts: edited } => {
                        consts.extend(edited.into_iter().map(|name| (name, path.clone())));
                        // The processes run what a patch left: keep its hash.
                        if let Some((patched, old)) = ran.get(path)
                            && classify::classify(old, &text) == Edit::Same
                        {
                            hash = *patched;
                        }
                    }
                    Edit::Unreadable => unreadable = Some(path.clone()),
                    Edit::Rebuild(why) if !module => {
                        return Diff::Rebuild(path.clone(), why.to_owned());
                    }
                    Edit::Rebuild(why) => {
                        restart.get_or_insert_with(|| (path.clone(), why.to_owned()));
                    }
                }
                texts.insert(path.clone(), text);
            }
        }
        if module {
            hashes.insert(path.clone(), hash);
        }
    }
    // The game's modules as they are now, but for those built only for tests.
    let now: Vec<(&Path, Option<&str>)> = (hashes.keys())
        .filter(|path| !tests_only(path, baseline))
        .map(|path| {
            let known = baseline
                .files
                .get(path)
                .and_then(|known| known.text.as_deref());
            (
                path.as_path(),
                texts.get(path).map(String::as_str).or(known),
            )
        })
        .collect();
    if !consts.is_empty() {
        let names: Vec<String> = consts.iter().map(|(name, _)| name.clone()).collect();
        let texts: Vec<&str> = now.iter().filter_map(|(_, text)| *text).collect();
        if let Uses::Rebuild(at, why) = classify::uses(&texts, &names) {
            restart.get_or_insert_with(|| (consts[at].1.clone(), why.to_owned()));
        }
    }
    let prints = match () {
        // A patch hears rustc's errors, and a build of it rebuilds.
        _ if unreadable.is_some() => None,
        _ if texts.is_empty() => baseline.prints.clone(),
        _ => read_prints(&now),
    };
    if let (Some(prints), Some(built)) = (&prints, &baseline.prints)
        && let Some((code, main)) = prints.started(built)
    {
        let file = match consts.first() {
            Some((_, constant)) if !texts.contains_key(&code.module) => constant.clone(),
            _ => code.module.clone(),
        };
        if main {
            let why = format!(
                "an edit changing `{}`, which only `main` calls, as the game's process starts",
                code.name
            );
            return Diff::Rebuild(file, why);
        }
        let why = format!(
            "an edit changing `{}`, which only code that runs as the game starts calls, as a \
             patch comes after",
            code.name
        );
        restart.get_or_insert((file, why));
    }
    match restart {
        Some((file, why)) => Diff::Restart(file, why, hashes, unreadable, prints, texts),
        None => Diff::Bodies(hashes, unreadable, prints, texts),
    }
}

/// How `files`, a patch's modules, run once it's applied: those whose hash after it, in `hashes`,
/// is their text's, in `texts`, run that text. The others run what the build or an earlier patch
/// left, whose own text gg compares edits with.
fn ran_after(files: &[PathBuf], hashes: &Hashes, texts: &Texts) -> Ran {
    let ran = files.iter().filter_map(|file| {
        let (hash, text) = (*hashes.get(file)?, texts.get(file)?);
        (hash == hash_of(text.as_bytes())).then(|| (file.clone(), (hash, text.clone())))
    });
    ran.collect()
}

/// The prints of the game's `modules`, each its path and text; `None` if one has no text, not
/// being UTF-8, or doesn't parse.
fn read_prints(modules: &[(&Path, Option<&str>)]) -> Option<Prints> {
    let modules: Option<Vec<(&Path, &str)>> = (modules.iter())
        .map(|&(path, text)| Some((path, text?)))
        .collect();
    reach::prints(&modules?)
}

/// Whether the module at `path` is built only for tests: declared apart under `#[cfg(test)]`, as
/// `#[cfg(test)] mod tests;`, in a module of the build's, as a file of its own or of one that is.
fn tests_only(path: &Path, baseline: &Baseline) -> bool {
    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem()) else {
        return false;
    };
    // `dir/name.rs` and `dir/name/mod.rs` are the module `name`, declared in a file of `dir`'s.
    let (name, dir) = if stem == "mod" {
        match (dir.file_name(), dir.parent()) {
            (Some(name), Some(parent)) => (name, parent),
            _ => return false,
        }
    } else {
        (stem, dir)
    };
    let name = name.to_string_lossy();
    let mut parents: Vec<PathBuf> = ["mod.rs", "main.rs", "lib.rs"]
        .iter()
        .map(|file| dir.join(file))
        .collect();
    if let (Some(up), Some(base)) = (dir.parent(), dir.file_name()) {
        parents.push(up.join(base).with_extension("rs"));
    }
    parents.retain(|parent| parent != path);
    parents.iter().any(|parent| {
        let Some(Known {
            text: Some(text), ..
        }) = baseline.files.get(parent)
        else {
            return false;
        };
        let root = parent
            .file_stem()
            .is_some_and(|stem| stem == "main" || stem == "lib");
        classify::declares_test(text, &name) || (!root && tests_only(parent, baseline))
    })
}

/// What patches watch, as cargo says of the game's package: its modules (the `.rs` files under its
/// binary's root module's directory, which a patch may carry edits of) and what rebuilds it: the
/// workspace's manifest and lock file, the game's manifest and build script, and what the last
/// build read, as its [`Inputs`] list, or before that, the directories of the workspace packages
/// it depends on. [`walk`] says what's skipped.
#[derive(Clone)]
struct Sources {
    modules: PathBuf,
    /// The game's package's directory, which its modules' directory may be.
    game: PathBuf,
    dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
    /// The workspace's lock file, among `files`.
    lock: PathBuf,
    /// The directory cargo builds into, whose files builds write rather than read.
    target: PathBuf,
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
            target: package.target.clone(),
        }
    }

    /// The directories and files watched, with `inputs`, but for the files those list.
    fn paths<'a>(&'a self, inputs: Option<&'a Inputs>) -> Vec<&'a Path> {
        let dirs = inputs.map_or(&self.dirs, |inputs| &inputs.dirs);
        let paths = iter::once(&self.modules).chain(dirs).chain(&self.files);
        paths.map(PathBuf::as_path).collect()
    }

    fn module(&self, path: &Path) -> bool {
        path.starts_with(&self.modules)
            && path.extension().is_some_and(|extension| extension == "rs")
            && !self.files.iter().any(|file| file == path)
    }

    /// The stamps of the files watched: with `inputs`, those of the build that listed them.
    fn scan(&self, inputs: Option<&Inputs>) -> Stamps {
        let mut stamps = Stamps::new();
        let root = self.modules == self.game;
        walk(
            &self.modules,
            root,
            &|path: &Path| self.module(path),
            &mut stamps,
        );
        // A package's directory leaves out what no build reads; a directory a build script watches
        // is read whole, as cargo does.
        let dirs = inputs.map_or(&self.dirs, |inputs| &inputs.dirs);
        for dir in dirs {
            walk(dir, inputs.is_none(), &|_: &Path| true, &mut stamps);
        }
        let listed = inputs.map_or(&[][..], |inputs| &inputs.files);
        for file in self.files.iter().chain(listed) {
            if let Ok(metadata) = fs::metadata(file) {
                stamps.insert(file.clone(), run::stamped(metadata));
            }
        }
        stamps
    }
}

/// What a build read, as cargo's dep-info beside its executable lists it: the files rustc read for
/// each path package the game builds with, its modules and what they embed (`include_str!`), and
/// what their build scripts watch. So an edit to a package's docs, or to a module built only for
/// its tests, isn't watched. Left out: what builds wrote, under the target directory, and the
/// `src` directories the game's build script watches whole for its source digest
/// (`gg_api::build`), which names the build alone.
#[derive(Clone, Debug, PartialEq)]
struct Inputs {
    /// The dep-info file.
    from: PathBuf,
    files: Vec<PathBuf>,
    /// Those a build script watches.
    dirs: Vec<PathBuf>,
}

impl Inputs {
    /// What the build of the executable at `exe` read; `None` if its dep-info can't be read.
    fn read(exe: &Path, sources: &Sources) -> Option<Self> {
        let from = exe.with_extension("d");
        let paths = dep_info(&fs::read_to_string(&from).ok()?)?;
        let (mut files, mut dirs) = (Vec::new(), Vec::new());
        for path in paths {
            if path.starts_with(&sources.target) {
                continue;
            }
            if !path.is_dir() {
                files.push(path);
            } else if !(path.ends_with("src") && path.with_file_name("Cargo.toml").is_file()) {
                dirs.push(path);
            }
        }
        Some(Self { from, files, dirs })
    }
}

/// The paths a dep-info file's first rule depends on, as cargo writes it: `target: path path…`,
/// a space in a path escaped by a backslash. `None` if it has no rule, or a path is relative, as
/// cargo's `build.dep-info-basedir` makes them.
fn dep_info(text: &str) -> Option<Vec<PathBuf>> {
    let line = text.lines().next()?;
    let (_, deps) = line.split_once(": ")?;
    let mut paths = Vec::new();
    let mut path = String::new();
    let mut chars = deps.chars().peekable();
    while let Some(char) = chars.next() {
        match char {
            '\\' if chars.peek() == Some(&' ') => path.push(chars.next()?),
            ' ' => paths.extend((!path.is_empty()).then(|| PathBuf::from(mem::take(&mut path)))),
            _ => path.push(char),
        }
    }
    paths.extend((!path.is_empty()).then(|| PathBuf::from(path)));
    paths.iter().all(|path| path.is_absolute()).then_some(paths)
}

/// Adds the stamp of each file under `dir` that `keep` keeps, skipping hidden entries (an editor's
/// locks), backups ending in `~`, other packages' directories, and, with `root` (`dir` is a
/// package's own), directories no build reads, [`UNBUILT`].
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

/// The sources as of a build's start, which edits are compared with.
#[derive(Clone)]
struct Baseline {
    files: BTreeMap<PathBuf, Known>,
    modules: Hashes,
    /// The prints of the functions of the game's modules, but for those built only for tests;
    /// `None` if gg can't read one.
    prints: Option<Prints>,
    /// What the build read, once it [narrowed](Self::narrow) `files` to them.
    inputs: Option<Inputs>,
}

#[derive(Clone)]
struct Known {
    stamp: Stamp,
    hash: u64,
    text: Option<String>,
}

impl Baseline {
    /// Takes every file a build might read, before it says what it read.
    fn new(sources: &Sources) -> Self {
        let (mut files, mut modules) = (BTreeMap::new(), Hashes::new());
        for (path, stamp) in sources.scan(None) {
            let content = fs::read(&path).unwrap_or_default();
            let hash = hash_of(&content);
            let mut text = None;
            if sources.module(&path) {
                modules.insert(path.clone(), hash);
                text = String::from_utf8(content).ok();
            }
            files.insert(path, Known { stamp, hash, text });
        }
        let mut baseline = Self {
            files,
            modules,
            prints: None,
            inputs: None,
        };
        let modules: Vec<(&Path, Option<&str>)> = (baseline.modules.keys())
            .filter(|path| !tests_only(path, &baseline))
            .map(|path| (path.as_path(), baseline.files[path].text.as_deref()))
            .collect();
        baseline.prints = read_prints(&modules);
        baseline
    }

    /// Narrows the files to what the build of the executable at `exe` read, as its [`Inputs`] list
    /// them; a file newly watched is taken as it is now. Keeps them all if the dep-info can't be
    /// read. A module added since the build's start, or a file watched still but removed, stays so,
    /// which rebuilds.
    fn narrow(&mut self, sources: &Sources, exe: &Path) {
        let Some(inputs) = Inputs::read(exe, sources) else {
            return;
        };
        let stamps = sources.scan(Some(&inputs));
        let watched = |path: &Path| {
            stamps.contains_key(path)
                || sources.module(path)
                || (sources.files.iter().chain(&inputs.files)).any(|file| file == path)
                || inputs.dirs.iter().any(|dir| path.starts_with(dir))
        };
        self.files.retain(|path, _| watched(path));
        for (path, stamp) in stamps {
            if !sources.module(&path) && !self.files.contains_key(&path) {
                let hash = hash_of(&fs::read(&path).unwrap_or_default());
                let text = None;
                self.files.insert(path, Known { stamp, hash, text });
            }
        }
        self.inputs = Some(inputs);
    }

    /// The sources as a restart in place's patch builds them: these, with the game's modules as
    /// `stamps` have them, those that differ holding `texts`, and the functions' prints `prints`.
    /// Its process runs them once restarted.
    fn restarted(
        &self,
        sources: &Sources,
        stamps: &Stamps,
        texts: &Texts,
        prints: Option<Prints>,
    ) -> Self {
        let mut files = self.files.clone();
        files.retain(|path, _| !sources.module(path) || stamps.contains_key(path));
        for (path, text) in texts {
            let Some(&stamp) = stamps.get(path) else {
                continue;
            };
            let (hash, text) = (hash_of(text.as_bytes()), Some(text.clone()));
            files.insert(path.clone(), Known { stamp, hash, text });
        }
        let modules = (files.iter())
            .filter(|(path, _)| sources.module(path))
            .map(|(path, known)| (path.clone(), known.hash))
            .collect();
        Self {
            files,
            modules,
            prints,
            inputs: self.inputs.clone(),
        }
    }

    /// Takes the non-module file at `path` as the build wrote it.
    fn refresh(&mut self, path: &Path) {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        let hash = hash_of(&fs::read(path).unwrap_or_default());
        let (stamp, text) = (run::stamped(metadata), None);
        self.files
            .insert(path.to_owned(), Known { stamp, hash, text });
    }

    fn stamps(&self) -> Stamps {
        let stamps = (self.files.iter()).map(|(path, known)| (path.clone(), known.stamp));
        stamps.collect()
    }
}

fn hash_of(content: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

fn recovered(failure: &Failure) -> Value {
    json!({"event": "recovered", "was": failure.code})
}

fn error(failure: &Failure) -> Value {
    json!({"code": failure.code, "message": failure.message})
}

/// Has each game of `sessions` log a warning through the API's `note` with `code`, `message` (cut
/// as [`fit`] says) and `fields`, an object, so that a client following its logs learns what
/// `gg run` prints. On its own thread, as an ended game takes a while to refuse the connection on
/// Windows. A game that doesn't take it (ended, or older than `note`) goes without.
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

/// Has each game of `sessions` log `gg.attach_changed` for the edits to `codes`, code only a
/// layer's `attach` reaches, if any.
fn tell_attach(sessions: Vec<Session>, codes: &[Code]) {
    if codes.is_empty() {
        return;
    }
    let files = modules(codes);
    let names: Vec<&str> = codes.iter().map(|code| code.name.as_str()).collect();
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    let message = format!(
        "Edits to {} change code only a layer's `attach` runs ({}), which builds the world as the \
         game starts afresh; this run keeps its world, as its restarts start from snapshots, so \
         `gg rebuild --fresh`, which starts the game afresh, shows the change",
        listed(&files),
        quoted.join(", ")
    );
    let fields = json!({"files": files, "functions": names});
    note(sessions, "gg.attach_changed", &message, &fields);
}

/// The modules of `codes`, which come sorted, each once.
fn modules(codes: &[Code]) -> Vec<PathBuf> {
    let mut modules: Vec<PathBuf> = codes.iter().map(|code| code.module.clone()).collect();
    modules.dedup();
    modules
}

fn listed(files: &[PathBuf]) -> String {
    let paths: Vec<String> = (files.iter())
        .map(|file| file.display().to_string())
        .collect();
    paths.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dep-info file's rule as cargo writes it: `target` depends on `deps`.
    fn rule(target: &Path, deps: &[PathBuf]) -> String {
        let escaped = |path: &Path| path.display().to_string().replace(' ', "\\ ");
        let deps: Vec<String> = deps.iter().map(|dep| escaped(dep)).collect();
        format!("{}: {}\n", escaped(target), deps.join(" "))
    }

    #[test]
    fn a_dep_info_file_gives_the_paths_its_first_rule_depends_on() {
        let root = std::env::temp_dir().join("gg dep info");
        let (exe, main, spaced) = (
            root.join("target/game"),
            root.join("src/main.rs"),
            root.join("my levels/one level.txt"),
        );
        let deps = vec![main, spaced];
        let text = rule(&exe, &deps) + &rule(&exe.with_extension("pdb"), &deps[..1]);
        assert_eq!(dep_info(&text), Some(deps));
        let relative = format!("{}: src/main.rs\n", exe.display());
        assert_eq!(dep_info(&relative), None, "a relative path");
        assert_eq!(dep_info("no rule\n"), None);
        assert_eq!(dep_info(""), None);
    }

    /// Once a build says what it read, its docs, a module built only for its tests, and what builds
    /// write aren't watched; what it embeds, a directory its build script watches, and a file
    /// outside the packages' directories are.
    #[test]
    fn a_build_watches_what_its_dep_info_says_it_read() {
        let root = std::env::temp_dir().join(format!("gg-inputs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("a directory")).expect("a directory");
            fs::write(path, text).expect("written");
        };
        let game = "fn main() {\n    run();\n}\n\nfn run() {\n    \
                    show(include_str!(\"level.txt\"));\n}\n";
        let tested = "pub fn tick() {}\n\n#[cfg(test)]\nmod tests;\n";
        for (path, text) in [
            ("Cargo.toml", "[workspace]"),
            ("Cargo.lock", "version = 4"),
            ("rust-toolchain.toml", "[toolchain]"),
            ("game/Cargo.toml", "[package]"),
            ("game/src/main.rs", game),
            ("game/src/level.txt", "a level"),
            ("engine/Cargo.toml", "[package]"),
            ("engine/AGENTS.md", "How to work here."),
            ("engine/src/lib.rs", tested),
            ("engine/src/tests.rs", "#[test]\nfn ticks() {}\n"),
            ("engine/shaders/sprite.slang", "a shader"),
            ("target/debug/build/engine/out/sprite.spv", "SPIR-V"),
            ("target/debug/game", "built"),
        ] {
            write(path, text);
        }
        let files = ["Cargo.toml", "Cargo.lock", "game/Cargo.toml"];
        let sources = Sources {
            modules: root.join("game/src"),
            game: root.join("game"),
            dirs: vec![root.join("engine")],
            files: files.iter().map(|file| root.join(file)).collect(),
            lock: root.join("Cargo.lock"),
            target: root.join("target"),
        };
        let mut baseline = Baseline::new(&sources);
        let exe = root.join("target/debug/game");
        baseline.narrow(&sources, &exe);
        assert_eq!(baseline.inputs, None, "no dep-info: all watched still");
        let listed = [
            "Cargo.lock",
            "Cargo.toml",
            "rust-toolchain.toml",
            "engine/Cargo.toml",
            "engine/shaders",
            "engine/src",
            "engine/src/lib.rs",
            "game/Cargo.toml",
            "game/src",
            "game/src/level.txt",
            "game/src/main.rs",
            "target/debug/build/engine/out/sprite.spv",
        ];
        let listed: Vec<PathBuf> = listed.iter().map(|path| root.join(path)).collect();
        fs::write(exe.with_extension("d"), rule(&exe, &listed)).expect("written");
        baseline.narrow(&sources, &exe);
        let inputs = baseline.inputs.clone().expect("inputs");
        assert_eq!(
            inputs.dirs,
            [root.join("engine/shaders")],
            "the digest's `src` left out"
        );
        assert!(
            !inputs
                .files
                .iter()
                .any(|file| file.starts_with(&sources.target))
        );

        let now = || {
            diff(
                &sources,
                &baseline,
                &sources.scan(Some(&inputs)),
                &Ran::new(),
            )
        };
        assert!(matches!(now(), Diff::Bodies(hashes, None, _, _) if hashes == baseline.modules));
        // Not read: docs, a module built for tests alone, what a build wrote. A walk of the
        // engine's directory would have rebuilt for the first two.
        write("engine/AGENTS.md", "How to work here, and why.");
        write("engine/src/tests.rs", "#[test]\nfn ticks_twice() {}\n");
        write("target/debug/build/engine/out/sprite.spv", "SPIR-V, again");
        assert!(matches!(now(), Diff::Bodies(hashes, None, _, _) if hashes == baseline.modules));
        // Read: each rebuilds, until undone.
        for (path, edited, was) in [
            ("game/src/level.txt", "a longer level", "a level"),
            (
                "rust-toolchain.toml",
                "[toolchain]\nchannel = \"1.99\"",
                "[toolchain]",
            ),
            ("engine/src/lib.rs", "pub fn tock() {}\n", tested),
            ("engine/shaders/sprite.slang", "another shader", "a shader"),
        ] {
            write(path, edited);
            let file = root.join(path);
            assert!(
                matches!(now(), Diff::Rebuild(at, why) if at == file && why == OUTSIDE),
                "{path}"
            );
            write(path, was);
            assert!(matches!(now(), Diff::Bodies(..)), "{path} undone");
        }
        // A file added to a directory a build script watches.
        write("engine/shaders/line.slang", "a line");
        let added = root.join("engine/shaders/line.slang");
        assert!(matches!(now(), Diff::Rebuild(at, _) if at == added));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn edits_inside_function_bodies_of_the_games_modules_patch_and_other_changes_rebuild() {
        let root = std::env::temp_dir().join(format!("gg-sources-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("a directory")).expect("a directory");
            fs::write(path, text).expect("written");
        };
        let game = "fn main() {\n    run(1);\n    title();\n}\n\nfn title() {\n    show(1);\n}\n\n\
                    fn run(_: u32) {\n    step(1);\n}\n\nimpl Layer for Game {\n    fn load(&mut \
                    self) {\n        paint(1);\n    }\n\n    fn attach(&mut self) {\n        \
                    spawn(1);\n    }\n\n    fn tick(&mut self) {\n        run(2);\n    }\n}\n";
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
            target: root.join("target"),
        };
        let baseline = Baseline::new(&sources);
        let (main, lib) = (
            root.join("game/src/main.rs"),
            root.join("engine/src/lib.rs"),
        );
        // Under `src`, a directory a package's root leaves unbuilt still holds modules.
        assert!(
            baseline
                .modules
                .contains_key(&root.join("game/src/tests/helper.rs"))
        );
        let now = || diff(&sources, &baseline, &sources.scan(None), &Ran::new());
        // Not watched: assets, a package's tests, an editor's lock and backup.
        write("game/assets/logo.png", "another logo");
        write("engine/tests/ticks.rs", "#[test]\nfn ticks_twice() {}\n");
        write("game/src/.#main.rs", "user@host.1234");
        write("engine/src/lib.rs~", "pub fn tick() {}\n");
        let Diff::Bodies(hashes, None, prints, _) = now() else {
            panic!("bodies");
        };
        assert_eq!(hashes, baseline.modules);
        assert!(prints.is_some() && prints == baseline.prints);
        // An edit in a function body the running game reaches; each edit here changes the file's
        // length, as one within the clock's resolution of the last may leave its time.
        write("game/src/main.rs", &game.replace("step(1)", "step(22)"));
        let Diff::Bodies(hashes, None, Some(_), _) = now() else {
            panic!("bodies");
        };
        assert_ne!(hashes[&main], baseline.modules[&main]);
        // Once a patch carried it, an edit of tests and doc comments alone keeps the patch's hash,
        // and one in a body doesn't.
        let Diff::Bodies(patched, None, _, texts) = now() else {
            panic!("bodies");
        };
        let ran = ran_after(std::slice::from_ref(&main), &patched, &texts);
        assert!(ran.contains_key(&main));
        let since = || diff(&sources, &baseline, &sources.scan(None), &ran);
        let stepped = game.replace("step(1)", "step(22)");
        let tested = format!("{stepped}\n/// A probe.\n#[cfg(test)]\nfn probe() {{}}\n");
        write("game/src/main.rs", &tested);
        let Diff::Bodies(hashes, None, _, _) = since() else {
            panic!("bodies");
        };
        assert_eq!(hashes[&main], patched[&main]);
        assert_ne!(hashes[&main], baseline.modules[&main]);
        write("game/src/main.rs", &game.replace("step(1)", "step(333)"));
        let Diff::Bodies(hashes, None, _, _) = since() else {
            panic!("bodies");
        };
        assert_ne!(hashes[&main], patched[&main]);
        // One in a layer's `attach`, which only a start afresh runs: told of against the world.
        let attached = game.replace("step(1)", "step(22)");
        write(
            "game/src/main.rs",
            &attached.replace("spawn(1)", "spawn(333)"),
        );
        let Diff::Bodies(_, None, Some(prints), _) = now() else {
            panic!("bodies");
        };
        let world = baseline.prints.as_ref().expect("prints");
        let told = prints.attached(world, &mut BTreeMap::new());
        let told: Vec<&str> = told.iter().map(|code| code.name.as_str()).collect();
        assert_eq!(told, ["<Game as Layer>::attach"]);
        // Text that doesn't parse.
        write("game/src/main.rs", &game.replace("step(1)", "step(1"));
        assert!(matches!(now(), Diff::Bodies(_, Some(file), _, _) if file == main));
        // A change outside the game's crate outweighs it, until undone.
        write("engine/src/lib.rs", "pub fn tick() {}\npub fn tock() {}\n");
        assert!(matches!(now(), Diff::Rebuild(file, why) if file == lib && why == OUTSIDE));
        write("engine/src/lib.rs", "pub fn tick() {}\n");
        assert!(matches!(now(), Diff::Bodies(..)));
        // A function re-signed patches. An edit outside function bodies, a module added, or one
        // in code only the game's start reaches, restarts the game in place, naming the function.
        write("game/src/main.rs", &game.replace("u32", "u128"));
        assert!(matches!(now(), Diff::Bodies(_, None, _, _)));
        write("game/src/main.rs", &game.replace("paint(1)", "paint(4444)"));
        let starts = "an edit to code that runs as the game starts, as a patch comes after";
        assert!(matches!(now(), Diff::Restart(file, why, ..) if file == main && why == starts));
        write(
            "game/src/main.rs",
            &game.replace("fn run(", "struct Ship;\n\nfn run("),
        );
        let outside = "an edit outside function bodies";
        let Diff::Restart(file, why, hashes, None, Some(_), texts) = now() else {
            panic!("a restart");
        };
        assert_eq!((file, why.as_str()), (main.clone(), outside));
        assert_ne!(hashes[&main], baseline.modules[&main]);
        assert!(texts[&main].contains("struct Ship;"));
        // One in code only `main` reaches, which a restart in place doesn't run again, rebuilds,
        // whatever else changed.
        write("game/src/main.rs", &game.replace("show(1)", "show(55555)"));
        let main_only = "an edit changing `title`, which only `main` calls, as the game's process \
                         starts";
        assert!(matches!(now(), Diff::Rebuild(file, why) if file == main && why == main_only));
        let ship = game.replace("fn run(", "struct Ship;\n\nfn run(");
        write("game/src/main.rs", &ship.replace("show(1)", "show(666666)"));
        assert!(
            matches!(now(), Diff::Rebuild(_, why) if why == main_only),
            "first"
        );
        write("game/src/main.rs", game);
        write("game/src/extra.rs", "");
        let added = "a module added or removed";
        let extra = root.join("game/src/extra.rs");
        let Diff::Restart(_, why, hashes, ..) = now() else {
            panic!("a restart");
        };
        assert_eq!(why, added);
        assert!(hashes.contains_key(&extra));
        let _ = fs::remove_dir_all(&root);
    }

    /// After a restart in place, edits are compared with the sources it built: an edit inside a
    /// function body patches, as the process lays out each type as they do. A new process runs the
    /// build's layouts again, against which the same sources restart it in place.
    #[test]
    fn edits_after_a_restart_in_place_are_compared_with_what_it_built() {
        let root = std::env::temp_dir().join(format!("gg-layout-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("a directory")).expect("a directory");
            fs::write(path, text).expect("written");
        };
        let game = "fn main() {}\n\nstruct Game {\n    lives: u32,\n}\n\nimpl Layer for Game {\n    \
                    fn tick(&mut self) {\n        step(1);\n    }\n}\n";
        write("Cargo.toml", "[workspace]");
        write("Cargo.lock", "version = 4");
        write("game/src/main.rs", game);
        let sources = Sources {
            modules: root.join("game/src"),
            game: root.join("game"),
            dirs: Vec::new(),
            files: vec![root.join("Cargo.toml"), root.join("Cargo.lock")],
            lock: root.join("Cargo.lock"),
            target: root.join("target"),
        };
        let baseline = Baseline::new(&sources);
        let main = root.join("game/src/main.rs");
        let fielded = game.replace("lives: u32,", "lives: u32,\n    score: u32,");
        write("game/src/main.rs", &fielded);
        write("game/src/extra.rs", "pub fn more() {}\n");
        let stamps = sources.scan(None);
        let Diff::Restart(_, _, hashes, None, prints, texts) =
            diff(&sources, &baseline, &stamps, &Ran::new())
        else {
            panic!("a restart");
        };
        let layout = baseline.restarted(&sources, &stamps, &texts, prints);
        assert_eq!(layout.modules, hashes);
        // As the restart left them, nothing; a body edited since patches.
        let since = |layout: &Baseline| diff(&sources, layout, &sources.scan(None), &Ran::new());
        let Diff::Bodies(same, None, Some(_), _) = since(&layout) else {
            panic!("bodies");
        };
        assert_eq!(same, layout.modules);
        write("game/src/main.rs", &fielded.replace("step(1)", "step(22)"));
        let Diff::Bodies(edited, None, Some(_), _) = since(&layout) else {
            panic!("bodies");
        };
        assert_eq!(
            changed(&layout.modules, &edited),
            std::slice::from_ref(&main)
        );
        // Against the build, as a new process runs, the same sources restart it again.
        assert!(matches!(since(&baseline), Diff::Restart(..)));
        // A module the restart added, removed since, restarts it.
        fs::remove_file(root.join("game/src/extra.rs")).expect("removed");
        let Diff::Restart(_, why, hashes, ..) = since(&layout) else {
            panic!("a restart");
        };
        assert_eq!(why, "a module added or removed");
        let extra = root.join("game/src/extra.rs");
        assert_eq!(changed(&layout.modules, &hashes), [extra, main]);
        let _ = fs::remove_dir_all(&root);
    }

    /// An edit to a constant patches where other modules' function bodies alone use it, as a
    /// layer's `attach`, which it tells of. A use in a type restarts the game in place, and one in a
    /// function only `main` reaches rebuilds, each naming the constant's module.
    #[test]
    fn edits_to_constants_patch_where_function_bodies_alone_use_them() {
        let root = std::env::temp_dir().join(format!("gg-consts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("a directory")).expect("a directory");
            fs::write(path, text).expect("written");
        };
        let consts = "pub const SPEED: f32 = 2.0;\npub const LANES: usize = 3;\n\
                      pub const START: f32 = 1.0;\npub const NAME: &str = \"Road\";\n";
        let game = "mod consts;\n\nuse consts::*;\n\nstruct Road([f32; LANES]);\n\n\
                    fn main() {\n    show();\n}\n\nfn show() {\n    println!(\"{NAME}\");\n}\n\n\
                    fn steer(x: &mut f32) {\n    *x += SPEED;\n}\n\n\
                    impl Layer for Game {\n    fn attach(&mut self) {\n        spawn(START);\n    \
                    }\n}\n";
        write("Cargo.toml", "[workspace]");
        write("Cargo.lock", "version = 4");
        write("game/Cargo.toml", "[package]");
        write("game/src/main.rs", game);
        write("game/src/consts.rs", consts);
        let files = ["Cargo.toml", "Cargo.lock", "game/Cargo.toml"];
        let sources = Sources {
            modules: root.join("game/src"),
            game: root.join("game"),
            dirs: Vec::new(),
            files: files.iter().map(|file| root.join(file)).collect(),
            lock: root.join("Cargo.lock"),
            target: root.join("target"),
        };
        let baseline = Baseline::new(&sources);
        let path = root.join("game/src/consts.rs");
        let now = || diff(&sources, &baseline, &sources.scan(None), &Ran::new());
        // Each edit here changes the file's length, as one within the clock's resolution of the
        // last may leave its time.
        write("game/src/consts.rs", &consts.replace("2.0", "3.75"));
        let Diff::Bodies(hashes, None, Some(_), _) = now() else {
            panic!("bodies");
        };
        assert_ne!(hashes[&path], baseline.modules[&path]);
        // One `attach` uses, told of against the world.
        write("game/src/consts.rs", &consts.replace("1.0", "-1.5"));
        let Diff::Bodies(_, None, Some(prints), _) = now() else {
            panic!("bodies");
        };
        let world = baseline.prints.as_ref().expect("prints");
        let told = prints.attached(world, &mut BTreeMap::new());
        let told: Vec<&str> = told.iter().map(|code| code.name.as_str()).collect();
        assert_eq!(told, ["<Game as Layer>::attach"]);
        // One a function only `main` reaches uses, in a format string: named with the constant's
        // module, the function's being unedited.
        write("game/src/consts.rs", &consts.replace("Road", "Long road"));
        let starts = "an edit changing `show`, which only `main` calls, as the game's process \
                      starts";
        assert!(matches!(now(), Diff::Rebuild(file, why) if file == path && why == starts));
        // One a type uses, which a restart in place carries.
        write("game/src/consts.rs", &consts.replace("3;", "4;"));
        let typed = classify::IN_A_TYPE;
        assert!(matches!(now(), Diff::Restart(file, why, ..) if file == path && why == typed));
        let _ = fs::remove_dir_all(&root);
    }

    /// What the game's binary doesn't hold, test code, changes nothing: the module's `mod tests {}`
    /// and the files declared apart under `#[cfg(test)]`, a file of one included.
    #[test]
    fn edits_to_test_code_change_nothing() {
        let root = std::env::temp_dir().join(format!("gg-tested-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("a directory")).expect("a directory");
            fs::write(path, text).expect("written");
        };
        let game = "mod ship;\n\n#[cfg(test)]\nmod tests;\n\nfn main() {\n    run(1);\n}\n\n\
                    fn run(_: u32) {}\n\n#[cfg(test)]\nmod checks {\n    fn one() {}\n}\n";
        write("Cargo.toml", "[workspace]");
        write("Cargo.lock", "version = 4");
        write("game/Cargo.toml", "[package]");
        write("game/src/main.rs", game);
        write("game/src/ship.rs", "pub fn sail() {}\n");
        write("game/src/tests.rs", "mod deep;\n\nfn test() {}\n");
        write("game/src/tests/deep.rs", "fn deeper() {}\n");
        let files = ["Cargo.toml", "Cargo.lock", "game/Cargo.toml"];
        let sources = Sources {
            modules: root.join("game/src"),
            game: root.join("game"),
            dirs: Vec::new(),
            files: files.iter().map(|file| root.join(file)).collect(),
            lock: root.join("Cargo.lock"),
            target: root.join("target"),
        };
        let baseline = Baseline::new(&sources);
        let now = || diff(&sources, &baseline, &sources.scan(None), &Ran::new());
        let unchanged = |diff: Diff| {
            let Diff::Bodies(hashes, None, _, _) = diff else {
                return false;
            };
            hashes == baseline.modules
        };
        // Each edit here changes the file's length, as one within the clock's resolution of the
        // last may leave its time.
        write(
            "game/src/main.rs",
            &game.replace("fn one() {}", "fn one() { two(); }"),
        );
        assert!(unchanged(now()), "an inline module of tests");
        write("game/src/main.rs", game);
        write("game/src/tests.rs", "mod deep;\n\nfn test() { help(); }\n");
        assert!(unchanged(now()), "a file declared under cfg(test)");
        write("game/src/tests/deep.rs", "struct Deeper;\n");
        assert!(
            unchanged(now()),
            "a file of a file declared under cfg(test)"
        );
        // A module the game builds is its own.
        write("game/src/ship.rs", "pub fn sail() { go(); }\n");
        let Diff::Bodies(hashes, None, _, _) = now() else {
            panic!("bodies");
        };
        let ship = root.join("game/src/ship.rs");
        assert_ne!(hashes[&ship], baseline.modules[&ship]);
        write("game/src/ship.rs", "pub struct Sail;\n\npub fn sail() {}\n");
        assert!(matches!(now(), Diff::Restart(file, ..) if file == ship));
        let _ = fs::remove_dir_all(&root);
    }

    /// A watcher reports a change only once it has settled.
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
            target: root.join("target"),
        };
        let start = Instant::now();
        let mut watcher = Watcher {
            seen: sources.scan(None),
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

    /// The run's state: waiting without processes, starting until each serves, then running
    /// current; changed once the sources change; not current after a failure, until a look finds
    /// the processes run what the sources hold; and, built with cargo, rebuilding at an edit a
    /// patch would carry.
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
            target: root.join("target"),
        };
        let baseline = Baseline::new(&sources);
        let seen = baseline.stamps();
        let mut patches = Patches {
            // Where cargo can't run: the rebuild fails at once.
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
            ran: Ran::new(),
            fresh: false,
            looked: seen.clone(),
            seen,
            since: Instant::now(),
            job: None,
            libs: Vec::new(),
            failed: None,
            reload: None,
            reloads: Vec::new(),
            asked: None,
            answered: None,
            world: None,
            told: BTreeMap::new(),
            baseline,
            layout: None,
            snapshot: None,
            taken: None,
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
            protocol: Some(gg_api::PROTOCOL),
            hot: false,
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
        // A look finding the processes run what the sources hold, as when an edit is undone.
        patches.fresh = true;
        patches.look(&serving);
        assert_eq!(told(&patches, &serving), ("running", Some(true), false));
        assert_eq!(
            recovered(&Failure::new("gg.build", "x")),
            json!({"event": "recovered", "was": "gg.build"})
        );
        // An edit a patch would carry rebuilds a cargo-built game, which here fails.
        fs::write(&main, "fn main() {}\n\nfn run() {\n    step(22);\n}\n").expect("written");
        patches.seen = patches.sources.scan(None);
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
        // An unanswered patch isn't a build failure: the run looks again at once; one that
        // failed to build is.
        for (code, failed) in [("api.closed", false), ("gg.build", true)] {
            patches.failed = None;
            let outcome = Outcome::Failed(Failure::new(code, "it went"));
            patches.job = Some(Job::Patch {
                pids: vec![7],
                files: vec![main.clone()],
                hashes: Hashes::new(),
                unreadable: None,
                prints: None,
                ran: Ran::new(),
                restart: None,
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
        // Failures are outcomes too, listed in order.
        let events: Vec<_> = patches
            .reloads
            .iter()
            .map(|outcome| outcome["event"].clone())
            .collect();
        assert_eq!(events, ["rebuild_failed", "patch_failed"]);
        // The warnings of the game that ends as a rebuild restarts it go with the rebuild before the
        // newest, the one that started it.
        patches.outcome(json!({"event": "rebuilt", "build_ms": 5}));
        patches.outcome(json!({"event": "rebuilt", "build_ms": 6}));
        let line = json!({"level": "WARN", "code": "snapshot.changed", "message": "lives added"});
        let line = serde_json::value::RawValue::from_string(line.to_string()).expect("a line");
        patches.warned(&[line]);
        let kept: Vec<_> = patches
            .reloads
            .iter()
            .map(|outcome| outcome.get("warnings"))
            .collect();
        assert_eq!(kept[..2], [None, None]);
        assert_eq!(
            kept[2],
            Some(&json!([{"code": "snapshot.changed", "message": "lives added"}]))
        );
        assert_eq!(kept[3], None);
        // A rebuild `gg rebuild` asks for waits for the job under way, the asks made meanwhile
        // one, afresh as one was, and answers how it went, here failing.
        let (go, going) = std::sync::mpsc::channel::<()>();
        patches.job = Some(Job::Patch {
            pids: vec![7],
            files: vec![main.clone()],
            hashes: Hashes::new(),
            unreadable: None,
            prints: None,
            ran: Ran::new(),
            restart: None,
            thread: thread::spawn(move || {
                let _ = going.recv();
                Patching {
                    libs: Vec::new(),
                    built: Duration::ZERO,
                    outcomes: vec![(Some(7), Outcome::Superseded)],
                }
            }),
        });
        patches.ask(Asked {
            at: 5,
            fresh: false,
        });
        patches.ask(Asked { at: 3, fresh: true });
        let quiet = || Instant::now() + QUIET;
        assert!(!patches.tend(&serving, quiet()));
        assert_eq!(patches.state(&serving).phase, "patching");
        go.send(()).expect("sent");
        while !patches.job.as_ref().is_some_and(Job::done) {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!patches.tend(&serving, quiet()));
        let state = patches.state(&serving);
        assert_eq!(
            (state.phase, state.why.as_deref()),
            ("rebuilding", Some(ASKED_FRESH))
        );
        assert_eq!(patches.answered(), None);
        while !patches.job.as_ref().is_some_and(Job::done) {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!patches.tend(&serving, quiet()));
        let answered = patches.answered();
        let answered = (answered.as_ref())
            .map(|(asked, failed)| (*asked, failed.as_ref().map(|failed| failed.code.as_str())));
        let one = Asked { at: 5, fresh: true };
        assert_eq!(answered, Some((one, Some("gg.build"))));
        assert_eq!(patches.answered(), None, "answered once");
        assert!(!patches.afresh(), "nothing to start afresh on");
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
            protocol: Some(gg_api::PROTOCOL),
            hot: false,
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
        let bodies = || Diff::Bodies(edited.clone(), None, None, Texts::new());
        // A change a patch can't carry rebuilds, whatever the process is doing.
        let lock = PathBuf::from("Cargo.lock");
        for games in [vec![], starting.clone(), serving(&patchable)] {
            let rebuild = Diff::Rebuild(lock.clone(), OUTSIDE.to_owned());
            let acted = act(rebuild, &games, &built, false, true);
            assert_eq!(acted, Act::Rebuild(lock.clone(), OUTSIDE.to_owned()));
        }
        // An edit in function bodies patches a serving process with what changed since what it
        // runs (nothing, once patched); a process not yet serving, once it does.
        let main = vec![PathBuf::from("game/src/main.rs")];
        let patch = Act::Patch(main.clone(), edited.clone(), None);
        assert_eq!(
            act(bodies(), &serving(&patchable), &built, false, true),
            patch
        );
        assert_eq!(
            act(bodies(), &serving(&patchable), &edited, false, true),
            Act::Nothing
        );
        assert_eq!(act(bodies(), &starting, &built, false, true), Act::Nothing);
        // Peers patch once both serve, and rebuild if either can't take one.
        let mut peers = serving(&patchable);
        peers.push(starting[0]);
        assert_eq!(act(bodies(), &peers, &built, false, true), Act::Nothing);
        peers[1].session = Some(&patchable);
        assert_eq!(act(bodies(), &peers, &built, false, true), patch);
        peers[1].session = Some(&unpatchable);
        let acted = act(bodies(), &peers, &built, false, true);
        assert!(matches!(acted, Act::Rebuild(..)), "{acted:?}");
        // What gg can't read goes with the patch.
        let unreadable = Diff::Bodies(edited.clone(), Some(main[0].clone()), None, Texts::new());
        let acted = act(unreadable, &serving(&patchable), &built, false, true);
        let carried = Act::Patch(main.clone(), edited.clone(), Some(main[0].clone()));
        assert_eq!(acted, carried);
        // A process that can't take patches rebuilds, as after a failed rebuild.
        for (session, stale) in [(&unpatchable, false), (&patchable, true)] {
            let acted = act(bodies(), &serving(session), &built, stale, true);
            assert!(matches!(acted, Act::Rebuild(..)), "{acted:?}");
        }
        // A game waiting for a rebuild gets one at a change since what it ran, an edit or one
        // undone after a patch, but not for sources as it had them.
        for (now, ran) in [(&edited, &built), (&built, &edited)] {
            let waiting = act(
                Diff::Bodies(now.clone(), None, None, Texts::new()),
                &[],
                ran,
                false,
                true,
            );
            assert!(matches!(waiting, Act::Rebuild(..)), "{waiting:?}");
        }
        assert_eq!(act(bodies(), &[], &edited, false, true), Act::Nothing);
        // Without patches, an edit a patch would carry rebuilds, and sources as run don't.
        let acted = act(bodies(), &serving(&patchable), &built, false, false);
        assert_eq!(acted, Act::Rebuild(main[0].clone(), UNPATCHED.to_owned()));
        let acted = act(bodies(), &serving(&patchable), &edited, false, false);
        assert_eq!(acted, Act::Nothing);
        // A change a patch carries only with a restart in place restarts a lone game that can,
        // and rebuilds one that can't, peers, and a game waiting for a rebuild.
        let (field, why) = (
            main[0].clone(),
            "an edit outside function bodies".to_owned(),
        );
        let restart = || {
            let (field, why) = (field.clone(), why.clone());
            Diff::Restart(field, why, edited.clone(), None, None, Texts::new())
        };
        let hot = Session {
            hot: true,
            ..session(26064)
        };
        let acted = act(restart(), &serving(&hot), &built, false, true);
        let restarts = Act::Restart(
            field.clone(),
            why.clone(),
            main.clone(),
            edited.clone(),
            None,
        );
        assert_eq!(acted, restarts);
        let rebuild = Act::Rebuild(field.clone(), why.clone());
        assert_eq!(
            act(restart(), &serving(&patchable), &built, false, true),
            rebuild
        );
        let mut both = serving(&hot);
        both.push(both[0]);
        assert_eq!(act(restart(), &both, &built, false, true), rebuild);
        let waiting = act(restart(), &[], &built, false, true);
        assert!(matches!(waiting, Act::Rebuild(..)), "{waiting:?}");
        let stale = act(restart(), &serving(&hot), &built, true, true);
        assert!(matches!(stale, Act::Rebuild(..)), "{stale:?}");
    }

    /// A module a restart in place removed counts as changed.
    #[test]
    fn a_module_removed_counts_as_changed() {
        let hashes = |paths: &[&str]| -> Hashes {
            (paths.iter())
                .map(|path| (PathBuf::from(path), 1))
                .collect()
        };
        let (was, now) = (hashes(&["a.rs", "b.rs"]), hashes(&["a.rs", "c.rs"]));
        let changed = changed(&was, &now);
        assert_eq!(changed, [PathBuf::from("b.rs"), PathBuf::from("c.rs")]);
    }
}
