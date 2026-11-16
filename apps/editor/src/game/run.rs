//! The project's `gg run`: its process and output, the connection to its game, and the answers.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use gg_api::probe::Probe;
use gg_api::{Client, Failure, ImageList, LogEvents, Looked, Session, Status};
use gg_engine::EntityId;
use serde_json::{Value, json};

use crate::hub::{self};
use crate::look::Part;

use super::{Action, CLIENT, Closing, Game, Level};

/// How long the connection to the game waits to connect. It waits for each answer as long as it
/// takes: a script stopped in the game holds answers while it's debugged, and `gg run` ends a game
/// that hangs, which closes the connection.
const CONNECT: Duration = Duration::from_secs(10);

/// How long each `logs` that follows the game's events waits for them to come, in milliseconds.
const FOLLOWING_MS: u64 = 1_000;

/// How long the connection that follows the game's events waits for each answer: well past
/// [`FOLLOWING_MS`].
const FOLLOWING: Duration = Duration::from_secs(30);

/// How long the editor, as it ends, waits for `gg run` to end the game and itself.
pub(super) const ENDING: Duration = Duration::from_secs(15);

/// How often the editor asks the game how it runs, `status`, which another client may change: the
/// title bar lags such a change by as much.
pub(super) const STATUS_EVERY: Duration = Duration::from_millis(250);

/// A line `gg run` printed: on stdout, its events; on stderr, its errors and what the game and the
/// build print.
pub(super) enum Line {
    Out(String),
    Err(String),
}

/// How the run goes, as the title bar says. For `Running` it says how the game runs instead, as
/// [`Game::label`] has it. A build says where it has come to once `gg run` does, as [`progress`]
/// puts it.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum RunState {
    Building(Option<String>),
    Starting,
    Running,
    Rebuilding(Option<String>),
    Restarting,
    Waiting,
    Ended(String),
}

