//! The game of the project the editor is started in, run as an agent runs it: under `cargo gg run
//! --embedded` in the project's directory, which builds it, patches it as its code changes and
//! restarts it after any other change, and which ends, with the game, as the editor ends, closing
//! its stdin. The game renders offscreen for the editor, which shows its view in the Viewport from
//! memory the two share, as the tool API's `view` asks, and forwards it the input the view gets.
//! The Logs panel shows `gg run`'s events, the build's errors among them, and what the game and
//! the build print; the title bar, how the run goes, with pause and step.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gg_api::view::Reader;
use gg_api::{Client, Failure, InputEvent, Session, Status};
use gg_engine::egui::{self, Color32, RichText, ScrollArea};
use gg_engine::{App, Context, Docked, Event, Layer, MouseButton, PerfPanel, Scroll, Ui};
use serde_json::{Value, json};
use tracing::{info, warn};

/// The most lines the Logs panel keeps, past which the oldest go.
const LOGGED: usize = 5000;

/// How long the connection to the game waits to connect.
const CONNECT: Duration = Duration::from_secs(10);

/// How long it waits for each answer: as long as `gg run` gives a game to run its first frame,
/// which answers the first requests.
const ANSWER: Duration = Duration::from_secs(120);

/// How long the editor, as it ends, waits for `gg run` to end the game and itself.
const ENDING: Duration = Duration::from_secs(15);

/// How many frames the measures of the view's frames keep, from the last; 68 s at 240 fps.
const MEASURED: usize = 16_384;

/// The editor of the project in directory `dir`: the game's view in the Viewport, the Logs below
/// it, and the Performance panel beside it.
pub(crate) fn app(dir: PathBuf) -> App {
    let app = App::new("Editor").root(env!("CARGO_MANIFEST_DIR"));
    let app = app.custom_title_bar().view_panel("Viewport");
    let app = app
        .dock("Logs", Docked::Below("Viewport"))
        .dock("Performance", Docked::RightOf("Viewport"));
    app.layer(Game::new(dir)).overlay(PerfPanel)
}

/// The editor's layer in a project: the `gg run` of its game, the connection to the game that
/// serves, the memory its view is shared through, and what the Logs panel and the title bar show.
struct Game {
    dir: PathBuf,
    /// `gg run`, until it ends, with its stdin, which the editor closes as it ends.
    run: Option<(Child, ChildStdin)>,
    /// What `gg run` prints, a line at a time.
    lines: Receiver<Line>,
    /// What the threads reading `gg run`'s output send `lines`, until the run starts.
    send: Option<Sender<Line>>,
    /// The connection to the game that serves, on a thread of its own.
    talker: Option<Talker>,
    /// The memory the game's view is shared through, and memory a `view` asked the game to share it
    /// through instead, until the game answers; and whether the game refused a `view`, after which
    /// the editor asks for none until another game serves.
    reader: Option<Reader>,
    asked: Option<Reader>,
    refused: bool,
    /// The input the view got for the game, since the last `input` sent, and whether that's still
    /// to be answered: one at a time, as the game answers one a frame.
    input: Vec<InputEvent>,
    inputting: bool,
    run_state: RunState,
    /// Whether the game's simulation is paused, as it last answered.
    paused: bool,
    logs: VecDeque<Logged>,
    measures: Measures,
}

/// A line `gg run` printed: on stdout, its events; on stderr, its errors, and what the game and the
/// build print.
enum Line {
    Out(String),
    Err(String),
}

/// How the run goes, as the title bar says.
#[derive(Clone, Debug, PartialEq)]
enum RunState {
    Building,
    Starting,
    Running,
    Rebuilding,
    Restarting,
    Waiting,
    Ended(String),
}

impl RunState {
    fn text(&self) -> String {
        match self {
            Self::Building => "Building…".to_owned(),
            Self::Starting => "Starting…".to_owned(),
            Self::Running => "Running".to_owned(),
            Self::Rebuilding => "Rebuilding…".to_owned(),
            Self::Restarting => "Restarting…".to_owned(),
            Self::Waiting => "Waiting for a rebuild".to_owned(),
            Self::Ended(why) => format!("Ended: {why}"),
        }
    }
}

