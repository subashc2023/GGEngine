//! What a `gg run` tells of itself: a file in the target directory, beside its lock, rewritten
//! every [`WRITE`] and removed as the run ends. Other commands and `gg mcp`'s tools read it while
//! no game answers, to tell a rebuild under way from one that failed; `wait` waits on it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use gg_api::{Failure, LogEvents, Status};
use serde_json::{Value, json};

use crate::build;
use crate::game::{self, Pick};

/// How often the file is written, and `wait` reads it.
const WRITE: Duration = Duration::from_millis(200);

/// A file unwritten for this long is a killed run's.
const STALE: Duration = Duration::from_secs(5);

/// How long `wait` waits for a just-started `gg run`'s file, within its timeout: written once
/// cargo names the target directory.
const GRACE: Duration = Duration::from_secs(5);

/// The most files `wait` lists the game's last event of, the latest.
const MOST_FILE_EVENTS: usize = 20;

/// How many of the game's latest events at `info` or above `wait` reads for those about files.
const FILE_EVENTS_READ: usize = 1_000;

/// The fields of an event about a file that `wait` lists, of those it has.
const FILE_FIELDS: [&str; 7] = [
    "timestamp",
    "level",
    "code",
    "message",
    "path",
    "script",
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
    /// (ms since the epoch): `patched` with its `files`, `build_ms` and `apply_ms`, or `rebuilt`
    /// with its `why` and `build_ms`. A rebuild's game then starts again from a snapshot.
    pub(crate) reload: Option<Value>,
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

/// Milliseconds since the Unix epoch, the file's unit.
pub(crate) fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH);
    since.map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

/// A run's file, written by a thread on each [`tell`](Self::tell) and every [`WRITE`]; removed on
/// drop.
pub(crate) struct Told {
    path: PathBuf,
    gg: u32,
    shared: Arc<Mutex<Value>>,
    ending: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Told {
    /// Starts telling `state` at `path`, of `package`'s run in process `gg`, in the project at
    /// `project`, if any.
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
            gg,
            shared,
            ending,
            thread: Some(thread),
        }
    }

    /// Tells `state`, as of the look at the sources at `looked` (ms since the epoch).
    pub(crate) fn tell(&self, state: &State, looked: u64) {
        if let Ok(mut told) = self.shared.lock() {
            fill(&mut told, state, looked);
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
}

impl Drop for Told {
    fn drop(&mut self) {
        self.ending.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(quit_path(&self.path));
    }
}

fn fill(told: &mut Value, state: &State, looked: u64) {
    let error = (state.error.as_ref())
        .map(|failure| json!({"code": failure.code, "message": failure.message}));
    told["state"] = json!(state.phase);
    told["why"] = json!(state.why);
    told["pids"] = json!(state.pids);
    told["current"] = json!(state.current);
    told["error"] = error.unwrap_or(Value::Null);
    told["looked"] = json!(looked);
    told["reload"] = state.reload.clone().unwrap_or(Value::Null);
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
/// before this call, and has looked at the scene and script files it watches since; returns the
/// run's file, with `files_looked` and `files` ([`looked`]). The code holds once the run has
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
            return looked(pick, waited?, asked, start, timeout);
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

/// Waits, within `timeout` of `start`, until `pick`'s game has looked at its scene and script
/// files since `asked` (ms since the epoch), each saved before then applied or refused. Returns
/// `told` with `files_looked`, when it last looked, and `files`, the last event it logged of each
/// file ([`about_files`]) among its latest [`FILE_EVENTS_READ`]: not the events since the wait
/// began, since a look every 200 ms often applies a save before `gg` starts. A game that watches
/// none, or hasn't looked yet, as one just started, has nothing to wait for.
///
/// Fails with `gg.timeout` if it hasn't looked within `timeout`, as while its frames pause.
fn looked(
    pick: &Pick,
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
                 scene and script files within {} s, as while its frames pause",
                timeout.as_secs()
            );
            return Err(Failure::new("gg.timeout", message));
        }
        thread::sleep(WRITE);
    };
    told["files_looked"] = json!(looked);
    // A read stops at half a line's bytes: read on to the end, a few times at most.
    let mut params = json!({"level": "info", "last": FILE_EVENTS_READ});
    let mut events = Vec::new();
    for _ in 0..8 {
        let Ok(read) = client.call_as::<LogEvents>("logs", params) else {
            break;
        };
        events.extend(read.events);
        if read.next >= read.end {
            break;
        }
        params = json!({"level": "info", "from": read.next, "run": read.run});
    }
    told["files"] = Value::Array(about_files(&events));
    Ok(told)
}

/// The last of `events` about each scene and script file, by its `script` or a scene event's
/// `path`, in the order of those last events: at most [`MOST_FILE_EVENTS`] files', the latest,
/// each with [`FILE_FIELDS`].
fn about_files(events: &[Box<serde_json::value::RawValue>]) -> Vec<Value> {
    let events = events.iter();
    let events = events.filter_map(|event| serde_json::from_str::<Value>(event.get()).ok());
    let mut listed: Vec<(String, Value)> = Vec::new();
    for event in events {
        let scene = (event["code"].as_str()).is_some_and(|code| code.starts_with("scene."));
        let file = match (event["script"].as_str(), event["path"].as_str()) {
            (Some(script), _) => script,
            (None, Some(path)) if scene => path,
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
        let told = Told::start(at.clone(), 4242, &package, Some(&project), &waiting);
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
