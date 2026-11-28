//! The running games: finding them by session file, calling them, reading their logs.

use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gg_api::{Client, Failure, LogEvents, Pulse, Session, Shown, while_paused};
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::{build, runs};

/// On the loopback only an ended game's refusal takes time.
const CONNECT: Duration = Duration::from_secs(2);

/// How long each following `logs` waits for events, in milliseconds.
const FOLLOWING_MS: u64 = 1_000;

/// Which game to call, how long to wait, the client name (the author of its edits), the
/// capabilities to keep the connection to (`None`: all), and inside a project its manifest's path,
/// whose games alone are reached.
#[derive(Clone)]
pub(crate) struct Pick {
    pub(crate) pid: Option<u32>,
    pub(crate) app: Option<String>,
    pub(crate) timeout: Duration,
    pub(crate) client: String,
    pub(crate) grant: Option<Vec<String>>,
    pub(crate) project: Option<PathBuf>,
}

fn session_files(project: Option<&Path>) -> Result<Vec<(PathBuf, Session)>, Failure> {
    let sessions = sessions_in(&build::target_dir(project.and_then(Path::parent))?);
    Ok(match project {
        Some(manifest) => scoped(sessions, manifest),
        None => sessions,
    })
}

/// The sessions naming the project manifest at `manifest`, by canonical path: a game may name it
/// in another form, as Windows' `\\?\`, or through a link. One naming no project is dropped.
fn scoped(sessions: Vec<(PathBuf, Session)>, manifest: &Path) -> Vec<(PathBuf, Session)> {
    let Ok(manifest) = fs::canonicalize(manifest) else {
        return Vec::new();
    };
    let ours = |session: &Session| {
        let named = session.project.as_deref().map(fs::canonicalize);
        named.is_some_and(|named| named.is_ok_and(|named| named == manifest))
    };
    sessions
        .into_iter()
        .filter(|(_, session)| ours(session))
        .collect()
}

/// The session files of each profile of `target`: cargo's, as `target/debug`, dx's, as
/// `target/x86_64-pc-windows-msvc/desktop-dev`.
fn sessions_in(target: &Path) -> Vec<(PathBuf, Session)> {
    let dirs = |dir: &Path| fs::read_dir(dir).into_iter().flatten().flatten();
    let profiles = dirs(target).flat_map(|dir| {
        let dir = dir.path();
        dirs(&dir).map(|profile| profile.path()).chain([dir])
    });
    let sessions = profiles.flat_map(|profile| Session::list(&Session::dir(&profile)));
    sessions.collect()
}

/// Connects to `session`'s game; removes its file at `path` if the game ended (a crash or kill
/// leaves it).
fn connect_to(
    path: &Path,
    session: &Session,
    client: &str,
    grant: Option<&[String]>,
) -> Result<Option<Client>, Failure> {
    match Client::connect_granted(session, client, grant.map(<[String]>::to_vec), CONNECT) {
        Ok(client) => Ok(Some(client)),
        Err(failure) if failure.code == "api.connect" => {
            let _ = fs::remove_file(path);
            Ok(None)
        }
        Err(failure) => Err(failure),
    }
}

/// Connects to each of `sessions` ([`connect_to`]), warning of other API revisions
/// ([`warn_protocol`]). A game failing otherwise than by ending is left out if another answers,
/// its file kept: a kill's file may name a port another process now has, which mustn't fail every
/// command.
///
/// Fails as the first that failed, if none answered.
fn answering(
    sessions: impl IntoIterator<Item = (PathBuf, Session)>,
    client: &str,
    grant: Option<&[String]>,
) -> Result<Vec<(Session, Client)>, Failure> {
    let (mut answering, mut failed) = (Vec::new(), None);
    for (path, session) in sessions {
        match connect_to(&path, &session, client, grant) {
            Ok(Some(client)) => {
                warn_protocol(&path, &session);
                answering.push((session, client));
            }
            Ok(None) => {}
            Err(failure) => {
                failed.get_or_insert(failure);
            }
        }
    }
    match failed {
        Some(failure) if answering.is_empty() => Err(failure),
        _ => Ok(answering),
    }
}

/// Warns `gg.protocol` if the game's tool API revision isn't [`gg_api::PROTOCOL`].
pub(crate) fn warn_protocol(path: &Path, session: &Session) {
    if let Some(message) = protocol_drift(session) {
        crate::warn("gg.protocol", &message, path);
    }
}