/// A line of the Logs panel.
struct Logged {
    level: Level,
    text: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Level {
    Info,
    Warn,
    Error,
}

impl Game {
    fn new(dir: PathBuf) -> Self {
        let (send, lines) = mpsc::channel();
        Self {
            dir,
            run: None,
            lines,
            send: Some(send),
            talker: None,
            reader: None,
            asked: None,
            refused: false,
            input: Vec::new(),
            inputting: false,
            run_state: RunState::Building,
            paused: false,
            logs: VecDeque::new(),
            measures: Measures::default(),
        }
    }

    /// Starts `cargo gg run --embedded` in the project's directory, with threads that read what it
    /// prints; logs why if it can't.
    fn start(&mut self) {
        let Some(send) = self.send.take() else {
            return;
        };
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let started = Command::new(cargo)
            .args(["gg", "run", "--embedded"])
            .current_dir(&self.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match started {
            Ok(child) => child,
            Err(error) => {
                let why = format!(
                    "can't run `cargo gg run` in {}: {error}",
                    self.dir.display()
                );
                self.log(Level::Error, why.clone());
                self.run_state = RunState::Ended(why);
                return;
            }
        };
        self.log(
            Level::Info,
            format!("cargo gg run --embedded, in {}", self.dir.display()),
        );
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            unreachable!("each was piped");
        };
        read_lines(stdout, send.clone(), Line::Out);
        read_lines(stderr, send, Line::Err);
        self.run = Some((child, stdin));
    }

    fn log(&mut self, level: Level, text: String) {
        if self.logs.len() == LOGGED {
            self.logs.pop_front();
        }
        self.logs.push_back(Logged { level, text });
    }

    /// Takes what `gg run` printed since: its events, which say how the run goes, and connect the
    /// editor to each game that serves; its errors; and what else it printed; and its end.
    fn take_lines(&mut self) {
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
            self.talker = None;
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
            "started" => self.run_state = RunState::Starting,
            "serving" => self.serve(event),
            "rebuilding" => self.run_state = RunState::Rebuilding,
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

    /// Lets go of the connection to the game, which has gone, and of the input it didn't get.
    fn disconnect(&mut self) {
        self.talker = None;
        self.input.clear();
        self.inputting = false;
    }

    /// Connects to the game that `serving`, `gg run`'s event, says serves, in place of the one
    /// before, which has ended.
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
        self.talker = Some(Talker::connect(session));
        // The game before's memory, which this one makes anew; the view shows its last frame
        // until this one's first.
        self.reader = None;
        self.asked = None;
        self.refused = false;
        self.paused = false;
        self.run_state = RunState::Running;
    }

    /// Takes the game's answers since: to `view`, which puts the memory it asked for in place of
    /// the one before; to `pause`, `resume` and `step`, which say whether it's paused; and errors,
    /// which it logs.
    fn take_answers(&mut self) {
        let Some(talker) = &self.talker else {
            return;
        };
        let answers: Vec<_> = talker.answers.try_iter().collect();
        for (method, answer) in answers {
            if method == "input" {
                self.inputting = false;
            }
            match (method.as_str(), answer) {
                ("view", Ok(_)) => {
                    if let Some(mut asked) = self.asked.take() {
                        asked.unlink();
                        self.reader = Some(asked);
                    }
                }
                ("pause" | "resume" | "step", Ok(status)) => {
                    if let Ok(status) = serde_json::from_value::<Status>(status) {
                        self.paused = status.paused;
                    }
                }
                (_, Ok(_)) => {}
                (method, Err(failure)) => {
                    if method == "view" {
                        self.asked = None;
                        self.refused = true;
                    }
                    let Failure { code, message } = failure;
                    self.log(Level::Error, format!("`{method}`: [{code}] {message}"));
                    // A connection that didn't open, broke or gave up on an answer takes no more
                    // requests.
                    if method == "hello" || matches!(code.as_str(), "api.closed" | "api.timeout") {
                        self.disconnect();
                        return;
                    }
                }
            }
        }
    }

