//! The running games: finding them by their session files, talking to them, and reading their
//! logs.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gg_api::{Client, Failure, LogEvents, Pulse, Session};
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::build;

/// How long a connection to a game may take: it's on the loopback, where only one that has ended
/// takes any time, refusing it.
const CONNECT: Duration = Duration::from_secs(2);

/// How long each `logs` that follows a game's events waits for them to come, in milliseconds.
const FOLLOWING_MS: u64 = 1_000;

/// Which running game to talk to, how long to wait for its answers, the client to say hello as,
/// the author of the edits it applies, and the project whose games alone it reaches, by its
/// manifest's path, inside one.
#[derive(Clone)]
pub(crate) struct Pick {
    pub(crate) pid: Option<u32>,
    pub(crate) app: Option<String>,
    pub(crate) timeout: Duration,
    pub(crate) client: String,
    pub(crate) project: Option<PathBuf>,
}

/// The session files of the games built in the workspace, as [`sessions_in`] its target directory
/// finds them; with `project`, a manifest's path, only those of the project's games, as [`scoped`]
/// says, in the target directory cargo names from the project's directory.
fn session_files(project: Option<&Path>) -> Result<Vec<(PathBuf, Session)>, Failure> {
    let sessions = sessions_in(&build::target_dir(project.and_then(Path::parent))?);
    Ok(match project {
        Some(manifest) => scoped(sessions, manifest),
        None => sessions,
    })
}

/// The sessions of `sessions` whose games name the project manifest at `manifest`, comparing the
/// paths canonical, as one may be named in another form, such as Windows' `\\?\` one, or through a
/// link: none that names no project, as a game's whose engine is older than projects, or one that
/// isn't there.
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

/// The session files in the `sessions` directory of each profile of target directory `target`:
/// cargo's, as `target/debug`, and those of a target's directory in it, where dx builds, as
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

/// Connects to the game that `session`, read from the file at `path`, names, as `client`, removing
/// the file if the game has ended, which a crash or a kill leaves it behind for.
fn connect_to(path: &Path, session: &Session, client: &str) -> Result<Option<Client>, Failure> {
    match Client::connect(session, client, CONNECT) {
        Ok(client) => Ok(Some(client)),
        Err(failure) if failure.code == "api.connect" => {
            let _ = fs::remove_file(path);
            Ok(None)
        }
        Err(failure) => Err(failure),
    }
}

