//! Running a game's development build: supervised until it ends, or headless for a set number of
//! frames, as a check.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use gg_api::{Client, Failure, Pulse, Session};
use serde_json::{Value, json};

use crate::{build, game};

/// What `gg run` runs: a package, its profile, how long its frames may stop before it has hung,
/// and the arguments it's given.
pub(crate) struct Run {
    pub(crate) package: String,
    pub(crate) release: bool,
    pub(crate) hang: Duration,
    pub(crate) args: Vec<String>,
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

/// Prints `event`, a line of JSON.
fn event(event: &Value) {
    crate::print(&event.to_string());
}

/// Builds and runs `run`'s package, supervising it until it ends: returns whether it ended
/// cleanly, having printed a line as it started, as it served the tool API, and as it ended.
///
/// Fails with `gg.build` if it doesn't build, and `gg.run` if it can't start or be waited for.
pub(crate) fn run(run: &Run) -> Result<bool, Failure> {
    let exe = build::build(&run.package, run.release)?;
    let mut child = Command::new(&exe)
        .args(&run.args)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|error| failed(&exe, error))?;
    let pid = child.id();
    event(&json!({"event": "started", "pid": pid, "exe": exe}));
    let dir = exe.parent().unwrap_or(Path::new("."));
    let serves = !run.args.iter().any(|arg| arg == "--no-api");
    let mut session: Option<Session> = None;
    let mut client: Option<Client> = None;
    let (mut frames, mut moved) = (0, Instant::now());
    let mut pinged = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| failed(&exe, error))? {
            break status;
        }
        if serves && session.is_none() {
            let found = Session::list(&Session::dir(dir)).into_iter();
            session = found
                .map(|(_, session)| session)
                .find(|session| session.pid == pid);
            if let Some(session) = &session {
                let serving = json!({
                    "event": "serving",
                    "pid": pid,
                    "app": session.app,
                    "address": session.address,
                    "log": session.log,
                });
                event(&serving);
            }
        }
        if let Some(session) = &session
            && pinged.elapsed() >= PING
        {
            pinged = Instant::now();
            // A ping that fails counts as frames that stopped.
            if let Some(pulse) = ping(&mut client, session)
                && (pulse.frames != frames || pulse.waiting.is_some())
            {
                frames = pulse.frames;
                moved = Instant::now();
            }
            let limit = if frames == 0 { STARTUP } else { run.hang };
            if moved.elapsed() > limit {
                let _ = child.kill();
                let _ = child.wait();
                let log = log(dir, pid, Some(session));
                event(&json!({
                    "event": "hung",
                    "pid": pid,
                    "frames": frames,
                    "seconds": limit.as_secs(),
                    "log": log,
                    "errors": log.as_deref().map(errors).unwrap_or_default(),
                }));
                return Ok(false);
            }
        }
        thread::sleep(Duration::from_millis(100));
    };
    let log = log(dir, pid, session.as_ref());
    let errors = log.as_deref().map(errors).unwrap_or_default();
    let ended = if status.success() {
        "exited"
    } else {
        "crashed"
    };
    event(&json!({
        "event": ended,
        "pid": pid,
        "status": status.code(),
        "log": log,
        "errors": errors,
    }));
    Ok(status.success())
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