    /// Has the game render its view at `size`, the Viewport's in pixels: in the memory it's shared
    /// through, if it fits there, or else in memory made for it, a bit larger, so that a drag that
    /// grows the view makes few, which a `view` asks the game to share it through, once the last
    /// asked is answered.
    fn fit(&mut self, size: [u32; 2]) {
        let Some(talker) = &self.talker else {
            return;
        };
        // Past the largest side frames have, the view shows as much as a frame does.
        let size = size.map(|side| side.min(gg_api::view::MAX_SIDE));
        let fits = self.reader.as_ref().is_some_and(|reader| reader.ask(size));
        if self.asked.is_some() || self.refused || fits {
            return;
        }
        let roomy = size.map(|side| side.next_multiple_of(256).min(gg_api::view::MAX_SIDE));
        match Reader::create(roomy[0], roomy[1]) {
            Ok(reader) => {
                let fits = reader.ask(size);
                debug_assert!(fits, "the memory is as large as the frames asked");
                talker.ask("view", json!({"memory": reader.name()}));
                self.asked = Some(reader);
            }
            Err(error) => {
                let message = format!("can't make memory for a {size:?} view: {error}");
                self.log(Level::Error, message);
            }
        }
    }
}

impl Layer for Game {
    fn attach(&mut self, _: &mut Context) {
        self.start();
    }

    fn update(&mut self, cx: &mut Context, _: Duration) {
        self.take_lines();
        self.take_answers();
        if let Some(view) = cx.view() {
            self.fit(view.size);
        }
        if let Some(reader) = &mut self.reader
            && let Some(frame) = reader.take()
        {
            let copying = Instant::now();
            cx.show_pixels(frame.size, |pixels| reader.read(pixels));
            self.measures
                .add(frame.started, copying.elapsed(), frame.size);
        }
        if let Some(talker) = &self.talker
            && !self.input.is_empty()
            && !self.inputting
        {
            let events = std::mem::take(&mut self.input);
            talker.ask("input", json!({ "events": events }));
            self.inputting = true;
        }
    }

    /// Keeps what the view gets for the game, while one serves.
    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> std::ops::ControlFlow<()> {
        let origin = cx.view().map_or([0, 0], |view| view.origin);
        if self.talker.is_some()
            && let Some(input) = forwarded(event, origin)
        {
            self.input.push(input);
        }
        std::ops::ControlFlow::Continue(())
    }

    fn ui(&mut self, _: &mut Context, ui: &mut Ui<'_>) {
        let talker = self.talker.as_ref();
        let paused = self.paused;
        ui.title_bar(|ui| {
            ui.label(self.run_state.text());
            let serves = talker.is_some();
            let (method, label, tip) = if paused {
                (
                    "resume",
                    "Resume",
                    "Run the game's simulation in real time again",
                )
            } else {
                ("pause", "Pause", "Pause the game's simulation")
            };
            let button = ui
                .add_enabled(serves, egui::Button::new(label))
                .on_hover_text(tip);
            if button.clicked()
                && let Some(talker) = talker
            {
                talker.ask(method, Value::Null);
            }
            let step = ui.add_enabled(serves, egui::Button::new("Step"));
            if step
                .on_hover_text("Run a tick of the game's simulation, and pause it")
                .clicked()
                && let Some(talker) = talker
            {
                talker.ask("step", Value::Null);
            }
        });
        let logs = &self.logs;
        ui.panel("Logs", |ui| {
            ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink(false)
                .show(ui, |ui| {
                    for logged in logs {
                        let text = RichText::new(&logged.text).monospace();
                        let text = match logged.level {
                            Level::Info => text,
                            Level::Warn => text.color(Color32::from_rgb(230, 190, 60)),
                            Level::Error => text.color(Color32::from_rgb(240, 90, 80)),
                        };
                        ui.label(text);
                    }
                });
        });
    }

