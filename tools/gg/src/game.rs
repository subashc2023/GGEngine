//! The running games: finding them by their session files, talking to them, and reading their
//! logs.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gg_api::{Client, Failure, Pulse, Session};
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::build;

/// How long a connection to a game may take: it's on the loopback, where only one that has ended
/// takes any time, refusing it.
const CONNECT: Duration = Duration::from_secs(2);

/// Which running game to talk to, and how long to wait for its answers.
pub(crate) struct Pick {
    pub(crate) pid: Option<u32>,
    pub(crate) app: Option<String>,
    pub(crate) timeout: Duration,
}

/// The session files of the games built in the workspace: those in the `sessions` directory of
/// each of its target directory's profiles.
fn session_files() -> Result<Vec<(PathBuf, Session)>, Failure> {
    let target = build::target_dir()?;
    let profiles = fs::read_dir(&target).into_iter().flatten().flatten();
    let sessions = profiles.flat_map(|profile| Session::list(&Session::dir(&profile.path())));
    Ok(sessions.collect())
}

/// Connects to the game that `session`, read from the file at `path`, names, removing the file if
/// the game has ended, which a crash or a kill leaves it behind for.
fn connect_to(path: &Path, session: &Session) -> Result<Option<Client>, Failure> {
    match Client::connect(session, CONNECT) {
        Ok(client) => Ok(Some(client)),
        Err(failure) if failure.code == "api.connect" => {
            let _ = fs::remove_file(path);
            Ok(None)
        }
        Err(failure) => Err(failure),
    }
}

/// Connects to the running game `pick` picks, waiting for its answers as long as it says.
///
/// Fails with `gg.no_game` if none runs that it picks, and `gg.which_game` if more than one does.
pub(crate) fn connect(pick: &Pick) -> Result<Client, Failure> {
    let picked = session_files()?.into_iter().filter(|(_, session)| {
        pick.pid.is_none_or(|pid| session.pid == pid)
            && (pick.app.as_ref()).is_none_or(|app| session.app.eq_ignore_ascii_case(app))
    });
    let mut running = Vec::new();
    for (path, session) in picked {
        running.extend(connect_to(&path, &session)?);
    }
    let mut client = match running.len() {
        0 => {
            let which = match (&pick.pid, &pick.app) {
                (None, None) => "no game is running".to_owned(),
                _ => "no game that `--pid` or `--app` picks is running".to_owned(),
            };
            let message = format!(
                "{which}; `gg run <PACKAGE>` runs one, and `gg sessions` lists those running"
            );
            return Err(Failure::new("gg.no_game", message));
        }
        1 => running.remove(0),
        _ => {
            let games: Vec<_> = (running.iter())
                .map(|client| {
                    let welcome = client.welcome();
                    format!("{} (process {})", welcome.app, welcome.pid)
                })
                .collect();
            let message = format!(
                "{} games are running, {}; `--pid` or `--app` picks one",
                games.len(),
                games.join(", ")
            );
            return Err(Failure::new("gg.which_game", message));
        }
    };
    client.set_timeout(Some(pick.timeout))?;
    Ok(client)
}

/// Sends `method` with `params` to the game `pick` picks, and returns what it answers.
pub(crate) fn call(pick: &Pick, method: &str, params: Value) -> Result<Box<RawValue>, Failure> {
    connect(pick)?.call(method, params)
}

/// The running games, as an array: each one's app, process, address, engine, log, the frames it
/// has run, and what it waits for, if anything.
pub(crate) fn sessions() -> Result<Value, Failure> {
    let mut games = Vec::new();
    for (path, session) in session_files()? {
        let Some(mut client) = connect_to(&path, &session)? else {
            continue;
        };
        let pulse: Pulse = client.call_as("ping", Value::Null)?;
        games.push(json!({
            "app": session.app,
            "pid": session.pid,
            "address": session.address,
            "engine": session.engine,
            "log": session.log,
            "frames": pulse.frames,
            "waiting": pulse.waiting,
        }));
    }
    Ok(Value::Array(games))
}

/// Prints the lines of the log of the game `pick` picks whose events are at `level` or above, as
/// [`rank`] ranks them.
///
/// Fails with `gg.no_log` if the game has no log, or it can't be read.
pub(crate) fn logs(pick: &Pick, level: u8) -> Result<(), Failure> {
    let client = connect(pick)?;
    let Some(log) = client.welcome().log.clone() else {
        return Err(Failure::new("gg.no_log", "the game writes no JSON log"));
    };
    let text = fs::read_to_string(&log).map_err(|error| {
        Failure::new(
            "gg.no_log",
            format!("can't read the log {}: {error}", log.display()),
        )
    })?;
    for line in whole_lines(&text) {
        let event: Option<Map<String, Value>> = serde_json::from_str(line).ok();
        let ranked = event
            .as_ref()
            .and_then(|event| rank(event.get("level")?.as_str()?));
        if ranked.is_some_and(|ranked| ranked >= level) {
            crate::print(line);
        }
    }
    Ok(())
}

/// The lines of `text` that end in a line break: a log's, but for a last line still being written.
fn whole_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive('\n')
        .filter_map(|line| line.strip_suffix('\n'))
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

/// Where a level ranks, from `TRACE`, 1, to `ERROR`, 5, as the log's events name them; `None` for
/// what isn't one.
pub(crate) fn rank(level: &str) -> Option<u8> {
    let rank = match level {
        "TRACE" => 1,
        "DEBUG" => 2,
        "INFO" => 3,
        "WARN" => 4,
        "ERROR" => 5,
        _ => return None,
    };
    Some(rank)
}

/// The events of the JSON log at `log`, each as it was written, but for the lines that aren't one.
pub(crate) fn events(log: &Path) -> Vec<Map<String, Value>> {
    let text = fs::read_to_string(log).unwrap_or_default();
    let events = whole_lines(&text).filter_map(|line| serde_json::from_str(line).ok());
    events.collect()
}

/// `event` as `gg` reports it: its level, code, message and fields, without where and when it was
/// logged.
pub(crate) fn trimmed(mut event: Map<String, Value>) -> Value {
    for place in [
        "timestamp",
        "target",
        "filename",
        "line_number",
        "threadName",
        "threadId",
        "span",
        "spans",
    ] {
        event.remove(place);
    }
    Value::Object(event)
}

/// The log, in `dir`, of the run in process `pid`: the latest, should the OS have given another
/// run that ID before.
pub(crate) fn log_of(dir: &Path, pid: u32) -> Option<PathBuf> {
    let ending = format!("-{pid}.jsonl");
    let logs = fs::read_dir(dir).into_iter().flatten().flatten();
    let mut logs: Vec<PathBuf> = logs
        .map(|entry| entry.path())
        .filter(|path| path.to_str().is_some_and(|path| path.ends_with(&ending)))
        .collect();
    logs.sort();
    logs.pop()
}
