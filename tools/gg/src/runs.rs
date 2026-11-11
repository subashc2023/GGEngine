//! What a `gg run` tells of itself as it runs, in a file of its own in the target directory, beside
//! its lock, which a thread of its own writes again every so often, and removes as the run ends: what
//! it's doing, as building, rebuilding or running the game, the game's processes, whether the code
//! they run holds every edit to the sources it watches, and the last build's or patch's failure,
//! until one succeeds. gg's other commands, and `gg mcp`'s tools, read it while no game answers,
//! so that an agent tells a rebuild under way from one that failed, and `wait` waits on it until
//! the code of the edits saved before it runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use gg_api::Failure;
use serde_json::{Value, json};

use crate::build;
use crate::game::Pick;

/// How often the file is written, at most, and how often `wait` reads it.
const WRITE: Duration = Duration::from_millis(200);

/// How long since the file was last written it's taken for the run's: a `gg run` that was killed
/// leaves its file, which then goes stale.
const STALE: Duration = Duration::from_secs(5);

/// How long `wait` waits for a run's file to come, for at most its timeout, as from a `gg run` just
/// started in the background, which writes it once cargo says where the target directory is.
const GRACE: Duration = Duration::from_secs(5);

/// What a `gg run` is doing, as its file tells.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct State {
    /// `building`, the first build; `starting`, as the game's processes start, before each serves;
    /// `running`, as each serves; `changed`, as the sources have changed, before the run acts on it;
    /// `patching`; `rebuilding`; `restarting`, rebuilt, as the game stops to start again from the
    /// new build; `stopping`; or `waiting`, the game having ended, for a change to rebuild it.
    pub(crate) phase: &'static str,
    /// Why, as the run's own events say it, if it's said.
    pub(crate) why: Option<String>,
    /// The game's processes.
    pub(crate) pids: Vec<u32>,
    /// Whether the code they run holds every edit to the sources the run watches, as of its last
    /// look at them; `None` while it watches none.
    pub(crate) current: Option<bool>,
    /// The failure of the last build or patch, until one succeeds or an edit undoes what failed.
    pub(crate) error: Option<Failure>,
}

impl State {
    pub(crate) fn new(phase: &'static str) -> Self {
        Self {
            phase,
            why: None,
            pids: Vec::new(),
            current: None,
            error: None,
        }
    }
}

/// The file of the `gg run` of `package` in target directory `target`.
pub(crate) fn path(target: &Path, package: &str) -> PathBuf {
    target.join(format!("gg-run-{package}.json"))
}

/// Milliseconds since the Unix epoch, as the file gives times, wherever it's read.
pub(crate) fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH);
    since.map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