    /// Closes `gg run`'s stdin, which ends the game and the run, and, while a game serves, waits for
    /// it to end, for at most [`ENDING`], rather than for a build under way, which `gg run` finishes
    /// first; then logs what it measured of the view's frames.
    fn detach(&mut self, _: &mut Context) {
        let serves = self.talker.is_some();
        self.disconnect();
        if let Some((mut child, stdin)) = self.run.take() {
            drop(stdin);
            if !serves {
                self.measures.report();
                return;
            }
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if start.elapsed() < ENDING => {
                        thread::sleep(Duration::from_millis(20))
                    }
                    Ok(None) => {
                        warn!(
                            code = "editor.run",
                            "`gg run` didn't end within {} s of the editor's",
                            ENDING.as_secs()
                        );
                        break;
                    }
                    Err(error) => {
                        warn!(code = "editor.run", "can't wait for `gg run`: {error}");
                        break;
                    }
                }
            }
        }
        self.measures.report();
    }
}

/// Reads `from` a line at a time on a thread of its own, until it ends, sending each to `send` as
/// `line` makes it one.
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

/// The event of the tool API's `input` that the game gets for `event`, which the view got, the
/// cursor's position taken from the main window's pixels to the view's, whose top left is at
/// `origin`; `None` for what the game reads itself, or doesn't get.
fn forwarded(event: &Event<'_>, origin: [u32; 2]) -> Option<InputEvent> {
    let event = match *event {
        Event::Key {
            key,
            pressed,
            repeat,
        } => InputEvent::Key {
            key: key.name().to_owned(),
            pressed,
            repeat,
        },
        Event::Text(text) => InputEvent::Text(text.to_owned()),
        Event::CursorMoved { x, y } => {
            InputEvent::Cursor([x - f64::from(origin[0]), y - f64::from(origin[1])])
        }
        Event::CursorEntered => InputEvent::CursorEntered,
        Event::CursorLeft => InputEvent::CursorLeft,
        Event::MouseButton { button, pressed } => InputEvent::Button {
            button: match button {
                MouseButton::Left => gg_api::Button::Left,
                MouseButton::Right => gg_api::Button::Right,
                MouseButton::Middle => gg_api::Button::Middle,
                MouseButton::Back => gg_api::Button::Back,
                MouseButton::Forward => gg_api::Button::Forward,
                MouseButton::Other(number) => gg_api::Button::Other(number),
            },
            pressed,
        },
        Event::Scroll(Scroll::Lines { x, y }) => InputEvent::Scroll(gg_api::Scroll::Lines([x, y])),
        Event::Scroll(Scroll::Pixels { x, y }) => {
            InputEvent::Scroll(gg_api::Scroll::Pixels([x, y]))
        }
        Event::Focused(focused) => InputEvent::Focused(focused),
        _ => return None,
    };
    Some(event)
}

/// A connection to the game, on a thread of its own, so that no frame waits for the game's answer:
/// requests go to it in order, and their answers come back in order.
struct Talker {
    asks: Sender<(String, Value)>,
    answers: Receiver<(String, Result<Value, Failure>)>,
}