fn protocol_drift(session: &Session) -> Option<String> {
    let theirs = match session.protocol {
        Some(theirs) if theirs == gg_api::PROTOCOL => return None,
        Some(theirs) => format!("speaks revision {theirs} of the tool API"),
        None => "names no revision of the tool API, its engine older than them".to_owned(),
    };
    Some(format!(
        "{} (process {}) {theirs}, where gg speaks revision {}: gg and the game were built against \
         different revisions of the engine, so gg may misread the game, or the game refuse gg; \
         build the game again against gg's engine, or run the gg of the engine the game builds \
         against, installed with `cargo install --path <engine>/tools/gg`, or `--git <url> --rev \
         <revision> gg`",
        session.app,
        session.pid,
        gg_api::PROTOCOL
    ))
}

/// Connects to the running game `pick` picks.
///
/// Fails with `gg.no_game` if none runs, unless a `gg run` says why ([`runs::down`]:
/// `gg.rebuilding`, `gg.build_failed`, `gg.waiting`, `gg.starting`);
/// `gg.which_game` if several do; and as [`answering`] does.
pub(crate) fn connect(pick: &Pick) -> Result<Client, Failure> {
    let picked = session_files(pick.project.as_deref())?
        .into_iter()
        .filter(|(_, session)| {
            pick.pid.is_none_or(|pid| session.pid == pid)
                && (pick.app.as_ref()).is_none_or(|app| session.app.eq_ignore_ascii_case(app))
        });
    let mut running = answering(picked, &pick.client, pick.grant.as_deref())?;
    let sessions: Vec<&Session> = running.iter().map(|(session, _)| session).collect();
    if let Some(host) = host_of(&sessions) {
        running = vec![running.swap_remove(host)];
    }
    let mut running: Vec<Client> = running.into_iter().map(|(_, client)| client).collect();
    let mut client = match running.len() {
        0 => {
            if let Some(down) = runs::down(pick) {
                return Err(down);
            }
            let of = (pick.project.as_ref())
                .map(|project| format!(" of the project {}", project.display()))
                .unwrap_or_default();
            let picks = pick.pid.is_some() || pick.app.is_some();
            let that = if picks {
                " that `--pid` or `--app` chooses"
            } else {
                ""
            };
            let run = match pick.project {
                Some(_) => "`gg run` runs its game",
                None => "`gg run <PACKAGE>` runs one",
            };
            let message = format!(
                "no game{of}{that} is running; {run}, and `gg sessions` lists those running"
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
                "{} games are running, {}; `--pid` or `--app` chooses one",
                games.len(),
                games.join(", ")
            );
            return Err(Failure::new("gg.which_game", message));
        }
    };
    client.set_timeout(Some(pick.timeout))?;
    Ok(client)
}

/// The host of two peers of one lockstep session (host and joiner of one app): both apply its
/// edits and patches. `None` for others.
fn host_of(sessions: &[&Session]) -> Option<usize> {
    let [a, b] = sessions else {
        return None;
    };
    let players = (a.player, b.player);
    let peers = a.app == b.app && matches!(players, (Some(0), Some(1)) | (Some(1), Some(0)));
    peers.then_some(if a.player == Some(0) { 0 } else { 1 })
}

/// Sends `method` with `params` to the game `pick` picks. A `quit` that no game answers ends the
/// `gg run` waiting without one ([`runs::quit`]).
///
/// Fails as the call does; a late answer says what the game waits for ([`waits`]).
pub(crate) fn call(pick: &Pick, method: &str, params: Value) -> Result<Box<RawValue>, Failure> {
    let mut client = match connect(pick) {
        Ok(client) => client,
        Err(down)
            if method == "quit" && matches!(&*down.code, "gg.build_failed" | "gg.waiting") =>
        {
            let ended = runs::quit(pick, down)?;
            return serde_json::value::to_raw_value(&ended)
                .map_err(|error| Failure::new("gg.run", error.to_string()));
        }
        Err(failure) => return Err(failure),
    };
    let (pid, keys) = (client.welcome().pid, client.welcome().keys);
    // A game of an older engine ignores keys.
    if !gg_api::changes(method) || !keys {
        let answered = client.call(method, params);
        return answered.map_err(|failure| waits(pick, method, failure));
    }
    // Keyed, so `gg retry` can send it again if no answer comes.
    let key = new_key();
    let answered = client.call_keyed(method, params.clone(), Some(&key));
    let Ok(path) = Unanswered::path(pick, pid) else {
        return answered.map_err(|failure| waits(pick, method, failure));
    };
    match answered {
        Err(failure) if unanswered(&failure) => {
            let failure = waits(pick, method, failure);
            let kept = Unanswered {
                method: method.to_owned(),
                params,
                key,
            };
            Err(match kept.keep(&path) {
                Ok(()) => Failure::new(
                    failure.code,
                    format!("{}; `gg retry` does so", failure.message),
                ),
                Err(_) => failure,
            })
        }
        answered => answered.map_err(|failure| waits(pick, method, failure)),
    }
}