impl Game {
    /// Starts gg with [`gg_run`]'s arguments in the project's directory, once, with threads that
    /// read what it prints, saying why if it can't. Does nothing outside a project. While the
    /// earlier run in the same directory goes on ending, it waits, each update starting it again.
    pub(super) fn start(&mut self) {
        let Some(dir) = &self.dir else {
            return;
        };
        self.waiting = self.ending.iter().any(|(ending, _)| hub::same(ending, dir));
        if self.waiting {
            return;
        }
        let Some(send) = self.send.take() else {
            return;
        };
        let args = gg_run(self.release);
        let started = (crate::gg(dir).args(&args))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match started {
            Ok(child) => child,
            Err(error) => {
                let why = format!("can't run `gg run` in {}: {error}", dir.display());
                self.log(Level::Error, why.clone());
                self.run_state = RunState::Ended(why);
                return;
            }
        };
        let started = format!("gg {}, in {}", args.join(" "), dir.display());
        self.log(Level::Info, started);
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            unreachable!("each was piped");
        };
        read_lines(stdout, send.clone(), Line::Out);
        read_lines(stderr, send, Line::Err);
        self.run = Some((child, stdin));
    }

    /// Takes what `gg run` printed since: its events, which say how the run goes and connect the
    /// editor to each game that serves; its errors; the rest; and its end.
    pub(super) fn take_lines(&mut self) {
        while let Ok(line) = self.lines.try_recv() {
            match line {
                Line::Out(text) => match serde_json::from_str::<Value>(&text) {
                    Ok(event) if event["event"].is_string() => self.take_event(&event),
                    _ => self.log(Level::Info, text),
                },
                Line::Err(text) => match serde_json::from_str::<Value>(&text) {
                    Ok(error) if error["error"].is_object() => {
                        let message = error["error"]["message"].as_str().unwrap_or_default();
                        let code = error["error"]["code"].as_str().unwrap_or_default();
                        self.log(Level::Error, format!("[{code}] {message}"));
                        self.run_state = RunState::Ended(format!("[{code}] {message}"));
                    }
                    _ => {
                        let level = if text.contains(" ERROR ") {
                            Level::Error
                        } else if text.contains(" WARN ") {
                            Level::Warn
                        } else {
                            Level::Info
                        };
                        self.log(level, text);
                    }
                },
            }
        }
        let ended = (self.run.as_mut()).and_then(|(child, _)| child.try_wait().transpose());
        if let Some(ended) = ended {
            let how = match ended {
                Ok(status) => format!("`gg run` ended, {status}"),
                Err(error) => format!("can't tell how `gg run` went: {error}"),
            };
            self.log(Level::Info, how.clone());
            if !matches!(self.run_state, RunState::Ended(_)) {
                self.run_state = RunState::Ended(how);
            }
            self.run = None;
            self.disconnect();
        }
    }

    /// Takes event `event` of `gg run`'s, logging it.
    fn take_event(&mut self, event: &Value) {
        let name = event["event"].as_str().unwrap_or_default();
        let level = match name {
            "crashed" | "hung" | "rebuild_failed" | "patch_failed" | "refused" => Level::Error,
            "snapshot_failed" | "waiting" | "patches_off" => Level::Warn,
            _ => Level::Info,
        };
        let mut rest = event.clone();
        if let Some(fields) = rest.as_object_mut() {
            fields.remove("event");
            fields.remove("errors");
        }
        let error = rest
            .as_object_mut()
            .and_then(|fields| fields.remove("error"));
        let mut text = format!("{name} {rest}");
        // rustc's errors, as it renders them.
        if let Some(message) = error.as_ref().and_then(|error| error["message"].as_str()) {
            text = format!("{text}\n{message}");
        }
        for logged in event["errors"].as_array().into_iter().flatten() {
            let message = logged["message"].as_str().unwrap_or_default();
            text = format!("{text}\n{message}");
        }
        self.log(level, text);
        match name {
            "building" => {
                if let RunState::Building(at) | RunState::Rebuilding(at) = &mut self.run_state {
                    *at = Some(progress(event));
                }
            }
            "started" => self.run_state = RunState::Starting,
            "serving" => self.serve(event),
            "rebuilding" => self.run_state = RunState::Rebuilding(None),
            // The game before goes on, if one runs.
            "rebuild_failed" if self.talker.is_some() => self.run_state = RunState::Running,
            "rebuild_failed" => self.run_state = RunState::Waiting,
            "rebuilt" => self.run_state = RunState::Starting,
            "crashed" | "hung" | "refused" => self.run_state = RunState::Restarting,
            "waiting" => self.run_state = RunState::Waiting,
            "exited" => self.run_state = RunState::Ended("the game exited".to_owned()),
            _ => {}
        }
        // The game that served has gone, or is going.
        if matches!(
            name,
            "rebuilt" | "crashed" | "hung" | "refused" | "waiting" | "exited" | "closed"
        ) {
            self.disconnect();
        }
    }

    /// Lets go of the connection to the game, which has gone, and of what the next game to serve
    /// answers anew: the input it didn't get, the picks it didn't answer (a drop's among them), its
    /// look, how it ran, and the Inspector's world. The File menu's request goes unanswered, with
    /// what was to follow it and a prompt for New or Open. A close being checked or prompted for
    /// goes by the last status, as [`close`](Self::close) says of one while no game serves.
    pub(super) fn disconnect(&mut self) {
        self.request = None;
        self.then = None;
        self.due = None;
        if self.prompt != Some(Action::Close) {
            self.prompt = None;
        }
        if matches!(self.closing, Closing::Checking(_) | Closing::Prompting) {
            self.settle_close();
        }
        self.talker = None;
        self.debugger.disconnect();
        self.view.link.reach(None);
        self.input.clear();
        self.inputting = false;
        self.told = false;
        self.view.asked = None;
        self.hovering = None;
        self.dropping = None;
        self.answered.clear();
        self.looking.reset();
        self.stopped = true;
        self.stopping = false;
        self.playing = 0;
        self.statusing = false;
        self.polled = None;
    }

    /// Connects to the game `serving`, `gg run`'s event, says serves, in place of the one before.
    fn serve(&mut self, serving: &Value) {
        let session = serving["session"].as_str().map(Path::new).and_then(|file| {
            let text = std::fs::read(file).ok()?;
            serde_json::from_slice::<Session>(&text).ok()
        });
        let Some(session) = session else {
            self.log(
                Level::Error,
                "a game served, but its session file can't be read".to_owned(),
            );
            return;
        };
        self.disconnect();
        self.view.link.reach(Some(session.clone()));
        self.debugger.serve(session.clone());
        self.talker = Some(Talker::connect(session));
        self.served = true;
        // The last game's memory; this one makes its own. The view shows the last frame until
        // this one's first.
        self.reader = None;
        self.asked = None;
        self.refused = false;
        self.paused = false;
        self.run_state = RunState::Running;
    }

    /// Takes the game's answers since:
    ///
    /// - `view` puts the memory it asked for in place of the last.
    /// - `status`, `play`, `stop`, `pause`, `resume`, `step` and `scene` say how it runs, as
    ///   [`take_status`](Self::take_status) takes it. A `scene` goes on as
    ///   [`scene_done`](Self::scene_done) says, and a status asked after a close settles it.
    /// - `look` is taken for what the editor takes of it.
    /// - Errors are logged, a `scene`'s as [`scene_refused`](Self::scene_refused) says.
    pub(super) fn take_answers(&mut self) {
        let Some(talker) = &self.talker else {
            return;
        };
        let answers: Vec<_> = talker.answers.try_iter().collect();
        for (method, answer) in answers {
            let mut scene = None;
            match method.as_str() {
                "input" => self.inputting = false,
                "status" => self.statusing = false,
                // Whether it stopped or refused to.
                "stop" => self.stopping = false,
                "play" => self.playing = self.playing.saturating_sub(1),
                "scene" => scene = self.request.take().map(|request| request.scene),
                _ => {}
            }
            match (method.as_str(), answer) {
                ("view", Ok(_)) => {
                    if let Some(mut asked) = self.asked.take() {
                        asked.unlink();
                        self.reader = Some(asked);
                    }
                }
                (
                    "status" | "play" | "stop" | "pause" | "resume" | "step" | "scene",
                    Ok(status),
                ) => {
                    if let Ok(status) = serde_json::from_value::<Status>(status) {
                        self.take_status(&status);
                        if let Some(scene) = &scene {
                            self.scene_done(scene);
                        }
                    }
                }
                ("cells", Ok(listed)) => match serde_json::from_value::<gg_api::CellList>(listed) {
                    Ok(listed) => self.cells = listed.world,
                    Err(error) => {
                        let message = format!("`cells`: an answer that isn't one: {error}");
                        self.log(Level::Error, message);
                    }
                },
                ("images", Ok(listed)) => match serde_json::from_value::<ImageList>(listed) {
                    Ok(listed) => {
                        self.view.toolbar.images = listed.images;
                        self.view.toolbar.images_refused = None;
                    }
                    Err(error) => {
                        let message = format!("`images`: an answer that isn't one: {error}");
                        self.log(Level::Error, message);
                    }
                },
                ("look", Ok(looked)) => match serde_json::from_value::<Looked>(looked) {
                    Ok(looked) => {
                        let taken = self.looking.read(looked);
                        self.take_look(taken);
                    }
                    Err(error) => {
                        self.looking.failed();
                        self.log(
                            Level::Error,
                            format!("`look`: an answer that isn't one: {error}"),
                        );
                    }
                },
                (_, Ok(_)) => {}
                (method, Err(failure)) => {
                    if method == "view" {
                        self.asked = None;
                        self.refused = true;
                    }
                    if method == "look" {
                        self.looking.failed();
                    }
                    // An older game doesn't know `images` and refuses it.
                    if method == "images" {
                        let Failure { code, message } = &failure;
                        self.view.toolbar.images_refused = Some(format!("[{code}] {message}"));
                    }
                    // The File menu goes on from these; they're no refusals to show.
                    let goes_on =
                        matches!(failure.code.as_str(), "scene.unsaved" | "scene.untitled");
                    let level = if scene.is_some() && goes_on {
                        Level::Info
                    } else {
                        Level::Error
                    };
                    let Failure { code, message } = &failure;
                    self.log(level, format!("`{method}`: [{code}] {message}"));
                    // A connection that didn't open, broke or gave up on an answer takes no more
                    // requests.
                    if method == "hello" || matches!(code.as_str(), "api.closed" | "api.timeout") {
                        self.disconnect();
                        return;
                    }
                    if let Some(scene) = scene {
                        self.scene_refused(scene, failure);
                    }
                }
            }
            // The answer to the status asked after a close, asked one at a time: what it says, or,
            // failed, what the last said.
            if method == "status" && self.closing == Closing::Checking(true) {
                self.settle_close();
            }
        }
    }

    /// Takes `status`, how the game runs as it answered: whether it's paused or stopped, its open
    /// scene's file, and whether it has unsaved edits.
    ///
    /// As it stops (at the editor's Stop, another client's `stop`, or a File menu New or Open that
    /// stopped a copy playing):
    ///
    /// - The unsent input meant for it goes, but its letting go.
    /// - The view turns to the editor's camera, if it was through the game's, whose viewpoint goes
    ///   to the game with the grid, as the user's turn would. A New or Open turns it back to the
    ///   game's, as [`scene_done`](Self::scene_done) says.
    /// - Neither happens while a Play or Simulate clicked since waits for its answer; the input
    ///   then goes to the copy it plays.
    fn take_status(&mut self, status: &Status) {
        self.paused = status.paused;
        self.scene.clone_from(&status.scene);
        self.unsaved = status.unsaved;
        self.streams = status.cells.is_some();
        if !self.streams {
            self.cells = None;
        }
        if status.stopped && !self.stopped && self.playing == 0 {
            self.drop_unsent();
            let toolbar = &mut self.view.toolbar;
            if toolbar.world_camera {
                toolbar.world_camera = false;
                // `changes`, which runs before the answers are taken, doesn't see this turn.
                self.looking.change(Part::Viewpoint);
                self.looking.follow(Part::Grid);
            }
        }
        self.stopped = status.stopped;
    }
}