impl Talker {
    /// Connects to the game `session` names, as the editor, the author of the edits it applies.
    fn connect(session: Session) -> Self {
        let (asks, asked) = mpsc::channel::<(String, Value)>();
        let (answer, answers) = mpsc::channel();
        thread::spawn(move || {
            let connected = Client::connect(&session, "editor", CONNECT)
                .and_then(|mut client| client.set_timeout(Some(ANSWER)).map(|()| client));
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
        Self { asks, answers }
    }

    /// Sends request `method` with `params`, whose answer comes back in `answers`.
    fn ask(&self, method: &str, params: Value) {
        let _ = self.asks.send((method.to_owned(), params));
    }
}

/// What the editor measures of the view's frames, as it shows them: each frame's age, from when
/// the game's frame that rendered it started to when the editor took it, and how long copying it
/// out of the memory took, of the last [`MEASURED`] frames; and the size of the last.
#[derive(Default)]
struct Measures {
    frames: u64,
    ages: Vec<Duration>,
    copies: Vec<Duration>,
    size: [u32; 2],
}

impl Measures {
    fn add(&mut self, started: u64, copy: Duration, size: [u32; 2]) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let age = now.saturating_sub(Duration::from_nanos(started));
        let at = (self.frames % MEASURED as u64) as usize;
        if self.ages.len() < MEASURED {
            self.ages.reserve_exact(MEASURED - self.ages.len());
            self.copies.reserve_exact(MEASURED - self.copies.len());
            self.ages.push(age);
            self.copies.push(copy);
        } else {
            self.ages[at] = age;
            self.copies[at] = copy;
        }
        self.frames += 1;
        self.size = size;
    }

    /// Logs them, `editor.view`, as the editor ends, in milliseconds: their median, 95th and 99th
    /// percentiles, by nearest rank, and maximum.
    fn report(&self) {
        if self.frames == 0 {
            return;
        }
        let stats = |times: &[Duration]| {
            let mut sorted = times.to_vec();
            sorted.sort_unstable();
            let at = |percent: usize| {
                let rank = (percent * sorted.len()).div_ceil(100);
                sorted[rank - 1].as_secs_f64() * 1000.0
            };
            let max = sorted.last().copied().unwrap_or_default().as_secs_f64() * 1000.0;
            [at(50), at(95), at(99), max]
        };
        let [age_p50, age_p95, age_p99, age_max] = stats(&self.ages);
        let [copy_p50, copy_p95, copy_p99, copy_max] = stats(&self.copies);
        info!(
            code = "editor.view",
            frames = self.frames,
            measured = self.ages.len(),
            width = self.size[0],
            height = self.size[1],
            age_ms_p50 = age_p50,
            age_ms_p95 = age_p95,
            age_ms_p99 = age_p99,
            age_ms_max = age_max,
            copy_ms_p50 = copy_p50,
            copy_ms_p95 = copy_p95,
            copy_ms_p99 = copy_p99,
            copy_ms_max = copy_max,
            "The game's view, as the editor showed it"
        );
    }
}

#[cfg(test)]
mod tests {
    use gg_engine::Key;

    use super::*;

    #[test]
    fn the_views_input_reaches_the_game_in_the_views_pixels() {
        let origin = [10, 20];
        let key = Event::Key {
            key: Key::KeyW,
            pressed: true,
            repeat: false,
        };
        let moved = Event::CursorMoved { x: 15.5, y: 20.0 };
        let wheel = Event::Scroll(Scroll::Lines { x: 0.0, y: -1.0 });
        let forwarded: Vec<_> = [
            key,
            moved,
            wheel,
            Event::Resized {
                width: 1,
                height: 1,
            },
        ]
        .iter()
        .filter_map(|event| forwarded(event, origin))
        .collect();
        let expected = [
            InputEvent::Key {
                key: "KeyW".to_owned(),
                pressed: true,
                repeat: false,
            },
            InputEvent::Cursor([5.5, 0.0]),
            InputEvent::Scroll(gg_api::Scroll::Lines([0.0, -1.0])),
        ];
        assert_eq!(
            forwarded, expected,
            "the window's own events stay the editor's"
        );
    }

    #[test]
    fn gg_runs_events_say_how_the_run_goes_and_reach_the_logs() {
        let mut game = Game::new(PathBuf::from("."));
        let send = game.send.take().expect("not started");
        let failed = json!({"code": "gg.build", "message": "error[E0308]: mismatched types"});
        let running = json!({"code": "gg.running", "message": "another `gg run` runs shards"});
        let lines = [
            Line::Out(json!({"event": "started", "pid": 7}).to_string()),
            Line::Out(json!({"event": "rebuild_failed", "error": failed}).to_string()),
            Line::Err("   1.000  WARN  engine  Something".to_owned()),
            Line::Err(json!({ "error": running }).to_string()),
        ];
        for line in lines {
            send.send(line).expect("sent");
        }
        game.take_lines();
        let levels: Vec<_> = game.logs.iter().map(|logged| logged.level).collect();
        assert_eq!(
            levels,
            [Level::Info, Level::Error, Level::Warn, Level::Error]
        );
        assert!(
            game.logs[1].text.contains("mismatched types"),
            "rustc's errors show"
        );
        assert_eq!(
            game.run_state,
            RunState::Ended("[gg.running] another `gg run` runs shards".to_owned())
        );
    }
}