/// Whether `failure` leaves a call unanswered, which the game may have taken.
fn unanswered(failure: &Failure) -> bool {
    matches!(failure.code.as_str(), "api.timeout" | "api.closed")
}

/// A key no other call has: 64 bits from `RandomState`, as hex.
fn new_key() -> String {
    format!("{:016x}", RandomState::new().hash_one(std::process::id()))
}

/// The last call that changed a game and went unanswered, which `gg retry` sends again: the
/// method, its parameters and its key.
struct Unanswered {
    method: String,
    params: Value,
    key: String,
}

impl Unanswered {
    /// `gg-retry-<pid>.json` in `pick`'s target directory, for the game in process `pid`.
    fn path(pick: &Pick, pid: u32) -> Result<PathBuf, Failure> {
        let target = build::target_dir(pick.project.as_deref().and_then(Path::parent))?;
        Ok(target.join(format!("gg-retry-{pid}.json")))
    }

    /// Keeps it at `path`, removing beside it those of games that ended.
    fn keep(&self, path: &Path) -> Result<(), Failure> {
        let kept = json!({"method": self.method, "params": self.params, "key": self.key});
        let written = fs::write(path, kept.to_string());
        written.map_err(|error| Failure::new("gg.retry", error.to_string()))?;
        if let Some(target) = path.parent() {
            Self::prune(target, path);
        }
        Ok(())
    }