/// A `gg run`'s file, written by a thread of its own as [`tell`](Self::tell) changes it and every
/// [`WRITE`] while it doesn't, and removed as this drops.
pub(crate) struct Told {
    path: PathBuf,
    shared: Arc<Mutex<Value>>,
    ending: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Told {
    /// Starts telling, at `path`, of the run of `package` in process `gg`, in the project whose
    /// manifest is `project`, if it's in one, `state`.
    pub(crate) fn start(
        path: PathBuf,
        gg: u32,
        package: &str,
        project: Option<&Path>,
        state: &State,
    ) -> Self {
        let mut told = json!({"gg": gg, "package": package, "project": project});
        fill(&mut told, state, now_ms());
        let shared = Arc::new(Mutex::new(told));
        let ending = Arc::new(AtomicBool::new(false));
        let (writing, ended, at) = (shared.clone(), ending.clone(), path.clone());
        let thread = thread::spawn(move || {
            while !ended.load(Ordering::Relaxed) {
                let told = writing.lock().map(|told| told.clone());
                if let Ok(mut told) = told {
                    told["updated"] = json!(now_ms());
                    write(&at, &told);
                }
                thread::sleep(WRITE);
            }
        });
        Self {
            path,
            shared,
            ending,
            thread: Some(thread),
        }
    }

    /// Tells `state`, as of the run's look at the sources at `looked`, ms since the Unix epoch.
    pub(crate) fn tell(&self, state: &State, looked: u64) {
        if let Ok(mut told) = self.shared.lock() {
            fill(&mut told, state, looked);
        }
    }
}

impl Drop for Told {
    fn drop(&mut self) {
        self.ending.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}

/// Fills `told`, a run's file's object, with `state`, as of a look at `looked`.
fn fill(told: &mut Value, state: &State, looked: u64) {
    let error = (state.error.as_ref())
        .map(|failure| json!({"code": failure.code, "message": failure.message}));
    told["state"] = json!(state.phase);
    told["why"] = json!(state.why);
    told["pids"] = json!(state.pids);
    told["current"] = json!(state.current);
    told["error"] = error.unwrap_or(Value::Null);
    told["looked"] = json!(looked);
}

/// Writes `told` at `path` whole, through a file beside it renamed over it, so that a reader never
/// reads half of it; one that can't be written, as while Windows has a reader hold it, is written
/// next time.
fn write(path: &Path, told: &Value) {
    let mut writing = path.as_os_str().to_owned();
    writing.push(".writing");
    let writing = PathBuf::from(writing);
    if fs::write(&writing, told.to_string()).is_ok() && fs::rename(&writing, path).is_err() {
        let _ = fs::remove_file(&writing);
    }
}

/// The files of the `gg run`s in target directory `target` that still run, as their files' being
/// written within [`STALE`] says, those of the project whose manifest is `project` alone if it's
/// given, comparing the paths canonical.
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

/// The run `pick` reaches the game of, as its project and `--pid` choose: the one there is, if
/// there's one; none of several, which `--pid` alone tells apart by their games' processes.
///
/// Fails with `gg.cargo` if cargo can't say where the target directory is.
fn picked(pick: &Pick) -> Result<Option<Value>, Failure> {
    let target = build::target_dir(pick.project.as_deref().and_then(Path::parent))?;
    let mut runs = runs_in(&target, pick.project.as_deref());
    if let Some(pid) = pick.pid {
        runs.retain(|told| {
            (told["pids"].as_array()).is_some_and(|pids| pids.contains(&json!(pid)))
        });
    }
    Ok(match runs.len() {
        1 => runs.pop(),
        _ => None,
    })
}

/// Why no game answers `pick`, if a `gg run` that runs it says so, as a failure with a code of its
/// own: `gg.rebuilding`, while it builds the game, or has built it and starts it again;
/// `gg.build_failed`, while the game, having ended, waits for a change to rebuild it after a build
/// that failed, with that build's errors; `gg.waiting`, while it waits so for another reason; and
/// `gg.starting`, as the game's processes start or stop. `None` if none does, or several.
pub(crate) fn down(pick: &Pick) -> Option<Failure> {
    let told = picked(pick).ok()??;
    Some(down_as(&told))
}

/// The failure that `told`, a run's file, gives while no game answers, as [`down`] says.
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

/// Waits until the game of the `gg run` `pick` reaches runs the code of every edit to the sources it
/// watches saved before this was called, for at most `timeout`, and returns the run's file as it
/// says so: once the run has looked at the sources since, and the game's processes serve, patched
/// or rebuilt with them, or with nothing to patch; or, for a run that watches no sources, as its
/// game serves.
///
/// Fails with `gg.build_failed` if the patch or the rebuild of an edit fails, with rustc's errors,
/// the game running its code as it was, or not at all; `gg.waiting` if the game ended and waits
/// for a change to rebuild it; `gg.no_run` if no `gg run` runs the game, its file not there within
/// [`GRACE`] of the call, nor once it's been, as when the run ends meanwhile;
/// `gg.which_run` if more than one does, which `--pid` tells apart; `gg.timeout` if the code isn't
/// running within `timeout`; and `gg.cargo` if cargo can't say where the target directory is.
pub(crate) fn wait(pick: &Pick, timeout: Duration) -> Result<Value, Failure> {
    let target = build::target_dir(pick.project.as_deref().and_then(Path::parent))?;
    let (asked, start) = (now_ms(), Instant::now());
    loop {
        let mut runs = runs_in(&target, pick.project.as_deref());
        if let Some(pid) = pick.pid {
            runs.retain(|told| {
                (told["pids"].as_array()).is_some_and(|pids| pids.contains(&json!(pid)))
            });
        }
        let told = match runs.len() {
            0 if start.elapsed() < GRACE.min(timeout) => {
                thread::sleep(WRITE);
                continue;
            }
            0 => {
                let message = "no `gg run` runs the game, as its file in the target directory \
                               says; `gg run` runs it, watching its sources";
                return Err(Failure::new("gg.no_run", message));
            }
            1 => runs.remove(0),
            _ => {
                let message = format!(
                    "{} `gg run`s run games; `--pid` chooses one by its game's process",
                    runs.len()
                );
                return Err(Failure::new("gg.which_run", message));
            }
        };
        if let Some(waited) = waited(&told, asked) {
            return waited;
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
        thread::sleep(WRITE);
    }
}

/// What `told`, a run's file, says of a wait that started at `asked`, ms since the Unix epoch:
/// `None` while it waits on, as it does until the run has looked at the sources since.
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
    use super::*;

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

    /// A run's file tells what it's doing as it changes, reads back while it's written and goes as
    /// the run ends; one left stale, as by a run that was killed, isn't read.
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
            &State::new("building"),
        );
        let mut state = State::new("running");
        state.pids = vec![42];
        state.current = Some(true);
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
        assert!(other.is_empty() && !gone && left.is_empty());
    }

    /// No game's answering says what its run does, with a code of its own, and the build's errors.
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

    /// A wait for a run whose file isn't there gives it a moment to come, within its timeout, then
    /// fails with `gg.no_run`.
    #[test]
    fn a_wait_without_a_run_waits_for_its_file_a_moment() {
        let start = Instant::now();
        let waited = wait(&pick(None), Duration::from_secs(1));
        let failed = waited.err().map(|failure| failure.code);
        assert_eq!(failed.as_deref(), Some("gg.no_run"));
        assert!(start.elapsed() >= Duration::from_secs(1) && start.elapsed() < GRACE * 2);
    }

    /// A wait goes on until the run has looked at the sources since it started, then ends as the
    /// game runs their code, or as their build failed.
    #[test]
    fn a_wait_ends_once_the_edits_code_runs_or_fails() {
        let told = |phase: &str, looked: u64, current: Option<bool>, error: Value| json!({"gg": 7, "state": phase, "looked": looked, "current": current, "error": error});
        let failed = json!({"code": "gg.build", "message": "error: expected `;`"});
        let at = |told: Value| waited(&told, 100).map(|waited| waited.map_err(|f| f.code));
        // Looked at before the wait started, whatever it said then.
        assert!(at(told("running", 99, Some(true), Value::Null)).is_none());
        // Since: an edit seen and not yet acted on, or being patched or rebuilt, waits on.
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
        // A run that watches nothing has its game's serving end it.
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