/// What the editor gives gg to run its game: `run`, optimized if `release`, embedded, starting
/// stopped.
pub(super) fn gg_run(release: bool) -> Vec<&'static str> {
    let optimized: &[&str] = if release { &["--release"] } else { &[] };
    [&["run"], optimized, &["--embedded", "--", "--stopped"]].concat()
}

/// Reads `from` a line at a time on its own thread until it ends, sending each to `send` as `line`
/// makes it.
fn read_lines(from: impl Read + Send + 'static, send: Sender<Line>, line: fn(String) -> Line) {
    thread::spawn(move || {
        for text in BufReader::new(from).lines() {
            let Ok(text) = text else {
                return;
            };
            if send.send(line(text)).is_err() {
                return;
            }
        }
    });
}

/// The fields of an event of the game's JSON log not shown among its other fields: where and when
/// it was logged, and those shown before them.
pub(super) const PLACES: [&str; 9] = [
    "timestamp",
    "level",
    "code",
    "message",
    "target",
    "filename",
    "line_number",
    "spans",
    "threadName",
];

/// What `gg run`'s `building` event says the build has come to, for the title bar: while
/// compiling, the crates dx counts compiled, as `212 of 530 crates`; else its stage, as
/// `linking`.
fn progress(event: &Value) -> String {
    match (event["compiled"].as_u64(), event["crates"].as_u64()) {
        (Some(compiled), Some(crates)) => format!("{compiled} of {crates} crates"),
        _ => event["stage"]
            .as_str()
            .unwrap_or_default()
            .replace('_', " "),
    }
}