    /// Removes from `target` the calls kept for games with no session file there, which ended, but
    /// the one at `kept`: no retry reaches them. Session files count by name, `<app>-<pid>.json`,
    /// read or not, as another version of gg may not read them.
    fn prune(target: &Path, kept: &Path) {
        let names = |dir: &Path| fs::read_dir(dir).into_iter().flatten().flatten();
        let profiles = names(target).flat_map(|dir| {
            let dir = dir.path();
            names(&dir).map(|profile| profile.path()).chain([dir])
        });
        let files = profiles.flat_map(|profile| names(&Session::dir(&profile)));
        let pid = |name: &str| {
            let (_, pid) = name.strip_suffix(".json")?.rsplit_once('-')?;
            pid.parse::<u32>().ok()
        };
        let live: Vec<u32> = files
            .filter_map(|file| pid(file.file_name().to_str()?))
            .collect();
        for entry in fs::read_dir(target).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let name = name
                .to_str()
                .and_then(|name| name.strip_prefix("gg-retry-"));
            let pid = name.and_then(|name| name.strip_suffix(".json"));
            let ended = pid
                .and_then(|pid| pid.parse().ok())
                .is_some_and(|pid| !live.contains(&pid));
            if ended && entry.path() != kept {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    /// The call kept at `path`, if one is, whole.
    fn read(path: &Path) -> Option<Self> {
        let kept: Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
        Some(Self {
            method: kept["method"].as_str()?.to_owned(),
            params: kept["params"].clone(),
            key: kept["key"].as_str()?.to_owned(),
        })
    }
}

/// Sends the picked game's last call that changed it and went unanswered again, with its key, so
/// the game answers as it did rather than doing it again ([`gg_api::Request::key`]). Forgets it
/// once answered. A game that ended takes it along.
///
/// Fails with:
/// - `gg.no_retry` if none of the game's is kept;
/// - `gg.no_keys` for a game of an older engine, which ignores keys;
/// - `api.forgotten` if the game no longer keeps its answer, having answered 64 keyed calls
///   since: it did nothing;
/// - as the call does, `api.timeout` keeping it for another retry.
pub(crate) fn retry(pick: &Pick) -> Result<Box<RawValue>, Failure> {
    let mut client = connect(pick)?;
    let (pid, app) = (client.welcome().pid, client.welcome().app.clone());
    if !client.welcome().keys {
        let message = format!(
            "{app}, process {pid}, is of an engine that ignores keys, so a retry may do the call \
             again: read what it changes before sending it again"
        );
        return Err(Failure::new("gg.no_keys", message));
    }
    let path = Unanswered::path(pick, pid)?;
    if let Some(target) = path.parent() {
        Unanswered::prune(target, &path);
    }
    let Some(kept) = Unanswered::read(&path) else {
        let message = format!("no call that changes {app}, process {pid}, went unanswered");
        return Err(Failure::new("gg.no_retry", message));
    };
    let answered = client.call_again(&kept.method, kept.params, &kept.key);
    if !answered.as_ref().is_err_and(unanswered) {
        let _ = fs::remove_file(&path);
    }
    answered.map_err(|failure| waits(pick, &kept.method, failure))
}

/// The API's `Shown` for `given`: a unit variant's name, as `roughness`, stays a string; any other
/// string is an image's debug name, as `view depth`, and becomes `{"image": NAME}`; an object of
/// an `image` alone is kept.
///
/// Fails with `gg.args` for an empty name, one over [`gg_api::MOST_IMAGE_NAME`] bytes, or
/// anything else.
pub(crate) fn shown(given: &Value) -> Result<Value, Failure> {
    let args = |message: String| Failure::new("gg.args", message);
    let name = match (given, serde_json::from_value::<Shown>(given.clone())) {
        (Value::String(_), Ok(_)) => return Ok(given.clone()),
        (Value::String(name), Err(_)) => name.clone(),
        (Value::Object(_), Ok(Shown::Image(name))) => name,
        _ => {
            return Err(args(format!(
                "`show` is a name, as `roughness` or `view depth`, or `{{\"image\": NAME}}`, not \
                 {given}"
            )));
        }
    };
    if name.is_empty() || name.len() > gg_api::MOST_IMAGE_NAME {
        return Err(args(format!(
            "an image's name is 1 to {} bytes, not `{name}`",
            gg_api::MOST_IMAGE_NAME
        )));
    }
    Ok(json!({"image": name}))
}

/// Captures with `params` once per show of `shows` ([`shown`]'s). Several are all of one world:
/// one connection, the game paused if it plays, resumed after. Each answer is a `Captured`.
///
/// Fails as the call does; the first capture that fails, or else `resume`; none follows it.
pub(crate) fn captures(
    pick: &Pick,
    params: Value,
    shows: &[Value],
) -> Result<Vec<Box<RawValue>>, Failure> {
    match shows {
        [] => call(pick, "capture", params).map(|taken| vec![taken]),
        [show] => {
            let params = with_show(&params, show);
            call(pick, "capture", params).map(|taken| vec![taken])
        }
        _ => {
            let mut client = connect(pick)?;
            let call = |method: &str, params: Value| {
                (client.call(method, params)).map_err(|failure| waits(pick, method, failure))
            };
            captured_together(call, &params, shows)
        }
    }
}

fn with_show(params: &Value, show: &Value) -> Value {
    let mut params = params.clone();
    params["show"] = show.clone();
    params
}

fn captured_together(
    call: impl FnMut(&str, Value) -> Result<Box<RawValue>, Failure>,
    params: &Value,
    shows: &[Value],
) -> Result<Vec<Box<RawValue>>, Failure> {
    while_paused(call, |call| {
        (shows.iter())
            .map(|show| call("capture", with_show(params, show)))
            .collect()
    })
}

/// `answers` as one JSON array, each as written.
pub(crate) fn joined(answers: &[Box<RawValue>]) -> String {
    let answers: Vec<&str> = answers.iter().map(|answer| answer.get()).collect();
    format!("[{}]", answers.join(","))
}

/// `failure`, `method`'s, with what the game waits for (its `ping` says, on a new connection) if
/// it's `api.timeout` and a stopped script, file dialog, window menu or load holds the main
/// thread, not a hang; else as it is.
pub(crate) fn waits(pick: &Pick, method: &str, failure: Failure) -> Failure {
    if failure.code != "api.timeout" {
        return failure;
    }
    let pulse = connect(pick).and_then(|mut client| client.call_as::<Pulse>("ping", Value::Null));
    let why = match pulse.ok().and_then(|pulse| pulse.waiting).as_deref() {
        Some("breakpoint") => {
            "a script has stopped in it, and it waits there until `gg debug continue`, or a step, \
             has it go on; `gg debug` shows where"
        }
        Some("dialog") => "it waits for a file dialog to close",
        Some("menu") => "it waits for its window menu to close",
        Some("loading") => {
            "it's loading a scene, a model or other assets, which a development build may take \
             seconds over; `gg wait` waits for it"
        }
        _ => return failure,
    };
    let message = format!(
        "the game didn't answer `{method}` in time: {why}{}",
        gg_api::unanswered(method)
    );
    Failure::new(failure.code, message)
}

/// The running games, of the project at `project` alone: session fields, frames run, and what each
/// waits for.
///
/// Fails as [`answering`] does, and as a game's ping fails.
pub(crate) fn sessions(project: Option<&Path>) -> Result<Value, Failure> {
    let mut games = Vec::new();
    for (session, mut client) in answering(session_files(project)?, "gg", None)? {
        let pulse: Pulse = client.call_as("ping", Value::Null)?;
        games.push(json!({
            "app": session.app,
            "pid": session.pid,
            "address": session.address,
            "engine": session.engine,
            "player": session.player,
            "project": session.project,
            "log": session.log,
            "frames": pulse.frames,
            "waiting": pulse.waiting,
        }));
    }
    Ok(Value::Array(games))
}

/// Prints the game's events at `level` and above, as JSON log lines, oldest first up to the last
/// logged at the first answer; with `follow`, then each as logged, until the game or stdout ends,
/// as with `head`.
///
/// Fails as [`connect`] and the calls do, but not for the game ending while it follows.
pub(crate) fn logs(pick: &Pick, level: &str, follow: bool) -> Result<(), Failure> {
    let mut client = connect(pick)?;
    let mut read: LogEvents = client.call_as("logs", json!({"level": level}))?;
    let end = read.end;
    let mut caught_up = false;
    loop {
        if !printed(&read) {
            return Ok(());
        }
        if read.next >= end && !caught_up {
            if !follow {
                return Ok(());
            }
            caught_up = true;
            client.set_timeout(Some(Duration::from_millis(FOLLOWING_MS) + pick.timeout))?;
        }
        let wait_ms = if caught_up { FOLLOWING_MS } else { 0 };
        let params =
            json!({"from": read.next, "run": read.run, "level": level, "wait_ms": wait_ms});
        read = match client.call_as("logs", params) {
            Ok(read) => read,
            Err(failure)
                if follow && matches!(failure.code.as_str(), "api.closed" | "api.connect") =>
            {
                return Ok(());
            }
            Err(failure) => return Err(failure),
        };
    }
}

/// Prints `read`'s events; whether stdout took them, as it doesn't after a `head` ends.
fn printed(read: &LogEvents) -> bool {
    let mut out = std::io::stdout().lock();
    (read.events.iter()).all(|event| writeln!(out, "{}", event.get()).is_ok())
}

/// All lines but a last one still being written.
fn whole_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive('\n')
        .filter_map(|line| line.strip_suffix('\n'))
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

/// The events of the JSON log at `log`.
pub(crate) fn events(log: &Path) -> Vec<Map<String, Value>> {
    let text = fs::read_to_string(log).unwrap_or_default();
    let events = whole_lines(&text).filter_map(|line| serde_json::from_str(line).ok());
    events.collect()
}

/// `event` without where and when it was logged.
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

/// The latest log in `dir` of process `pid`, as the OS may reuse an ID.
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

#[cfg(test)]
mod tests {
    use std::io::BufReader;
    use std::net::{SocketAddr, TcpListener};
    use std::thread;

    use gg_api::Request;

    use super::*;

    /// An unanswered call kept for `gg retry` reads back whole, apart for each game; a file that
    /// isn't one reads as none.
    #[test]
    fn an_unanswered_call_is_kept_whole() {
        let dir = std::env::temp_dir().join(format!("gg-retry-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("a directory");
        let path = dir.join("gg-retry-42.json");
        let kept = Unanswered {
            method: "step".to_owned(),
            params: json!({"ticks": 3}),
            key: new_key(),
        };
        kept.keep(&path).expect("kept");
        let read = Unanswered::read(&path).expect("kept");
        assert_eq!(read.method, "step");
        assert_eq!((read.params, read.key), (kept.params, kept.key));
        assert_ne!(new_key(), new_key());
        fs::write(&path, "{\"method\": \"step\"}").expect("written");
        assert!(Unanswered::read(&path).is_none());
        let pick = Pick {
            pid: None,
            app: None,
            timeout: Duration::from_secs(1),
            client: "test".to_owned(),
            grant: None,
            project: None,
        };
        let [one, other] = [1, 2].map(|pid| Unanswered::path(&pick, pid).expect("a path"));
        assert_ne!(one, other);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Keeping a call removes those kept for games that ended, whose session files are gone, but
    /// not a running game's.
    #[test]
    fn keeping_a_call_removes_those_of_games_that_ended() {
        let target = std::env::temp_dir().join(format!("gg-retry-pruned-{}", std::process::id()));
        let sessions = target.join("debug").join("sessions");
        write_session(&sessions, 7, None);
        // A session file this gg can't read, as another version's, counts too.
        fs::write(sessions.join("sandbox-6.json"), "{}").expect("written");
        let kept = |pid: u32| target.join(format!("gg-retry-{pid}.json"));
        for pid in [6, 7, 8] {
            fs::write(kept(pid), "{}").expect("written");
        }
        let call = Unanswered {
            method: "step".to_owned(),
            params: Value::Null,
            key: new_key(),
        };
        call.keep(&kept(9)).expect("kept");
        let left = [6, 7, 8, 9].map(|pid| kept(pid).exists());
        let _ = fs::remove_dir_all(&target);
        assert_eq!(left, [true, true, false, true]);
    }

    fn write_session(dir: &Path, pid: u32, project: Option<PathBuf>) {
        fs::create_dir_all(dir).expect("a directory");
        let session = Session {
            app: "Sandbox".to_owned(),
            pid,
            address: "127.0.0.1:4000".parse().expect("an address"),
            token: "t".to_owned(),
            log: None,
            engine: "0.1.0 debug".to_owned(),
            aslr_reference: 0,
            player: None,
            project,
            protocol: Some(gg_api::PROTOCOL),
            hot: false,
        };
        let text = serde_json::to_string(&session).expect("JSON");
        fs::write(dir.join(Session::file_name("Sandbox", pid)), text).expect("written");
    }

    fn pids(sessions: Vec<(PathBuf, Session)>) -> Vec<u32> {
        let mut pids: Vec<u32> = sessions
            .into_iter()
            .map(|(_, session)| session.pid)
            .collect();
        pids.sort();
        pids
    }

    /// Games cargo built, and one dx built for its target, are all found.
    #[test]
    fn sessions_are_found_in_cargos_profiles_and_in_those_of_dxs_target() {
        let target = std::env::temp_dir().join(format!("gg-sessions-{}", std::process::id()));
        let _ = fs::remove_dir_all(&target);
        let profiles = ["debug", "release", "x86_64-pc-windows-msvc/desktop-dev"];
        for (pid, profile) in (1..).zip(profiles) {
            write_session(&Session::dir(&target.join(profile)), pid, None);
        }
        let pids = pids(sessions_in(&target));
        let _ = fs::remove_dir_all(&target);
        assert_eq!(pids, [1, 2, 3]);
    }

    /// Inside a project, only its games' sessions are found, by its manifest's path in any form.
    #[test]
    fn sessions_are_scoped_to_their_project() {
        let root = std::env::temp_dir().join(format!("gg-scoped-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let manifests = ["a", "b"].map(|game| root.join(game).join("project.ron"));
        for manifest in &manifests {
            fs::create_dir_all(manifest.parent().expect("a directory")).expect("a directory");
            fs::write(manifest, "").expect("written");
        }
        let dir = Session::dir(&root.join("target").join("debug"));
        let roundabout = root.join("b").join("..").join("a").join("project.ron");
        write_session(&dir, 1, Some(roundabout));
        write_session(&dir, 2, Some(manifests[1].clone()));
        write_session(&dir, 3, None);
        let found = |manifest: &Path| pids(scoped(sessions_in(&root.join("target")), manifest));
        let (a, b, none) = (
            found(&manifests[0]),
            found(&manifests[1]),
            found(&root.join("c")),
        );
        let _ = fs::remove_dir_all(&root);
        assert_eq!((a, b), (vec![1], vec![2]));
        assert!(none.is_empty());
    }

    /// Two peers of one lockstep session go through the host; other games are a choice.
    #[test]
    fn a_lockstep_sessions_peers_alone_are_reached_through_the_host() {
        let session = |pid: u32, app: &str, player: Option<u8>| Session {
            app: app.to_owned(),
            pid,
            address: "127.0.0.1:4000".parse().expect("an address"),
            token: "t".to_owned(),
            log: None,
            engine: "0.1.0 debug".to_owned(),
            aslr_reference: 0,
            player,
            project: None,
            protocol: Some(gg_api::PROTOCOL),
            hot: false,
        };
        let host = session(1, "Shards", Some(0));
        let joiner = session(2, "Shards", Some(1));
        let other = session(3, "Foxball", Some(1));
        let alone = session(4, "Shards", None);
        let second = session(5, "Shards", Some(0));
        assert_eq!(host_of(&[&host, &joiner]), Some(0));
        assert_eq!(host_of(&[&joiner, &host]), Some(1));
        for sessions in [
            &[&host][..],
            &[&host, &other],
            &[&host, &alone],
            &[&host, &second],
            &[&host, &joiner, &other],
        ] {
            let pids: Vec<u32> = sessions.iter().map(|session| session.pid).collect();
            assert_eq!(host_of(sessions), None, "{pids:?}");
        }
    }

    /// A game of another tool API revision, or none (an older engine), is told, naming both.
    #[test]
    fn a_game_of_another_revision_of_the_tool_api_is_told() {
        let session = |protocol| Session {
            app: "Shards".to_owned(),
            pid: 7,
            address: "127.0.0.1:4000".parse().expect("an address"),
            token: "t".to_owned(),
            log: None,
            engine: "0.1.0 debug".to_owned(),
            aslr_reference: 0,
            player: None,
            project: None,
            protocol,
            hot: false,
        };
        let ours = gg_api::PROTOCOL;
        assert_eq!(protocol_drift(&session(Some(ours))), None);
        let other = protocol_drift(&session(Some(ours + 1))).expect("told");
        let speaks = format!("Shards (process 7) speaks revision {}", ours + 1);
        assert!(other.starts_with(&speaks), "{other}");
        assert!(
            other.contains(&format!("where gg speaks revision {ours}:")),
            "{other}"
        );
        let older = protocol_drift(&session(None)).expect("told");
        assert!(
            older.starts_with("Shards (process 7) names no revision"),
            "{older}"
        );
    }

    /// Serves `hello` on the loopback, refusing a token but `token` with `api.unauthorized`.
    fn serving(token: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = listener.local_addr().expect("its address");
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                thread::spawn(move || {
                    let mut writer = stream.try_clone().expect("a writer");
                    let mut reader = BufReader::new(stream);
                    while let Ok(Some(line)) = gg_api::read_line(&mut reader) {
                        let request: Request = serde_json::from_str(&line).expect("a request");
                        let welcome = r#"{"app":"Sandbox","pid":1,"engine":"0.1.0","log":null}"#;
                        let refused = Failure::new("api.unauthorized", "another game's token");
                        let answer = if request.params["token"] == token {
                            Ok(welcome)
                        } else {
                            Err(&refused)
                        };
                        let line = gg_api::answer_line(request.id, answer);
                        if writer.write_all(line.as_bytes()).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        address
    }

    /// A game failing otherwise than by ending, as a kill's file naming another game's port, is
    /// left out while another answers, its file kept; alone, its failure is the command's. An
    /// ended game goes, with its file.
    #[test]
    fn a_session_that_fails_fails_a_command_only_if_no_other_answers() {
        let dir = std::env::temp_dir().join(format!("gg-answering-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a directory");
        let session = |pid: u32, address: SocketAddr, token: &str| {
            let session = Session {
                app: "Sandbox".to_owned(),
                pid,
                address,
                token: token.to_owned(),
                log: None,
                engine: "0.1.0 debug".to_owned(),
                aslr_reference: 0,
                player: None,
                project: None,
                protocol: Some(gg_api::PROTOCOL),
                hot: false,
            };
            let path = dir.join(Session::file_name("Sandbox", pid));
            fs::write(&path, serde_json::to_string(&session).expect("JSON")).expect("written");
            (path, session)
        };
        let address = serving("t");
        let (live, stale) = (
            session(1, address, "t"),
            session(2, address, "a stale token"),
        );
        let ended = session(3, "127.0.0.1:1".parse().expect("an address"), "t");
        let pids = |answered: Vec<(Session, Client)>| -> Vec<u32> {
            answered.iter().map(|(session, _)| session.pid).collect()
        };
        let all = [stale.clone(), live.clone(), ended.clone()];
        let answered = answering(all, "test", None).expect("one answers");
        let left = [&stale, &live, &ended].map(|(path, _)| path.exists());
        let refused = answering([stale.clone()], "test", None).err();
        let none = answering(Vec::new(), "test", None).expect("none to fail");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(pids(answered), [1]);
        assert_eq!(left, [true, true, false]);
        let refused = refused.map(|failure| failure.code);
        assert_eq!(refused.as_deref(), Some("api.unauthorized"));
        assert!(none.is_empty());
    }

    /// A variant's name stays a string, any other is an image's; an empty or long name, or a
    /// non-name, is refused.
    #[test]
    fn a_show_is_a_variant_or_an_images_name() {
        for (given, want) in [
            (json!("roughness"), json!("roughness")),
            (json!("surface_normal"), json!("surface_normal")),
            (json!("view depth"), json!({"image": "view depth"})),
            // The newtype variant's own name is a name, not a variant.
            (json!("image"), json!({"image": "image"})),
            (
                json!({"image": "view depth"}),
                json!({"image": "view depth"}),
            ),
        ] {
            assert_eq!(shown(&given).expect("a show"), want, "{given}");
        }
        let long = "x".repeat(gg_api::MOST_IMAGE_NAME + 1);
        for given in [
            json!(""),
            json!(long),
            json!({"image": ""}),
            json!({"image": 3}),
            json!({"other": "x"}),
            json!(3),
            json!(null),
            json!(["roughness"]),
        ] {
            let refused = shown(&given).err().map(|failure| failure.code);
            assert_eq!(refused.as_deref(), Some("gg.args"), "{given}");
        }
    }

    /// The requests a game `paused` or not gets from [`captured_together`], and the result; it
    /// fails its capture of `fail`.
    fn together(
        paused: bool,
        fail: Option<&str>,
    ) -> (Vec<String>, Result<Vec<Box<RawValue>>, Failure>) {
        let mut got = Vec::new();
        let result = captured_together(
            |method, params| {
                got.push(match params.get("show") {
                    Some(show) => format!("{method} {show}"),
                    None => method.to_owned(),
                });
                if method == "capture" && fail.is_some_and(|show| params["show"] == show) {
                    return Err(Failure::new("renderer.no_image", "no such image"));
                }
                let answer = match method {
                    "status" => json!({
                        "ticks": 0, "paused": paused, "stopped": false, "frames": 0,
                        "entities": 0, "view": null, "scene": null, "unsaved": false,
                    }),
                    _ => json!({"path": "a.png"}),
                };
                Ok(RawValue::from_string(answer.to_string()).expect("JSON"))
            },
            &json!({"own": true}),
            &[json!("normal"), json!("roughness")],
        );
        (got, result)
    }

    /// Several captures of a playing game pause it and resume it after, or after one fails; of a
    /// paused game, they leave it paused.
    #[test]
    fn several_captures_are_taken_paused_and_the_game_resumes_if_it_played() {
        let (got, result) = together(false, None);
        let want = [
            "status",
            "pause",
            r#"capture "normal""#,
            r#"capture "roughness""#,
            "resume",
        ];
        assert_eq!(got, want);
        assert_eq!(result.expect("taken").len(), 2);
        let (got, result) = together(true, None);
        assert_eq!(
            got,
            ["status", r#"capture "normal""#, r#"capture "roughness""#]
        );
        assert_eq!(result.expect("taken").len(), 2);
        let (got, result) = together(false, Some("normal"));
        assert_eq!(got, ["status", "pause", r#"capture "normal""#, "resume"]);
        assert_eq!(
            result.err().map(|f| f.code).as_deref(),
            Some("renderer.no_image")
        );
        let (got, _) = together(true, Some("normal"));
        assert_eq!(got, ["status", r#"capture "normal""#]);
    }
}
