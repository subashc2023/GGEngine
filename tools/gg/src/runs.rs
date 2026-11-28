//! What a `gg run` tells of itself: a file in the target directory, beside its lock, rewritten
//! as what it tells changes and every [`WRITE`], and removed as the run ends. Other commands and `gg mcp`'s tools read it while
//! no game answers, to tell a rebuild under way from one that failed; `wait` waits on it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use gg_api::{Failure, LogEvents, Status};
use serde_json::{Value, json};

use crate::build;
use crate::game::{self, Pick};

/// How often the file is written when what it tells hasn't changed, for its `updated`.
const WRITE: Duration = Duration::from_millis(200);

/// How often `wait` reads the file, and asks the game whether it looked at its files: the run
/// writes an outcome at once.
const READ: Duration = Duration::from_millis(25);

/// A file unwritten for this long is a killed run's.
const STALE: Duration = Duration::from_secs(5);

/// How long `wait` waits for a just-started `gg run`'s file, within its timeout: written once
/// cargo names the target directory.
const GRACE: Duration = Duration::from_secs(5);

/// The most files `wait` lists the game's last event of, the latest.
const MOST_FILE_EVENTS: usize = 20;

/// The most warnings `wait` lists after a rebuild.
pub(crate) const MOST_WARNINGS: usize = 20;

/// How many patch and rebuild outcomes the run keeps for `wait` to list.
pub(crate) const MOST_KEPT: usize = 20;

/// The most outcomes `wait` lists, the latest.
const MOST_RELOADS: usize = 10;

/// How many of the game's latest events at `info` or above `wait` reads for those about files.
const FILE_EVENTS_READ: usize = 1_000;

/// The fields of an event about a file that `wait` lists, of those it has.
const FILE_FIELDS: [&str; 9] = [
    "timestamp",
    "level",
    "code",
    "message",
    "path",
    "script",
    "ui",
    "prefab",
    "operations",
];

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct State {
    /// `building`, the first build; `starting`, before each process serves; `running`, each
    /// serves; `changed`, sources changed, not yet acted on; `patching`; `rebuilding`;
    /// `restarting`, rebuilt, the game stopping to start anew; `stopping`; or `waiting`, the game
    /// ended, for a change to rebuild it.
    pub(crate) phase: &'static str,
    /// Why, as the run's events say.
    pub(crate) why: Option<String>,
    pub(crate) pids: Vec<u32>,
    /// Whether the game's code holds every edit to the watched sources, as of the last look;
    /// `None` while none are watched.
    pub(crate) current: Option<bool>,
    /// The last build's or patch's failure, until one succeeds or an edit undoes it.
    pub(crate) error: Option<Failure>,
    /// The last patch or rebuild that succeeded, as its event told it, with `at`, when it landed
    /// (ms since the epoch): `patched` with its `files`, `build_ms`, `apply_ms` and `saved_ms`
    /// (from the newest file's save); `restarted`, restarted in place, with its `files`, `why`,
    /// `build_ms`, `restart_ms`, `saved_ms` and the game's `warnings` as it restored its world; or
    /// `rebuilt` with its `why` and `build_ms`. A
    /// rebuild's game then starts again from a snapshot.
    pub(crate) reload: Option<Value>,
    /// The latest [`MOST_KEPT`] outcomes, oldest first: [`reload`](Self::reload)'s kind, and
    /// `patch_failed`, `restart_failed` and `rebuild_failed` with their `error`, each with `at`. A
    /// rebuild's has `warnings` once its processes ended, restarting for the next.
    pub(crate) reloads: Vec<Value>,
}

impl State {
    pub(crate) fn new(phase: &'static str) -> Self {
        Self {
            phase,
            why: None,
            pids: Vec::new(),
            current: None,
            error: None,
            reload: None,
            reloads: Vec::new(),
        }
    }
}

pub(crate) fn path(target: &Path, package: &str) -> PathBuf {
    target.join(format!("gg-run-{package}.json"))
}

/// The file through which `gg quit` asks the run whose file is `told` to end ([`quit`]). It holds
/// the run's process ID, so a request left over never ends a later run.
fn quit_path(told: &Path) -> PathBuf {
    told.with_extension("quit")
}

/// The file through which `gg rebuild` asks the run whose file is `told` to rebuild the game
/// ([`rebuild`]), holding its process ID as [`quit_path`]'s does; `fresh`, to start it afresh too.
/// Two files, so neither request overwrites the other.
fn rebuild_path(told: &Path, fresh: bool) -> PathBuf {
    told.with_extension(if fresh { "fresh" } else { "rebuild" })
}

/// A `gg rebuild` the run took: when (ms since the epoch), and whether it asked to start the game
/// afresh. Requests taken together are one: the latest time, afresh if any asked to be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Asked {
    pub(crate) at: u64,
    pub(crate) fresh: bool,
}

impl Asked {
    /// This and `other`, as one request.
    pub(crate) fn and(self, other: Asked) -> Asked {
        Asked {
            at: self.at.max(other.at),
            fresh: self.fresh || other.fresh,
        }
    }
}

/// The file in which `wait` keeps when it last answered the run whose file is `told`: `at`, the
/// latest outcome it listed, and the run's process ID `gg`, so a file left over never reaches a
/// later run. Each `gg wait` is a process of its own, and `gg mcp`'s may outlive none of the
/// game's runs, so what was answered can't live in a `wait`. The run's own file is rewritten from
/// the run's memory, so a `wait` can't add to it. This one, beside it as the
/// requests are, is the run's too: it goes as the run ends.
fn answered_path(told: &Path) -> PathBuf {
    told.with_extension("answered")
}

/// The `at` of the latest outcome `wait` listed of run `gg`, in the file at `told`; 0 if none.
fn answered(told: &Path, gg: u64) -> u64 {
    let read = fs::read_to_string(answered_path(told));
    let read = read
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    read.filter(|read| read["gg"].as_u64() == Some(gg))
        .and_then(|read| read["at"].as_u64())
        .unwrap_or(0)
}

/// Records that `wait` listed run `gg`'s outcomes up to `until`. A write that fails lists them
/// again next time.
fn answer(told: &Path, gg: u64, until: u64) {
    write(&answered_path(told), &json!({"gg": gg, "at": until}));
}

/// Milliseconds since the Unix epoch, the file's unit.
pub(crate) fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH);
    since.map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