/// Follows the log of the game `session` names on a connection of its own, as the editor, sending
/// `send` each `logs`'s events as it comes, from the oldest the game keeps, until the game ends or
/// the editor lets go (`send`'s receiver dropping). Each `logs` sends, events or none.
fn follow(session: &Session, send: &Sender<Vec<Value>>) {
    let connected = Client::connect(session, CLIENT, CONNECT)
        .and_then(|mut client| client.set_timeout(Some(FOLLOWING)).map(|()| client));
    let Ok(mut client) = connected else {
        return;
    };
    let mut params = json!({"wait_ms": FOLLOWING_MS});
    while let Ok(read) = client.call_as::<LogEvents>("logs", &params) {
        let events = read.events.iter();
        let events = events.filter_map(|event| serde_json::from_str(event.get()).ok());
        if send.send(events.collect()).is_err() {
            return;
        }
        params = json!({"from": read.next, "run": read.run, "wait_ms": FOLLOWING_MS});
    }
}

/// Asks the game `session` names, on a connection of its own, as the editor, for the entity its
/// view shows at each pixel `asks` gives, in turn, within the model of the entity given, if any,
/// as [`gg_api::Pick::within`] says. Sends `send` each answer with its ticket, or why there's
/// none, until the editor lets go (`asks`' sender dropping).
fn pick(
    session: &Session,
    asks: Receiver<(u64, [f64; 2], Option<EntityId>)>,
    send: &Sender<(u64, Result<gg_api::Picked, Failure>)>,
) {
    let mut connected = Client::connect(session, CLIENT, CONNECT)
        .and_then(|mut client| client.set_timeout(None).map(|()| client));
    for (ticket, at, within) in asks {
        let within = within.map(|id| id.to_string());
        let answer = match &mut connected {
            Ok(client) => client.call_as("pick", gg_api::Pick { at, within }),
            Err(failure) => Err(failure.clone()),
        };
        if send.send((ticket, answer)).is_err() {
            return;
        }
    }
}