/// Connects to each game of `sessions`, each read from its file, as `client`, as [`connect_to`]
/// does, and returns those that answered, each with its session. One that fails otherwise than by
/// having ended is left out if another answers, as a file that a kill left naming a port that
/// another game, or another process, has since been given fails, which then mustn't fail every
/// command; its file stays, as its game may yet answer.
///
/// Fails as the first that failed so did, if none answered.
fn answering(
    sessions: impl IntoIterator<Item = (PathBuf, Session)>,
    client: &str,
) -> Result<Vec<(Session, Client)>, Failure> {
    let (mut answering, mut failed) = (Vec::new(), None);
    for (path, session) in sessions {
        match connect_to(&path, &session, client) {
            Ok(Some(client)) => answering.push((session, client)),
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

/// Connects to the running game `pick` picks, waiting for its answers as long as it says.
///
/// Fails with `gg.no_game` if none runs that it picks, and `gg.which_game` if more than one does;
/// and as [`answering`] does.
pub(crate) fn connect(pick: &Pick) -> Result<Client, Failure> {
    let picked = session_files(pick.project.as_deref())?
        .into_iter()
        .filter(|(_, session)| {
            pick.pid.is_none_or(|pid| session.pid == pid)
                && (pick.app.as_ref()).is_none_or(|app| session.app.eq_ignore_ascii_case(app))
        });
    let answered = answering(picked, &pick.client)?.into_iter();
    let mut running: Vec<_> = answered
        .map(|(session, client)| (session.player, client))
        .collect();
    // A lockstep session's peers, of one game, are reached through the host, whose edits and
    // patches both peers apply.
    let peers = running.iter().all(|(player, _)| player.is_some());
    let hosts = running
        .iter()
        .filter(|(player, _)| *player == Some(0))
        .count();
    if running.len() > 1 && peers && hosts == 1 {
        running.retain(|(player, _)| *player == Some(0));
    }
    let mut running: Vec<Client> = running.into_iter().map(|(_, client)| client).collect();
    let mut client = match running.len() {
        0 => {
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

/// Sends `method` with `params` to the game `pick` picks, and returns what it answers.
pub(crate) fn call(pick: &Pick, method: &str, params: Value) -> Result<Box<RawValue>, Failure> {
    connect(pick)?.call(method, params)
}

/// The running games, those of the project whose manifest is at `project` alone, as an array: each
/// one's app, process, address, engine, player, project, log, the frames it has run, and what it
/// waits for, if anything.
///
/// Fails as [`answering`] does, and as a game's ping fails.
pub(crate) fn sessions(project: Option<&Path>) -> Result<Value, Failure> {
    let mut games = Vec::new();
    for (session, mut client) in answering(session_files(project)?, "gg")? {
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

/// Prints the events the game `pick` picks keeps at `level` and more severe, as the API's `logs`
/// reads them, each its JSON log's line, from the oldest to the last logged as the first answer
/// came; and with `follow`, then each as it's logged, until the game ends, or stdout does, as when
/// piped into `head`.
///
/// Fails as [`connect`] does, and as the calls fail, but for the game's ending while it follows.
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

/// Prints the events `read` read, each its line; returns whether stdout took them, as it doesn't
/// once it's piped into a `head` that has ended.
fn printed(read: &LogEvents) -> bool {
    let mut out = std::io::stdout().lock();
    (read.events.iter()).all(|event| writeln!(out, "{}", event.get()).is_ok())
}

/// The lines of `text` that end in a line break: a log's, but for a last line still being written.
fn whole_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive('\n')
        .filter_map(|line| line.strip_suffix('\n'))
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
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

#[cfg(test)]
mod tests {
    use std::io::BufReader;
    use std::net::{SocketAddr, TcpListener};
    use std::thread;

    use gg_api::Request;

    use super::*;

    /// Writes the session file of a game in process `pid`, of the project whose manifest is at
    /// `project`, if any, in `dir`.
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
        };
        let text = serde_json::to_string(&session).expect("JSON");
        fs::write(dir.join(Session::file_name("Sandbox", pid)), text).expect("written");
    }

    /// The process IDs of `sessions`, in order.
    fn pids(sessions: Vec<(PathBuf, Session)>) -> Vec<u32> {
        let mut pids: Vec<u32> = sessions
            .into_iter()
            .map(|(_, session)| session.pid)
            .collect();
        pids.sort();
        pids
    }

    /// Games cargo built, and one dx built, which builds for its target, are all found.
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

    /// Inside a project, its games' sessions alone are found, by its manifest's path in whatever
    /// form each names it: not another project's, nor a game's that names none.
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

    /// Serves the API's `hello` on the loopback as a game does, on a thread for each connection,
    /// welcoming a client that gives `token` and refusing any other with `api.unauthorized`, while
    /// the tests run; and returns where.
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

    /// A session whose game fails otherwise than by having ended, as one whose file a kill left
    /// naming a port another game has since been given, which refuses its token, is left out while
    /// another answers, and its file kept; alone, its failure is the command's. One whose game has
    /// ended goes, with its file.
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
        let answered = answering(all, "test").expect("one answers");
        let left = [&stale, &live, &ended].map(|(path, _)| path.exists());
        let refused = answering([stale.clone()], "test").err();
        let none = answering(Vec::new(), "test").expect("none to fail");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(pids(answered), [1]);
        assert_eq!(left, [true, true, false]);
        let refused = refused.map(|failure| failure.code);
        assert_eq!(refused.as_deref(), Some("api.unauthorized"));
        assert!(none.is_empty());
    }
}