/// A run's file, written by a thread as a [`tell`](Self::tell) changes what it says, and every
/// [`WRITE`]; removed on drop.
pub(crate) struct Told {
    path: PathBuf,
    gg: u32,
    shared: Arc<Mutex<Value>>,
    wake: Arc<(Mutex<Wake>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

/// What wakes a [`Told`]'s writer before its next [`WRITE`].
#[derive(Default)]
struct Wake {
    changed: bool,
    ending: bool,
}

impl Told {
    /// Starts telling `state` at `path`, of `package`'s run in process `gg`, in the project at
    /// `project`, if any. `rebuilds`: whether the run builds the game itself, as `gg rebuild` asks.
    pub(crate) fn start(
        path: PathBuf,
        gg: u32,
        package: &str,
        project: Option<&Path>,
        rebuilds: bool,
        state: &State,
    ) -> Self {
        let mut told = json!({
            "gg": gg,
            "package": package,
            "project": project,
            "rebuilds": rebuilds,
            "rebuild": null,
        });
        fill(&mut told, state, now_ms());
        let shared = Arc::new(Mutex::new(told));
        let wake = Arc::new((Mutex::new(Wake::default()), Condvar::new()));
        let (writing, waking, at) = (shared.clone(), wake.clone(), path.clone());
        let thread = thread::spawn(move || {
            let (lock, woken) = &*waking;
            loop {
                let told = writing.lock().map(|told| told.clone());
                if let Ok(mut told) = told {
                    told["updated"] = json!(now_ms());
                    write(&at, &told);
                }
                let Ok(wake) = lock.lock() else {
                    return;
                };
                let waited =
                    woken.wait_timeout_while(wake, WRITE, |wake| !wake.changed && !wake.ending);
                let Ok((mut wake, _)) = waited else {
                    return;
                };
                if wake.ending {
                    return;
                }
                wake.changed = false;
            }
        });
        Self {
            path,
            gg,
            shared,
            wake,
            thread: Some(thread),
        }
    }

    /// Tells `state`, as of the look at the sources at `looked` (ms since the epoch).
    pub(crate) fn tell(&self, state: &State, looked: u64) {
        let changed = (self.shared.lock()).is_ok_and(|mut told| fill(&mut told, state, looked));
        if changed {
            self.wake(|wake| wake.changed = true);
        }
    }

    /// Wakes the writer, as `how` says why.
    fn wake(&self, how: impl FnOnce(&mut Wake)) {
        let (lock, woken) = &*self.wake;
        if let Ok(mut wake) = lock.lock() {
            how(&mut wake);
            woken.notify_one();
        }
    }

    /// Whether `gg quit` asked this run to end ([`quit`]); takes the request if so. The run asks
    /// only while it waits, with no game to answer `quit` itself.
    pub(crate) fn asked_to_quit(&self) -> bool {
        let at = quit_path(&self.path);
        let asked = fs::read_to_string(&at).is_ok_and(|gg| gg.trim() == self.gg.to_string());
        if asked {
            let _ = fs::remove_file(&at);
        }
        asked
    }

    /// When `gg rebuild` asked this run to rebuild the game ([`rebuild`]), if it did: now, as the
    /// requests are taken (ms since the epoch), afresh if one asked. Each was written before, so
    /// every `gg rebuild` that asked by then is answered by a rebuild started after.
    pub(crate) fn asked_to_rebuild(&self) -> Option<Asked> {
        let [rebuild, fresh] = [false, true].map(|fresh| {
            let at = rebuild_path(&self.path, fresh);
            let asked = fs::read_to_string(&at).is_ok_and(|gg| gg.trim() == self.gg.to_string());
            if asked {
                let _ = fs::remove_file(&at);
            }
            asked
        });
        (rebuild || fresh).then(|| Asked {
            at: now_ms(),
            fresh,
        })
    }

    /// Answers the `gg rebuild`s whose requests were taken by `asked`: with the rebuild's failure,
    /// or `None` once it succeeded. One that asked to start afresh waits for an answer that did.
    pub(crate) fn answer(&self, asked: Asked, failure: Option<&Failure>) {
        let error =
            failure.map(|failure| json!({"code": failure.code, "message": failure.message}));
        if let Ok(mut told) = self.shared.lock() {
            told["rebuild"] = json!({"asked": asked.at, "fresh": asked.fresh, "error": error});
        }
        self.wake(|wake| wake.changed = true);
    }
}

impl Drop for Told {
    fn drop(&mut self) {
        self.wake(|wake| wake.ending = true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(quit_path(&self.path));
        let _ = fs::remove_file(rebuild_path(&self.path, false));
        let _ = fs::remove_file(rebuild_path(&self.path, true));
        let _ = fs::remove_file(answered_path(&self.path));
    }
}

/// Fills `told` with `state`, as of the look at `looked`; returns whether it changed more than
/// `looked`, which each turn of the run's loop moves on.
fn fill(told: &mut Value, state: &State, looked: u64) -> bool {
    let error = (state.error.as_ref())
        .map(|failure| json!({"code": failure.code, "message": failure.message}));
    let fields = [
        ("state", json!(state.phase)),
        ("why", json!(state.why)),
        ("pids", json!(state.pids)),
        ("current", json!(state.current)),
        ("error", error.unwrap_or(Value::Null)),
        ("reload", state.reload.clone().unwrap_or(Value::Null)),
        ("reloads", json!(state.reloads)),
    ];
    let mut changed = false;
    for (key, value) in fields {
        if told[key] != value {
            told[key] = value;
            changed = true;
        }
    }
    told["looked"] = json!(looked);
    changed
}

/// Writes `told` through a file beside `path` renamed over it, so that a reader never reads half.
/// A write that fails, as while Windows has a reader hold the file, is redone next time.
fn write(path: &Path, told: &Value) {
    let mut writing = path.as_os_str().to_owned();
    writing.push(".writing");
    let writing = PathBuf::from(writing);
    if fs::write(&writing, told.to_string()).is_ok() && fs::rename(&writing, path).is_err() {
        let _ = fs::remove_file(&writing);
    }
}

/// The files of the runs in `target` written within [`STALE`], of `project` alone if given, by
/// canonical path.
fn runs_in(target: &Path, project: Option<&Path>) -> Vec<Value> {
    let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let project = project.map(canonical);
    let now = now_ms();
    let stale = u64::try_from(STALE.as_millis()).unwrap_or(u64::MAX);
    let entries = fs::read_dir(target).into_iter().flatten().flatten();
    let files = entries.filter(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.starts_with("gg-run-") && name.ends_with(".json")
    });
    let told = files.filter_map(|entry| {
        let text = fs::read_to_string(entry.path()).ok()?;
        serde_json::from_str::<Value>(&text).ok()
    });
    let live = told.filter(|told| {
        let updated = told["updated"].as_u64().unwrap_or(0);
        now.saturating_sub(updated) <= stale
    });
    let ours = live.filter(|told| match &project {
        None => true,
        Some(project) => (told["project"].as_str())
            .is_some_and(|theirs| canonical(Path::new(theirs)) == *project),
    });
    ours.collect()
}

/// The only run of `pick`'s project and `--pid`, with the target directory it's in; none of
/// several.
///
/// Fails with `gg.cargo` if cargo can't name the target directory.
fn picked(pick: &Pick) -> Result<Option<(PathBuf, Value)>, Failure> {
    let target = build::target_dir(pick.project.as_deref().and_then(Path::parent))?;
    let mut runs = runs_in(&target, pick.project.as_deref());
    if let Some(pid) = pick.pid {
        runs.retain(|told| {
            (told["pids"].as_array()).is_some_and(|pids| pids.contains(&json!(pid)))
        });
    }
    Ok(match runs.len() {
        1 => runs.pop().map(|told| (target, told)),
        _ => None,
    })
}

/// Why no game answers `pick`, if its `gg run` says:
///
/// - `gg.rebuilding`: it builds the game or starts it again.
/// - `gg.build_failed`: the game ended after a failed build, with its errors.
/// - `gg.waiting`: it waits for another reason.
/// - `gg.starting`: the processes start or stop.
///
/// `None` if no run, or several, do.
pub(crate) fn down(pick: &Pick) -> Option<Failure> {
    let (_, told) = picked(pick).ok()??;
    Some(down_as(&told))
}

/// Asks the `gg run` `pick` picks to end, as `gg quit` does when no game answers, if it waits with
/// none for a change to its sources, as after a failed build. Writes the request beside the run's
/// file ([`quit_path`]), which the run's waits poll, and returns `{"run": PID}`, its process, once
/// its file is gone.
///
/// # Errors
///
/// - `down`, why no game answers, if no run waits, or several do
/// - `gg.timeout` if the run doesn't end within the pick's timeout, as when it began a build first
/// - `gg.run` if the request can't be written
pub(crate) fn quit(pick: &Pick, down: Failure) -> Result<Value, Failure> {
    let Ok(Some((target, told))) = picked(pick) else {
        return Err(down);
    };
    let (Some(gg), Some(package)) = (told["gg"].as_u64(), told["package"].as_str()) else {
        return Err(down);
    };
    if told["state"] != "waiting" {
        return Err(down);
    }
    let at = path(&target, package);
    let request = quit_path(&at);
    fs::write(&request, gg.to_string()).map_err(|error| {
        let message = format!(
            "can't ask the `gg run` in process {gg} to end, writing {}: {error}",
            request.display()
        );
        Failure::new("gg.run", message)
    })?;
    let start = Instant::now();
    while start.elapsed() < pick.timeout {
        if !at.exists() {
            return Ok(json!({"run": gg}));
        }
        thread::sleep(WRITE);
    }
    let _ = fs::remove_file(&request);
    let message = format!(
        "the `gg run` in process {gg} didn't end within {} s of being asked, as when a change \
         began a build first; `gg quit` asks the game once it runs",
        pick.timeout.as_secs()
    );
    Err(Failure::new("gg.timeout", message))
}

fn down_as(told: &Value) -> Failure {
    let phase = told["state"].as_str().unwrap_or_default();
    let error = &told["error"];
    let code = match phase {
        "building" | "rebuilding" | "restarting" => "gg.rebuilding",
        "waiting" if !error.is_null() => "gg.build_failed",
        "waiting" => "gg.waiting",
        _ => "gg.starting",
    };
    let doing = match phase {
        "building" => "builds the game",
        "rebuilding" => "rebuilds the game",
        "restarting" => "has rebuilt the game, and starts it again",
        "waiting" => "waits for a change to the game's sources to rebuild it, as it ended",
        "stopping" => "stops the game",
        _ => "starts the game",
    };
    let gg = &told["gg"];
    let mut message = format!("no game answers, as the `gg run` in process {gg} {doing}");
    if let Some(why) = told["why"].as_str() {
        message.push_str(&format!(" ({why})"));
    }
    if let Some(failed) = error["message"].as_str() {
        message.push_str(&format!("; its last build failed: {failed}"));
    }
    message.push_str("; `gg wait` waits until the game runs the code of the edits saved before it");
    Failure::new(code, message)
}

/// Waits, for at most `timeout`, until `pick`'s game runs every edit to the watched sources saved
/// before this call, and has looked at the scene and script files it watches since, and reloaded
/// the UI and prefab files saved just before; returns the run's file, with `files_looked`, `files`
/// and the outcomes since the last answer, `reloads` ([`looked`]). The code holds once the run has
/// looked at the sources since and the processes serve, patched or rebuilt; with none watched,
/// once the game serves.
///
/// # Errors
///
/// - `gg.build_failed`: an edit's patch or rebuild failed, with rustc's errors; the game runs its
///   old code, or none.
/// - `gg.waiting`: the game ended and waits for a change to rebuild it.
/// - `gg.no_run`: no run: no file within [`GRACE`] of the call, or gone since.
/// - `gg.which_run`: several do; `--pid` tells them apart.
/// - `gg.timeout`: the code isn't running within `timeout`, or the game hasn't looked at its
///   files since.
/// - `gg.cargo`: cargo can't name the target directory.
pub(crate) fn wait(pick: &Pick, timeout: Duration) -> Result<Value, Failure> {
    let target = build::target_dir(pick.project.as_deref().and_then(Path::parent))?;
    let (asked, start) = (now_ms(), Instant::now());
    loop {
        let told = found(&target, pick, start, timeout)?;
        if let Some(waited) = waited(&told, asked) {
            return looked(pick, &target, waited?, asked, start, timeout);
        }
        if start.elapsed() >= timeout {
            let phase = told["state"].as_str().unwrap_or_default();
            let message = format!(
                "the game didn't run the code of the edits saved before `wait` within {} s; the \
                 run's state is `{phase}`",
                timeout.as_secs()
            );
            return Err(Failure::new("gg.timeout", message));
        }
        thread::sleep(READ);
    }
}

/// The file of the one run in `target` of `pick`'s project and `--pid`, waiting within [`GRACE`]
/// and `timeout` of `start` for a just-started one's.
///
/// # Errors
///
/// - `gg.no_run`: none, past the grace.
/// - `gg.which_run`: several.
fn found(target: &Path, pick: &Pick, start: Instant, timeout: Duration) -> Result<Value, Failure> {
    loop {
        let mut runs = runs_in(target, pick.project.as_deref());
        if let Some(pid) = pick.pid {
            runs.retain(|told| {
                (told["pids"].as_array()).is_some_and(|pids| pids.contains(&json!(pid)))
            });
        }
        match runs.len() {
            0 if start.elapsed() < GRACE.min(timeout) => thread::sleep(READ),
            0 => {
                let message = "no `gg run` runs the game, as its file in the target directory \
                               says; `gg run` runs it, watching its sources";
                return Err(Failure::new("gg.no_run", message));
            }
            1 => return Ok(runs.remove(0)),
            _ => {
                let message = format!(
                    "{} `gg run`s run games; `--pid` chooses one by its game's process",
                    runs.len()
                );
                return Err(Failure::new("gg.which_run", message));
            }
        }
    }
}

/// Asks the `gg run` `pick` picks to rebuild the game and start it again from a snapshot, as an
/// edit no patch carries does, through a request beside the run's file ([`rebuild_path`]). With
/// `fresh`, the game starts afresh instead, its layers' `attach` building the world again. The run
/// takes it at once, and rebuilds once no patch or build is under way. Then waits, as [`wait`]
/// does, until the game runs the new build; returns what `wait` does. All within `timeout`.
///
/// # Errors
///
/// - `gg.no_rebuild`: the run doesn't build the game itself, with `--no-patch` or restarts off.
/// - `gg.build_failed`: the rebuild failed, with rustc's errors; the game runs on as it was.
/// - `gg.timeout`: the run didn't rebuild, or the game didn't serve again, within `timeout`.
/// - `gg.run`: the request can't be written.
/// - As [`wait`] does: `gg.no_run`, `gg.which_run`, `gg.waiting`, `gg.cargo`.
pub(crate) fn rebuild(pick: &Pick, timeout: Duration, fresh: bool) -> Result<Value, Failure> {
    let target = build::target_dir(pick.project.as_deref().and_then(Path::parent))?;
    let start = Instant::now();
    let told = found(&target, pick, start, timeout)?;
    let (gg, package) = (&told["gg"], told["package"].as_str().unwrap_or_default());
    if told["rebuilds"] != true {
        let message = format!(
            "the `gg run` in process {gg} doesn't build the game itself: with `--no-patch`, it \
             restarts the game from a snapshot as you rebuild it; with restarts off (`--snapshot \
             0`, `--record`, `--replay` or `--no-api`), it never restarts it"
        );
        return Err(Failure::new("gg.no_rebuild", message));
    }
    let request = rebuild_path(&path(&target, package), fresh);
    // Before the request, which the run takes after: its answer's time is later.
    let asked = now_ms();
    fs::write(&request, gg.to_string()).map_err(|error| {
        let message = format!(
            "can't ask the `gg run` in process {gg} to rebuild the game, writing {}: {error}",
            request.display()
        );
        Failure::new("gg.run", message)
    })?;
    loop {
        let told = found(&target, pick, start, timeout)?;
        let answer = &told["rebuild"];
        let taken = answer["asked"].as_u64().is_some_and(|taken| taken >= asked);
        if taken && (!fresh || answer["fresh"] == true) {
            if let Some(failed) = answer["error"]["message"].as_str() {
                let message =
                    format!("the rebuild failed, so the game runs its code as it was: {failed}");
                return Err(Failure::new("gg.build_failed", message));
            }
            return wait(pick, timeout.saturating_sub(start.elapsed()));
        }
        if start.elapsed() >= timeout {
            let _ = fs::remove_file(&request);
            let phase = told["state"].as_str().unwrap_or_default();
            let message = format!(
                "the `gg run` in process {gg} didn't rebuild the game within {} s of being asked, \
                 as a patch or build under way goes first; the run's state is `{phase}`",
                timeout.as_secs()
            );
            return Err(Failure::new("gg.timeout", message));
        }
        thread::sleep(READ);
    }
}

/// Waits, within `timeout` of `start`, until `pick`'s game has looked at its scene, script and
/// asset files since `asked` (ms since the epoch), each saved before then applied or refused.
/// Returns `told` with:
///
/// - `files_looked`, when it last looked.
/// - `reloads`, every outcome the run had since the last `wait` answered, oldest first, at most
///   [`MOST_RELOADS`] and `reloads_more` before them ([`listed`]); `reload` stays the latest.
/// - `warnings` and `warnings_more` ([`about_warnings`]), what the games restarted by those rebuilds
///   warned of.
/// - `files`, the last event it logged of each scene, script, UI and prefab file
///   ([`about_files`]) among its latest [`FILE_EVENTS_READ`]: not the events since the wait began,
///   since a look every 200 ms often applies a save before `gg` starts.
///
/// A game that watches none, or hasn't looked yet, as one just started, has nothing to wait for.
///
/// Fails with `gg.timeout` if it hasn't looked within `timeout`, as while its frames pause.
fn looked(
    pick: &Pick,
    target: &Path,
    mut told: Value,
    asked: u64,
    start: Instant,
    timeout: Duration,
) -> Result<Value, Failure> {
    let (mut client, looked) = loop {
        // A game restarting answers once it serves again.
        if let Ok(mut client) = game::connect(pick)
            && let Ok(status) = client.call_as::<Status>("status", Value::Null)
            && status.files_looked.is_none_or(|looked| looked >= asked)
        {
            break (client, status.files_looked);
        }
        if start.elapsed() >= timeout {
            let message = format!(
                "the game runs the code of the edits saved before `wait`, but didn't look at its \
                 scene, script and asset files within {} s, as while its frames pause",
                timeout.as_secs()
            );
            return Err(Failure::new("gg.timeout", message));
        }
        thread::sleep(READ);
    };
    told["files_looked"] = json!(looked);
    let events = read_events(
        &mut client,
        json!({"level": "info", "last": FILE_EVENTS_READ}),
    );
    told["files"] = Value::Array(about_files(&events));
    let gg = told["gg"].as_u64().unwrap_or(0);
    let at = path(target, told["package"].as_str().unwrap_or_default());
    let shown = listed(&mut told, answered(&at, gg));
    if let Some(until) = shown.until {
        answer(&at, gg, until);
    }
    let mut tally = shown.warnings;
    if shown.rebuilt {
        // The game runs since the latest rebuild, which the listed always hold, being the latest
        // outcomes: all its events since it started are that rebuild's.
        let events = read_events(&mut client, json!({"level": "warn"}));
        tally_events(&mut tally, &events);
    }
    let (warnings, more) = told_warnings(tally, MOST_WARNINGS);
    if !warnings.is_empty() {
        told["warnings"] = Value::Array(warnings);
    }
    if more > 0 {
        told["warnings_more"] = json!(more);
    }
    Ok(told)
}

/// What [`listed`] found in the outcomes it listed.
#[derive(Debug, Default, PartialEq)]
struct Listed {
    /// The latest outcome's `at`: where the next `wait` goes on from.
    until: Option<u64>,
    /// Whether it listed a rebuild.
    rebuilt: bool,
    /// The warnings the listed rebuilds and restarts in place kept, as a tally
    /// ([`tally_events`]).
    warnings: Vec<(Value, Value, u64)>,
}

/// Sets `told["reloads"]` to the outcomes after `answered` (ms since the epoch), oldest first, the
/// latest [`MOST_RELOADS`], with `reloads_more` for those before; takes the warnings off the
/// rebuilds and restarts in place listed, which are told as `warnings`.
fn listed(told: &mut Value, answered: u64) -> Listed {
    let all = told["reloads"].as_array().cloned().unwrap_or_default();
    let at = |outcome: &Value| outcome["at"].as_u64().unwrap_or(0);
    let mut new: Vec<Value> = all
        .into_iter()
        .filter(|outcome| at(outcome) > answered)
        .collect();
    let more = new.len().saturating_sub(MOST_RELOADS);
    new.drain(..more);
    let mut found = Listed {
        until: new.last().map(at),
        ..Listed::default()
    };
    for outcome in &mut new {
        match outcome["event"].as_str() {
            Some("rebuilt") => found.rebuilt = true,
            Some("restarted") => {}
            _ => continue,
        }
        let kept = outcome
            .as_object_mut()
            .and_then(|outcome| outcome.remove("warnings"));
        for warning in kept.iter().filter_map(Value::as_array).flatten() {
            let count = warning["count"].as_u64().unwrap_or(1);
            tally_add(
                &mut found.warnings,
                warning["code"].clone(),
                warning["message"].clone(),
                count,
            );
        }
    }
    told["reloads"] = Value::Array(new);
    if more > 0 {
        told["reloads_more"] = json!(more);
    }
    found
}

/// The events the game's `logs` method answers to `params`, read on to the end a few times at most:
/// a read stops at half a line's bytes.
pub(crate) fn read_events(
    client: &mut gg_api::Client,
    mut params: Value,
) -> Vec<Box<serde_json::value::RawValue>> {
    let level = params["level"].clone();
    let mut events = Vec::new();
    for _ in 0..8 {
        let Ok(read) = client.call_as::<LogEvents>("logs", params) else {
            break;
        };
        events.extend(read.events);
        if read.next >= read.end {
            break;
        }
        params = json!({"level": level, "from": read.next, "run": read.run});
    }
    events
}

/// The warnings and errors among `events`, each with its `code` and `message`, and a `count` where
/// the same code and message came more than once, in the order first seen: at most `most`, and how
/// many more there were.
pub(crate) fn about_warnings(
    events: &[Box<serde_json::value::RawValue>],
    most: usize,
) -> (Vec<Value>, usize) {
    let mut listed = Vec::new();
    tally_events(&mut listed, events);
    told_warnings(listed, most)
}

/// A tally of warnings: code, message and how many came.
type Tally = Vec<(Value, Value, u64)>;

/// Adds the warnings and errors among `events` to `listed`.
fn tally_events(listed: &mut Tally, events: &[Box<serde_json::value::RawValue>]) {
    let events = events.iter();
    let events = events.filter_map(|event| serde_json::from_str::<Value>(event.get()).ok());
    for event in events {
        let severe = event["level"].as_str().is_some_and(|level| {
            level.eq_ignore_ascii_case("warn") || level.eq_ignore_ascii_case("error")
        });
        if severe {
            tally_add(listed, event["code"].clone(), event["message"].clone(), 1);
        }
    }
}

/// Adds `count` of the warning with `code` and `message` to `listed`, as a new one if it has none.
fn tally_add(listed: &mut Tally, code: Value, message: Value, count: u64) {
    match listed
        .iter_mut()
        .find(|(c, m, _)| *c == code && *m == message)
    {
        Some((.., counted)) => *counted += count,
        None => listed.push((code, message, count)),
    }
}

/// `listed` as JSON: at most `most` warnings, and how many more there were.
fn told_warnings(mut listed: Tally, most: usize) -> (Vec<Value>, usize) {
    let more = listed.len().saturating_sub(most);
    listed.truncate(most);
    let told = listed.into_iter().map(|(code, message, count)| {
        let mut told = json!({"code": code, "message": message});
        if count > 1 {
            told["count"] = json!(count);
        }
        told
    });
    (told.collect(), more)
}

/// The last of `events` about each scene, script, UI and prefab file, by its `script`, a scene
/// event's `path`, or the `ui` or `prefab` of `ui.reloaded` and `prefab.reloaded` and of a
/// warning, in the order of those last events: at most [`MOST_FILE_EVENTS`] files', the latest,
/// each with [`FILE_FIELDS`].
fn about_files(events: &[Box<serde_json::value::RawValue>]) -> Vec<Value> {
    let events = events.iter();
    let events = events.filter_map(|event| serde_json::from_str::<Value>(event.get()).ok());
    let mut listed: Vec<(String, Value)> = Vec::new();
    for event in events {
        let code = event["code"].as_str().unwrap_or_default();
        let scene = code.starts_with("scene.");
        let reloaded = matches!(code, "ui.reloaded" | "prefab.reloaded");
        let severe = event["level"].as_str().is_some_and(|level| {
            level.eq_ignore_ascii_case("warn") || level.eq_ignore_ascii_case("error")
        });
        let asset = event["ui"].as_str().or_else(|| event["prefab"].as_str());
        let file = match (event["script"].as_str(), event["path"].as_str(), asset) {
            (Some(script), ..) => script,
            (None, Some(path), _) if scene => path,
            (None, _, Some(asset)) if reloaded || severe => asset,
            _ => continue,
        };
        let file = file.to_owned();
        let fields = FILE_FIELDS.iter().filter_map(|&name| {
            let value = event.get(name).filter(|value| !value.is_null())?;
            Some((name.to_owned(), value.clone()))
        });
        listed.retain(|(listed, _)| *listed != file);
        listed.push((file, Value::Object(fields.collect())));
    }
    let over = listed.len().saturating_sub(MOST_FILE_EVENTS);
    listed.drain(..over);
    listed.into_iter().map(|(_, event)| event).collect()
}

/// The wait's end, from `told`, for a wait started at `asked` (ms since the epoch); else `None`.
fn waited(told: &Value, asked: u64) -> Option<Result<Value, Failure>> {
    if told["looked"].as_u64().unwrap_or(0) < asked {
        return None;
    }
    let phase = told["state"].as_str().unwrap_or_default();
    let current = told["current"].as_bool();
    let failed = told["error"]["message"].as_str();
    match (phase, current, failed) {
        ("running", Some(true) | None, _) => Some(Ok(told.clone())),
        ("running" | "waiting", Some(false) | None, Some(failed)) => {
            let message = format!(
                "the edits' build failed, so the game runs its code as it was, or not at all: \
                 {failed}"
            );
            Some(Err(Failure::new("gg.build_failed", message)))
        }
        ("waiting", _, None) => Some(Err(down_as(told))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    /// A turn's look alone doesn't write the file at once; what else it tells does.
    #[test]
    fn a_look_alone_is_no_change_to_write() {
        let mut told = json!({"gg": 1});
        assert!(fill(&mut told, &State::new("running"), 1));
        assert!(!fill(&mut told, &State::new("running"), 2));
        assert_eq!(told["looked"], 2);
        let patched = State {
            reloads: vec![json!({"event": "patched", "at": 3})],
            ..State::new("running")
        };
        assert!(fill(&mut told, &patched, 3));
        assert!(!fill(&mut told, &patched, 4));
        assert!(fill(&mut told, &State::new("changed"), 5));
    }

    fn pick(project: Option<PathBuf>) -> Pick {
        Pick {
            pid: None,
            app: None,
            timeout: Duration::from_secs(1),
            client: "gg".to_owned(),
            grant: None,
            project,
        }
    }

    /// Warnings and errors alone are listed, a repeat as a count, at most so many.
    #[test]
    fn warnings_repeat_as_a_count_and_stop_at_the_most() {
        let line = |level: &str, code: &str, message: &str| {
            let line = json!({"level": level, "code": code, "message": message}).to_string();
            serde_json::value::RawValue::from_string(line).expect("a line")
        };
        let events = vec![
            line("WARN", "snapshot.changed", "lives added"),
            line("INFO", "scene.applied", "applied"),
            line("WARN", "gg.attach_changed", "attach"),
            line("WARN", "snapshot.changed", "lives added"),
            line("ERROR", "asset.missing", "a.png"),
            line("WARN", "snapshot.changed", "lives added"),
            line("WARN", "snapshot.changed", "score added"),
        ];
        let (told, more) = about_warnings(&events, 3);
        let codes: Vec<_> = told
            .iter()
            .map(|told| told["code"].as_str().unwrap())
            .collect();
        assert_eq!(
            codes,
            ["snapshot.changed", "gg.attach_changed", "asset.missing"]
        );
        assert_eq!(
            (told[0]["count"].clone(), told[1].get("count")),
            (json!(3), None)
        );
        assert_eq!(more, 1);
        assert_eq!(about_warnings(&events[1..2], 3), (Vec::new(), 0));
    }

    /// A run's file follows its state, goes as the run ends; a stale one isn't read.
    #[test]
    fn a_runs_file_tells_what_it_does_until_it_ends() {
        let dir = std::env::temp_dir().join(format!("gg-runs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a directory");
        let manifest = dir.join("project.ron");
        fs::write(&manifest, "()").expect("written");
        let at = path(&dir, "shards");
        let told = Told::start(
            at.clone(),
            7,
            "shards",
            Some(&manifest),
            true,
            &State::new("building"),
        );
        let mut state = State::new("running");
        state.pids = vec![42];
        state.current = Some(true);
        let patched = json!({"event": "patched", "files": ["src/main.rs"], "at": 1200});
        state.reload = Some(patched.clone());
        told.tell(&state, 1234);
        thread::sleep(WRITE * 3);
        let read = runs_in(&dir, Some(&manifest));
        let other = runs_in(&dir, Some(&dir.join("other.ron")));
        drop(told);
        let gone = at.exists();
        let stale = json!({"gg": 8, "state": "running", "updated": 1});
        fs::write(path(&dir, "foxball"), stale.to_string()).expect("written");
        let left = runs_in(&dir, None);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(read.len(), 1);
        let read = &read[0];
        assert_eq!(
            (&read["gg"], &read["state"], &read["pids"], &read["looked"]),
            (&json!(7), &json!("running"), &json!([42]), &json!(1234))
        );
        assert_eq!(read["reload"], patched);
        assert!(other.is_empty() && !gone && left.is_empty());
    }

    /// A wait lists the last event the game logged of each scene and script file, with their own
    /// fields alone, the latest files'.
    #[test]
    fn a_wait_lists_the_last_event_the_game_logged_of_each_file() {
        let line = |event: Value| serde_json::value::to_raw_value(&event).expect("JSON");
        let mut events = vec![
            line(json!({"level": "INFO", "message": "Starting GGEngine", "pid": 7})),
            line(
                json!({"level": "WARN", "code": "scene.file", "path": "scenes/main.ron",
                        "message": "doesn't load"}),
            ),
            line(
                json!({"timestamp": "t", "level": "INFO", "code": "scene.applied",
                        "path": "scenes/main.ron", "operations": 2, "message": "Applied",
                        "target": "gg_engine", "line_number": 449}),
            ),
            line(
                json!({"level": "INFO", "script": "scripts/report.luau", "component": "Report",
                        "message": "Each `Report` holds its fields"}),
            ),
            line(json!({"level": "INFO", "script": "scripts/report.luau",
                        "message": "Script reloaded", "threadName": "main"})),
            line(
                json!({"level": "WARN", "code": "script.syntax", "script": "scripts/a.luau",
                        "message": "a.luau:3: expected"}),
            ),
            line(
                json!({"level": "WARN", "code": "asset.no_id", "path": "textures/a.png",
                        "message": "no ID"}),
            ),
        ];
        assert_eq!(
            about_files(&events),
            [
                json!({"timestamp": "t", "level": "INFO", "code": "scene.applied",
                       "message": "Applied", "path": "scenes/main.ron", "operations": 2}),
                json!({"level": "INFO", "message": "Script reloaded",
                       "script": "scripts/report.luau"}),
                json!({"level": "WARN", "code": "script.syntax", "message": "a.luau:3: expected",
                       "script": "scripts/a.luau"}),
            ]
        );
        let reload = |i| {
            json!({"level": "INFO", "script": format!("{i}.luau"),
                                "message": "Script reloaded"})
        };
        events.extend((0..MOST_FILE_EVENTS).map(|i| line(reload(i))));
        let listed = about_files(&events);
        let latest: Vec<Value> = (0..MOST_FILE_EVENTS).map(reload).collect();
        assert_eq!(listed, latest, "the latest files'");
    }

    /// Save twice, wait once: both outcomes; save, wait, save, wait: the second alone. The cursor
    /// is the run's file's, and a run's later file doesn't take an earlier one's.
    #[test]
    fn a_wait_lists_the_outcomes_since_the_last_answer() {
        let dir = std::env::temp_dir().join(format!("gg-answered-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a directory");
        let at = path(&dir, "shards");
        let patched = |at: u64| json!({"event": "patched", "files": ["src/main.rs"], "at": at});
        let rebuilt = json!({"event": "rebuilt", "why": "a field", "build_ms": 900, "at": 200});
        let failed = json!({"event": "patch_failed", "error": {"code": "gg.build"}, "at": 300});
        // What the run has told so far, and what each wait answers with.
        let mut told = json!({"gg": 7, "reloads": [rebuilt, patched(250)]});
        let wait = |told: &Value| {
            let mut told = told.clone();
            let shown = listed(&mut told, answered(&at, 7));
            if let Some(until) = shown.until {
                answer(&at, 7, until);
            }
            let events = told["reloads"].as_array().expect("reloads").iter();
            let events = events.map(|outcome| outcome["event"].as_str().unwrap().to_owned());
            (
                events.collect::<Vec<_>>(),
                told.get("reloads_more").cloned(),
                shown,
            )
        };
        let (both, ..) = wait(&told);
        assert_eq!(both, ["rebuilt", "patched"], "oldest first");
        let (none, _, shown) = wait(&told);
        assert!(none.is_empty() && shown.until.is_none(), "answered once");
        told["reloads"].as_array_mut().unwrap().push(failed);
        let (second, ..) = wait(&told);
        assert_eq!(second, ["patch_failed"], "the second alone");
        // Another run's answer isn't this run's.
        assert_eq!(answered(&at, 8), 0);
        assert_eq!(answered(&at, 7), 300);
        let _ = fs::remove_dir_all(&dir);
    }

    /// At most ten are listed, the latest, with how many came before; a rebuild's kept warnings
    /// join the tally.
    #[test]
    fn a_wait_lists_ten_and_tallies_the_warnings_of_every_rebuild() {
        let warning = |message: &str| json!({"code": "snapshot.changed", "message": message});
        let mut once = warning("lives added");
        once["count"] = json!(2);
        let outcomes: Vec<Value> = (1..=12)
            .map(|at| json!({"event": "patched", "files": ["a.rs"], "at": at}))
            .collect();
        let mut told = json!({"reloads": outcomes});
        let shown = listed(&mut told, 0);
        let kept = told["reloads"].as_array().unwrap();
        assert_eq!((kept.len(), &told["reloads_more"]), (10, &json!(2)));
        assert_eq!((kept[0]["at"].clone(), shown.until), (json!(3), Some(12)));
        assert!(!shown.rebuilt);
        let mut told = json!({"reloads": [
            {"event": "rebuilt", "at": 10, "warnings": [once, warning("score added")]},
            {"event": "rebuilt", "at": 20},
        ]});
        let shown = listed(&mut told, 0);
        assert!(shown.rebuilt);
        assert!(
            told["reloads"][0].get("warnings").is_none(),
            "told apart, as `warnings`"
        );
        let mut tally = shown.warnings;
        let line = |message: &str| {
            let line = json!({"level": "WARN", "code": "snapshot.changed", "message": message});
            serde_json::value::RawValue::from_string(line.to_string()).expect("a line")
        };
        tally_events(&mut tally, &[line("lives added")]);
        let (told, more) = told_warnings(tally, MOST_WARNINGS);
        assert_eq!(more, 0);
        assert_eq!(
            told,
            [
                json!({"code": "snapshot.changed", "message": "lives added", "count": 3}),
                warning("score added"),
            ]
        );
        // The later rebuild alone, since the answer.
        let mut told = json!({"reloads": [
            {"event": "rebuilt", "at": 10},
            {"event": "rebuilt", "at": 20},
        ]});
        let shown = listed(&mut told, 10);
        assert!(shown.rebuilt && shown.until == Some(20));
        let mut told = json!({"reloads": [
            {"event": "rebuilt", "at": 10},
            {"event": "rebuilt", "at": 20},
        ]});
        let shown = listed(&mut told, 20);
        assert!(!shown.rebuilt && shown.until.is_none());
    }

    /// A wait lists UI and prefab files as it does scenes and scripts.
    #[test]
    fn a_wait_lists_ui_and_prefab_files() {
        let line = |event: Value| serde_json::value::to_raw_value(&event).expect("JSON");
        let events = vec![
            line(
                json!({"timestamp": "t", "level": "INFO", "code": "ui.reloaded",
                        "ui": "ui/hud.ui", "message": "UI reloaded", "target": "gg_engine"}),
            ),
            line(
                json!({"level": "INFO", "code": "prefab.reloaded", "prefab": "prefabs/ship.prefab",
                        "message": "Prefab reloaded"}),
            ),
            line(
                json!({"level": "WARN", "code": "ui.parse", "ui": "ui/hud.ui",
                        "message": "expected a node"}),
            ),
            line(json!({"level": "INFO", "prefab": "prefabs/other.prefab", "message": "Loaded"})),
        ];
        assert_eq!(
            about_files(&events),
            [
                json!({"level": "INFO", "code": "prefab.reloaded", "message": "Prefab reloaded",
                       "prefab": "prefabs/ship.prefab"}),
                json!({"level": "WARN", "code": "ui.parse", "message": "expected a node",
                       "ui": "ui/hud.ui"}),
            ]
        );
    }

    /// `quit` ends a run that waits without a game, through a request it takes as its own alone;
    /// a run doing anything else is left to say why no game answers.
    #[test]
    fn quit_ends_a_run_that_waits_without_a_game() {
        // gg's directory, in the workspace, so cargo names its target directory; no project there
        // is any other run's.
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = build::target_dir(Some(here)).expect("the target directory");
        let project = here.join(format!("quit-{}.ron", std::process::id()));
        let package = format!("quit-{}", std::process::id());
        let at = path(&target, &package);
        let failed = Failure::new("gg.build", "error[E0308]: mismatched types");
        let waiting = State {
            error: Some(failed),
            ..State::new("waiting")
        };
        let told = Told::start(at.clone(), 4242, &package, Some(&project), true, &waiting);
        // Another run's request is left alone.
        fs::write(quit_path(&at), "4243").expect("written");
        let others = told.asked_to_quit();
        let down = || Failure::new("gg.build_failed", "its last build failed");
        // Running: not asked.
        told.tell(&State::new("running"), now_ms());
        thread::sleep(WRITE * 2);
        let running = quit(&pick(Some(project.clone())), down()).map_err(|failure| failure.code);
        told.tell(&waiting, now_ms());
        thread::sleep(WRITE * 2);
        // The run's side: its wait polls for the request, then ends.
        let run = thread::spawn(move || {
            let start = Instant::now();
            while !told.asked_to_quit() && start.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(100));
            }
        });
        let ended = quit(&pick(Some(project)), down()).map_err(|failure| failure.code);
        run.join().expect("no panic");
        let left = (at.exists(), quit_path(&at).exists());
        assert!(!others, "a request for another run");
        assert_eq!(running, Err("gg.build_failed".to_owned()));
        assert_eq!(ended, Ok(json!({"run": 4242})));
        assert_eq!(
            left,
            (false, false),
            "the run's file and the request are gone"
        );
    }

    /// `rebuild` asks a run that builds the game, through a request it takes as its own alone; a
    /// failed rebuild answers with its errors, and one that succeeded waits for the game.
    #[test]
    fn rebuild_asks_the_run_and_waits_for_its_answer() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = build::target_dir(Some(here)).expect("the target directory");
        let project = here.join(format!("rebuild-{}.ron", std::process::id()));
        let package = format!("rebuild-{}", std::process::id());
        let at = path(&target, &package);
        let running = State {
            pids: vec![4244],
            current: Some(true),
            ..State::new("running")
        };
        let ask = |timeout, fresh| {
            let pick = pick(Some(project.clone()));
            let asked = rebuild(&pick, Duration::from_secs(timeout), fresh);
            asked.map_err(|failure| (failure.code, failure.message))
        };
        // A run that doesn't build the game itself.
        let told = Told::start(at.clone(), 4242, &package, Some(&project), false, &running);
        thread::sleep(WRITE * 2);
        let refused = ask(1, false).map_err(|(code, _)| code);
        drop(told);
        let told = Told::start(at.clone(), 4242, &package, Some(&project), true, &running);
        // Another run's request is left alone.
        fs::write(rebuild_path(&at, false), "4243").expect("written");
        let others = told.asked_to_rebuild();
        let _ = fs::remove_file(rebuild_path(&at, false));
        // Both kinds at once are one request, afresh.
        for fresh in [false, true] {
            fs::write(rebuild_path(&at, fresh), "4242").expect("written");
        }
        let both = told.asked_to_rebuild().map(|asked| asked.fresh);
        // The run's side: it takes each request, failing the first rebuild and not the second.
        // Asked afresh, a rebuild under way that wasn't lands first, failing.
        let asking = Arc::new(AtomicBool::new(true));
        let going = asking.clone();
        let run = thread::spawn(move || {
            let failed = || Some(Failure::new("gg.build", "error[E0308]: mismatched types"));
            let mut answers = [failed(), None].into_iter();
            let mut taken = Vec::new();
            while going.load(Ordering::Relaxed) {
                if let Some(asked) = told.asked_to_rebuild() {
                    if asked.fresh {
                        let under_way = Asked {
                            fresh: false,
                            ..asked
                        };
                        told.answer(under_way, failed().as_ref());
                        thread::sleep(WRITE * 3);
                        told.answer(asked, None);
                    } else {
                        told.answer(asked, answers.next().flatten().as_ref());
                    }
                    taken.push(asked.fresh);
                }
                told.tell(&running, now_ms());
                thread::sleep(Duration::from_millis(50));
            }
            taken
        });
        let failed = ask(5, false);
        // Answered, the run serving: the wait goes on to the game, which never answers here.
        let rebuilt = ask(2, false);
        let fresh = ask(3, true);
        asking.store(false, Ordering::Relaxed);
        let taken = run.join().expect("no panic");
        let left = [
            at.clone(),
            rebuild_path(&at, false),
            rebuild_path(&at, true),
        ]
        .map(|at| at.exists());
        assert_eq!(refused, Err("gg.no_rebuild".to_owned()));
        assert_eq!(others, None, "a request for another run");
        assert_eq!(both, Some(true));
        assert_eq!(taken, [false, false, true]);
        let Err((code, message)) = failed else {
            panic!("a failed rebuild answers {failed:?}");
        };
        assert!(
            code == "gg.build_failed" && message.contains("E0308"),
            "{message}"
        );
        let past = "the game runs the code of the edits saved before `wait`";
        for rebuilt in [rebuilt, fresh] {
            let Err((code, message)) = rebuilt else {
                panic!("no game answers here, but {rebuilt:?}");
            };
            assert!(
                code == "gg.timeout" && message.starts_with(past),
                "{message}"
            );
        }
        assert_eq!(left, [false; 3], "the run's file and requests are gone");
    }

    /// A game down says what its run does, with the build's errors.
    #[test]
    fn a_game_down_says_what_its_run_does() {
        let told = |phase: &str, error: Value| json!({"gg": 7, "state": phase, "why": null, "error": error});
        let failed = json!({"code": "gg.build", "message": "error[E0308]: mismatched types"});
        let codes: Vec<String> = [
            told("rebuilding", Value::Null),
            told("building", Value::Null),
            told("waiting", failed.clone()),
            told("waiting", Value::Null),
            told("starting", Value::Null),
        ]
        .iter()
        .map(|told| down_as(told).code)
        .collect();
        assert_eq!(
            codes,
            [
                "gg.rebuilding",
                "gg.rebuilding",
                "gg.build_failed",
                "gg.waiting",
                "gg.starting"
            ]
        );
        let message = down_as(&told("waiting", failed)).message;
        assert!(
            message.contains("E0308") && message.contains("gg wait"),
            "{message}"
        );
        // A pick outside any run's project finds none.
        assert!(down(&pick(Some(PathBuf::from("/nowhere/project.ron")))).is_none());
    }

    /// A wait for a missing file gives it a moment, then fails with `gg.no_run`.
    #[test]
    fn a_wait_without_a_run_waits_for_its_file_a_moment() {
        // gg's directory holds no project, so the editor's run isn't it; it's in the workspace,
        // so cargo names its target directory.
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("project.ron");
        // Two of the pauses between its looks for the file.
        let timeout = WRITE * 2;
        let start = Instant::now();
        let waited = wait(&pick(Some(project)), timeout);
        let failed = waited.err().map(|failure| failure.code);
        assert_eq!(failed.as_deref(), Some("gg.no_run"));
        assert!(start.elapsed() >= timeout && start.elapsed() < GRACE * 2);
    }

    /// A wait goes on until the run looks at the sources, then ends on their code or failed build.
    #[test]
    fn a_wait_ends_once_the_edits_code_runs_or_fails() {
        let told = |phase: &str, looked: u64, current: Option<bool>, error: Value| json!({"gg": 7, "state": phase, "looked": looked, "current": current, "error": error});
        let failed = json!({"code": "gg.build", "message": "error: expected `;`"});
        let at = |told: Value| waited(&told, 100).map(|waited| waited.map_err(|f| f.code));
        // Looked before the wait started: waits on.
        assert!(at(told("running", 99, Some(true), Value::Null)).is_none());
        // Since: an edit unacted on, patching or rebuilding waits on.
        for phase in [
            "changed",
            "patching",
            "rebuilding",
            "restarting",
            "starting",
        ] {
            assert!(
                at(told(phase, 100, Some(false), Value::Null)).is_none(),
                "{phase}"
            );
        }
        assert!(matches!(
            at(told("running", 100, Some(true), Value::Null)),
            Some(Ok(_))
        ));
        // A run that watches nothing ends when its game serves.
        assert!(matches!(
            at(told("running", 100, None, Value::Null)),
            Some(Ok(_))
        ));
        let build_failed = Some(Err("gg.build_failed".to_owned()));
        assert_eq!(
            at(told("running", 100, Some(false), failed.clone())),
            build_failed
        );
        assert_eq!(at(told("waiting", 100, Some(false), failed)), build_failed);
        assert_eq!(
            at(told("waiting", 100, Some(false), Value::Null)),
            Some(Err("gg.waiting".to_owned()))
        );
    }
}