/// Asks the game `session` names, on a connection of its own, as the editor, what shading took and
/// made at each pixel of its view `asks` gives, in turn, as `gg probe` does, the view as the look
/// shows it. Sends `send` each answer with its ticket, or why there's none, until the editor lets
/// go (`asks`' sender dropping).
fn probe(
    session: &Session,
    asks: Receiver<(u64, [f64; 2])>,
    send: &Sender<(u64, Result<Probe, Failure>)>,
) {
    let mut connected = Client::connect(session, CLIENT, CONNECT)
        .and_then(|mut client| client.set_timeout(None).map(|()| client));
    for (ticket, at) in asks {
        let answer = match &mut connected {
            Ok(client) => {
                gg_api::probe::probe(|method, params| client.call(method, params), at, false)
            }
            Err(failure) => Err(failure.clone()),
        };
        if send.send((ticket, answer)).is_err() {
            return;
        }
    }
}

/// Connections to the game, each on its own thread so no frame waits for an answer:
///
/// - Requests go in order; answers come back in order.
/// - One follows the game's log, as [`follow`] does; events come back as they're read, each
///   read's together.
/// - Picks go to one, as [`pick`] does; each waits for a frame to read it, and answers come back
///   in order with their tickets.
/// - Probes go to one, as [`probe`] does, likewise; each waits for a frame for each capture.
pub(super) struct Talker {
    pub(super) asks: Sender<(String, Value)>,
    pub(super) answers: Receiver<(String, Result<Value, Failure>)>,
    pub(super) logged: Receiver<Vec<Value>>,
    pub(super) picks: Sender<(u64, [f64; 2], Option<EntityId>)>,
    pub(super) picked: Receiver<(u64, Result<gg_api::Picked, Failure>)>,
    pub(super) probes: Sender<(u64, [f64; 2])>,
    pub(super) probed: Receiver<(u64, Result<Probe, Failure>)>,
}

impl Talker {
    /// Connects to the game `session` names, as the editor, the author of the edits it applies.
    fn connect(session: Session) -> Self {
        let (asks, asked) = mpsc::channel::<(String, Value)>();
        let (answer, answers) = mpsc::channel();
        let (log, logged) = mpsc::channel();
        let (picks, picking) = mpsc::channel();
        let (found, picked) = mpsc::channel();
        let (probes, probing) = mpsc::channel();
        let (read, probed) = mpsc::channel();
        let followed = session.clone();
        thread::spawn(move || follow(&followed, &log));
        let picker = session.clone();
        thread::spawn(move || pick(&picker, picking, &found));
        let prober = session.clone();
        thread::spawn(move || probe(&prober, probing, &read));
        thread::spawn(move || {
            let connected = Client::connect(&session, CLIENT, CONNECT)
                .and_then(|mut client| client.set_timeout(None).map(|()| client));
            let mut client = match connected {
                Ok(client) => client,
                Err(failure) => {
                    let _ = answer.send(("hello".to_owned(), Err(failure)));
                    return;
                }
            };
            for (method, params) in asked {
                let result = client
                    .call(&method, params)
                    .map(|result| serde_json::from_str(result.get()).unwrap_or(Value::Null));
                if answer.send((method, result)).is_err() {
                    return;
                }
            }
        });
        Self {
            asks,
            answers,
            logged,
            picks,
            picked,
            probes,
            probed,
        }
    }

    /// Sends request `method` with `params`, whose answer comes back in `answers`.
    pub(super) fn ask(&self, method: &str, params: Value) {
        let _ = self.asks.send((method.to_owned(), params));
    }

    /// Asks for the entity the game's view shows at `at`, in its pixels from its top left, within
    /// the model of `within`; the answer comes back in `picked` with `ticket`.
    pub(super) fn pick(&self, ticket: u64, at: [f64; 2], within: Option<EntityId>) {
        let _ = self.picks.send((ticket, at, within));
    }

    /// Asks what shading took and made at `at`, in the view's pixels from its top left, whose
    /// answer comes back in `probed` with `ticket`.
    pub(super) fn probe(&self, ticket: u64, at: [f64; 2]) {
        let _ = self.probes.send((ticket, at));
    }
}
