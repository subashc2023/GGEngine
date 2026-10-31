//! The game of the project the editor is started in, run as an agent runs it: under `cargo gg run
//! --embedded` in the project's directory, which builds it, patches it as its code changes and
//! restarts it after any other change, and which ends, with the game, as the editor ends, closing
//! its stdin. The game renders offscreen for the editor, which shows its view in the Viewport from
//! memory the two share, as the tool API's `view` asks. The Entities and Inspector panels show the
//! game's world, and edit it, through the API, as a remote [`Inspector`] does; the Logs panel shows
//! `gg run`'s events, the build's errors among them, the game's events as it logs them, which the
//! tool API's `logs` follows, and what the game and the build print; the title bar, how the run
//! goes, with pause and step.
//!
//! The view's tools edit the game's entities as they do the editor's own world's. The view starts
//! through the game's own camera, which gets the input the view gets, but what the editor's camera
//! takes, which turns the view to it, the game letting go of what's held; through the editor's
//! camera, the game gets none, and the handles show. A click in the view picks what the game's
//! view shows there, as the API's `pick` finds it, which the panels show picked; the handles move,
//! turn and scale it through the Inspector's link, each drag one edit of the editor's, which its
//! Edit menu undoes; and the view's bottom left names what's under the cursor. The editor's camera,
//! the outline round what's picked, the grid and the colliders' switch reach the game as parts of
//! its look, as [`look`](crate::look) says: each as the user changes it, the editor taking the parts
//! another client, such as an agent, sets.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read};
use std::mem;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gg_api::view::Reader;
use gg_api::{Client, Failure, InputEvent, LogEvents, Looked, Session, Status};
use gg_engine::egui::{self, Color32, RichText, ScrollArea};
use gg_engine::grid::Plane;
use gg_engine::math::{Quat, Vec3};
use gg_engine::{
    App, Context, Docked, EntityId, Event, Inspector, InspectorLink, Layer, MouseButton, PerfPanel,
    Projection, Scroll, Ui, Viewpoint, Viewport,
};
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::handles::{Picked, Reading, Sight};
use crate::look::{Looking, Part, Parts};
use crate::overlay::Toolbar;
use crate::view::{self, OUTLINE, OUTLINE_WIDTH, ViewTools};

/// The most lines the Logs panel keeps, past which the oldest go.
const LOGGED: usize = 5000;

/// How long the connection to the game waits to connect.
const CONNECT: Duration = Duration::from_secs(10);

/// How long it waits for each answer: as long as `gg run` gives a game to run its first frame,
/// which answers the first requests.
const ANSWER: Duration = Duration::from_secs(120);

/// How long each `logs` that follows the game's events waits for them to come, in milliseconds.
const FOLLOWING_MS: u64 = 1_000;

/// How long the connection that follows the game's events waits for each answer: well past the
/// time each `logs` waits.
const FOLLOWING: Duration = Duration::from_secs(30);

/// How long the editor, as it ends, waits for `gg run` to end the game and itself.
const ENDING: Duration = Duration::from_secs(15);

/// How many frames the measures of the view's frames keep, from the last; 68 s at 240 fps.
const MEASURED: usize = 16_384;

/// The editor of the project in directory `dir`: the game's view in the Viewport, its entities
/// left of it, the one picked right of it, above the Performance panel, and the Logs below it.
pub(crate) fn app(dir: PathBuf) -> App {
    let app = App::new("Editor").root(env!("CARGO_MANIFEST_DIR"));
    let app = app.custom_title_bar().view_panel("Viewport");
    let app = app
        .dock("Inspector", Docked::RightOf("Viewport"))
        .dock("Entities", Docked::LeftOf("Viewport"))
        .dock("Performance", Docked::Below("Inspector"))
        .dock("Logs", Docked::Below("Viewport"));
    let inspector = Inspector::remote(CLIENT);
    let game = Game::new(dir, inspector.link());
    app.layer(game).overlay(PerfPanel).overlay(inspector)
}

/// The client the editor is to the game, the author of its edits.
const CLIENT: &str = "editor";

/// The editor's layer in a project: the `gg run` of its game, the connection to the game that
/// serves, the memory its view is shared through, and what the Logs panel and the title bar show;
/// and the view's tools, the picks they ask of the game and the look they keep with it.
struct Game {
    dir: PathBuf,
    /// `gg run`, until it ends, with its stdin, which the editor closes as it ends.
    run: Option<(Child, ChildStdin)>,
    /// What `gg run` prints, a line at a time.
    lines: Receiver<Line>,
    /// What the threads reading `gg run`'s output send `lines`, until the run starts.
    send: Option<Sender<Line>>,
    /// The connections to the game that serves, each on a thread of its own.
    talker: Option<Talker>,
    /// The view's tools, whose link is the Inspector's, which shows the world of the game that
    /// serves; each pick's ticket the next of the editor's count, `tickets`, and what one finds the
    /// entity the game's view showed.
    view: ViewTools<u64, Found>,
    tickets: u64,
    /// The ticket of the pick at the cursor that waits for its answer, if one does: one at a time.
    hovering: Option<u64>,
    /// The answers to picks that came back since the last update, each with its ticket.
    answered: Vec<(u64, Option<Found>)>,
    /// The game's look, as the editor keeps it.
    looking: Looking,
    /// What the editor's wanted parts of the look went by as the last update left them, and the
    /// pick that update asked the Inspector for as the look's outline, if it asked for one:
    /// `Some(None)` unpicks.
    wants: Wants,
    took_pick: Option<Option<EntityId>>,
    /// Whether the game's camera drove the view as the game was last told: as the view turns to the
    /// editor's camera, the game lets go of what's held, and as it turns back, it gets the cursor.
    told: bool,
    /// The last frame of the game's view the editor showed: its size, and what it was drawn from,
    /// if that projects.
    size: Option<[u32; 2]>,
    seen: Option<Viewpoint>,
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

/// What a pick of the game's view found: an entity, by its ID, and what the view's bottom left
/// calls it, its name, or without one, its ID.
#[derive(Clone, Debug, PartialEq)]
struct Found {
    id: EntityId,
    name: String,
}

impl Found {
    /// What the game's answer to a pick, `picked`, found: none outside the view, nor where no entity
    /// draws.
    fn of(picked: gg_api::Picked) -> Option<Self> {
        let id: EntityId = picked.id?.parse().ok()?;
        let name = picked.name.unwrap_or_else(|| id.to_string());
        Some(Self { id, name })
    }
}

/// What the parts of the look the editor wants go by, which between updates only the user changes:
/// whether the game's camera drives the view, what the editor's camera sees from, what the
/// Inspector has picked, and the toolbar's grid, its plane and the colliders' switch.
#[derive(Clone, Copy, PartialEq)]
struct Wants {
    through: bool,
    viewpoint: Viewpoint,
    picked: Option<EntityId>,
    grid: bool,
    plane: Plane,
    colliders: bool,
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

/// How a line of the Logs panel shows: `Debug` for the game's debug and trace events, which
/// recede.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Game {
    /// The layer, through the game's camera at first, whose tools pick and edit through
    /// `inspector`, the Inspector's link.
    fn new(dir: PathBuf, inspector: InspectorLink) -> Self {
        let (send, lines) = mpsc::channel();
        let toolbar = Toolbar {
            world_camera: true,
            game: true,
            ..Toolbar::default()
        };
        let view = ViewTools::new(inspector, toolbar);
        let wants = wants(&view);
        Self {
            dir,
            run: None,
            lines,
            send: Some(send),
            talker: None,
            view,
            tickets: 0,
            hovering: None,
            answered: Vec::new(),
            looking: Looking::new(CLIENT),
            wants,
            took_pick: None,
            told: true,
            size: None,
            seen: None,
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
            self.disconnect();
        }
    }

    /// Takes the game's events logged since, each a line of the Logs panel, as [`logged`] makes it.
    fn take_logged(&mut self) {
        let Some(talker) = &self.talker else {
            return;
        };
        let reads: Vec<Vec<Value>> = talker.logged.try_iter().collect();
        for event in reads.iter().flatten() {
            let Logged { level, text } = logged(event);
            self.log(level, text);
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

    /// Lets go of the connection to the game, which has gone, of the input it didn't get, of the
    /// picks it didn't answer and of its look; and the Inspector, of its world.
    fn disconnect(&mut self) {
        self.talker = None;
        self.view.link.reach(None);
        self.input.clear();
        self.inputting = false;
        self.view.asked = None;
        self.hovering = None;
        self.answered.clear();
        self.looking.reset();
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
        self.view.link.reach(Some(session.clone()));
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
    /// the one before; to `pause`, `resume` and `step`, which say whether it's paused; to `look`,
    /// what the editor takes of which it takes; and errors, which it logs.
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

    /// Takes the parts of the look that `taken` sets, as another client set them: a viewpoint the
    /// editor's camera adopts, driving the view, and `null` turns the view back to the game's
    /// camera, either turn taking the grid with it, as the user's does, unless another client set
    /// the grid; an outline's entity the Inspector picks, and `null` unpicks; and the grid and the
    /// colliders set the toolbar's switches, and the grid its plane.
    fn take_look(&mut self, taken: gg_api::Look) {
        let toolbar = &mut self.view.toolbar;
        let through = toolbar.world_camera;
        match taken.viewpoint {
            Some(Some(viewpoint)) => {
                self.view.camera.adopt(&engine_viewpoint(&viewpoint));
                toolbar.world_camera = false;
            }
            Some(None) => toolbar.world_camera = true,
            None => {}
        }
        if toolbar.world_camera != through {
            self.looking.follow(Part::Grid);
        }
        if let Some(outline) = taken.outline {
            let id = outline.and_then(|outline| outline.id.parse().ok());
            self.view.link.pick(id);
            self.took_pick = Some(id);
        }
        if let Some(grid) = taken.grid {
            toolbar.grid = grid.is_some();
            if let Some(plane) = grid {
                toolbar.plane = engine_plane(plane);
            }
        }
        if let Some(colliders) = taken.colliders {
            toolbar.colliders = colliders == Some(true);
        }
    }

    /// Takes the answers to picks that came back since: the latest click's, which the Inspector
    /// picks, and all of them for what's under the cursor, which [`ui`](Layer::ui) reads; and
    /// failures, which it logs.
    fn take_picks(&mut self) {
        self.answered.clear();
        let Some(talker) = &self.talker else {
            return;
        };
        let answers: Vec<_> = talker.picked.try_iter().collect();
        for (ticket, answer) in answers {
            if self.hovering == Some(ticket) {
                self.hovering = None;
            }
            match answer {
                Ok(picked) => self.answered.push((ticket, Found::of(picked))),
                Err(Failure { code, message }) => {
                    if self.view.asked == Some(ticket) {
                        self.view.asked = None;
                    }
                    self.log(Level::Error, format!("`pick`: [{code}] {message}"));
                    // As a request's connection that broke or gave up on an answer.
                    if matches!(code.as_str(), "api.closed" | "api.timeout") {
                        self.disconnect();
                        return;
                    }
                }
            }
        }
        if let Some(found) = view::answer(&mut self.view.asked, &self.answered) {
            self.view.link.pick(found.map(|found| found.id));
            self.took_pick = None;
        }
    }

    /// Shows the game's latest frame, if one came since: what it was drawn from is what the handles
    /// are hit against, while it projects, and the editor's camera adopts it while the game's
    /// drives the view; and the look it drew with may have changed since the editor last read it.
    fn take_frame(&mut self, cx: &mut Context) {
        let Some(reader) = &mut self.reader else {
            return;
        };
        let Some(frame) = reader.take() else {
            return;
        };
        let copying = Instant::now();
        cx.show_pixels(frame.size, |pixels| reader.read(pixels));
        self.measures
            .add(frame.started, copying.elapsed(), frame.size);
        let seen = engine_viewpoint(&frame.viewpoint);
        self.seen = seen.projection.projects().then_some(seen);
        self.size = Some(frame.size);
        if self.view.toolbar.world_camera
            && let Some(seen) = self.seen
        {
            self.view.camera.adopt(&seen);
        }
        self.looking.drew(frame.look);
    }

    /// Where the view shows the game's last frame, in the main window's pixels, with the view at
    /// `view`: from its top left, as large as the frame. What the editor's camera and the handles
    /// read as the view.
    fn screen(&self, view: Option<Viewport>) -> Option<Viewport> {
        let (view, size) = view.zip(self.size)?;
        Some(Viewport {
            origin: view.origin,
            size,
        })
    }

    /// Tells the look which of its parts the user changed since the last update, as what the parts
    /// the editor wants go by changed since, to `now`: the viewpoint, as the editor's camera moved
    /// while it drives the view, by its input, an ease or flying, or the view turned, to the
    /// editor's camera or back to the game's; the outline, as the pick changed, but to what the
    /// Inspector made of the pick the look's outline asked for, its entity, or none for an ID a
    /// read of the game has found gone; the grid, as the view turned, or the toolbar's grid or its
    /// plane changed; and the colliders, as their switch did.
    fn changes(&mut self, now: Wants) {
        let was = self.wants;
        let turned = now.through != was.through;
        if turned || (!now.through && now.viewpoint != was.viewpoint) {
            self.looking.change(Part::Viewpoint);
        }
        // A pick of the user's in the Inspector's pass that made the outline's is theirs; an
        // unpick of theirs there passes for the outline's.
        let took = self.took_pick.take();
        let taken = took.is_some_and(|asked| now.picked == asked || now.picked.is_none());
        if now.picked != was.picked && !taken {
            self.looking.change(Part::Outline);
        }
        if turned || now.grid != was.grid || now.plane != was.plane {
            self.looking.change(Part::Grid);
        }
        if now.colliders != was.colliders {
            self.looking.change(Part::Colliders);
        }
    }

    /// What the editor wants of the game's look, with the window at `scale` pixels a point: the
    /// editor's camera's viewpoint while it drives the view; the Inspector's pick outlined; the
    /// toolbar's grid, while it's on and the editor's camera drives; and the colliders' outlines
    /// while their switch is on, or else whether the game shows them.
    fn wanted(&self, scale: f32) -> Parts {
        let ViewTools {
            camera,
            toolbar,
            link,
            ..
        } = &self.view;
        let driving = !toolbar.world_camera;
        let outline = |id: EntityId| gg_api::Outline {
            id: id.to_string(),
            color: OUTLINE.to_array(),
            width: OUTLINE_WIDTH * scale,
        };
        Parts {
            viewpoint: driving.then(|| api_viewpoint(&camera.viewpoint())),
            outline: link.picked().map(outline),
            grid: (driving && toolbar.grid).then(|| api_plane(toolbar.plane)),
            colliders: toolbar.colliders.then_some(true),
        }
    }

    /// Does what `event` does, which the view got, as the editor reads the window, `reading`, with
    /// the view at `view`: what the view's tools don't take goes to the game while its camera drives
    /// the view, in the view's pixels, but what the editor's camera takes, which turns the view to
    /// it, and nothing does through the editor's camera; and a press off the handles asks the game
    /// for a pick. `Break` for what the tools took.
    fn route(
        &mut self,
        event: &Event<'_>,
        reading: &Reading,
        view: Option<Viewport>,
    ) -> ControlFlow<()> {
        let origin = view.map_or([0, 0], |view| view.origin);
        let through = self.view.toolbar.world_camera;
        let handled = self.view.take(event, reading, picked(&self.view.link));
        if through
            && handled.is_continue()
            && self.talker.is_some()
            && let Some(input) = forwarded(event, origin)
        {
            self.input.push(input);
        }
        if let Some(at) = self.view.picking.take() {
            self.view.asked = self.ask_pick(at, origin);
        }
        self.tell(reading.cursor, view);
        handled
    }

    /// Asks the game for the entity its view shows at `at`, in the main window's pixels, with the
    /// view's top left at `origin`; returns its ticket, while a game serves.
    fn ask_pick(&mut self, at: [f64; 2], origin: [u32; 2]) -> Option<u64> {
        let talker = self.talker.as_ref()?;
        let ticket = self.tickets;
        self.tickets += 1;
        talker.pick(ticket, in_view(at, origin));
        Some(ticket)
    }

    /// Tells the game the view turned, if it did since it was last told: to the editor's camera, the
    /// game loses the focus, letting go of what's held, as the input stops coming; back to the
    /// game's, it gets the cursor, at `cursor`, in the main window's pixels, if it's over the view,
    /// at `view`.
    fn tell(&mut self, cursor: Option<[f64; 2]>, view: Option<Viewport>) {
        let through = self.view.toolbar.world_camera;
        if through == mem::replace(&mut self.told, through) || self.talker.is_none() {
            return;
        }
        if !through {
            self.input.push(InputEvent::Focused(false));
            return;
        }
        if let (Some(at), Some(view)) = (cursor, view) {
            let [x, y] = in_view(at, view.origin);
            let [width, height] = view.size.map(f64::from);
            if (0.0..width).contains(&x) && (0.0..height).contains(&y) {
                self.input.push(InputEvent::Cursor([x, y]));
            }
        }
    }
}

/// What the parts of the look the editor wants go by, as `view`, the view's tools, has them.
fn wants<T, F>(view: &ViewTools<T, F>) -> Wants {
    Wants {
        through: view.toolbar.world_camera,
        viewpoint: view.camera.viewpoint(),
        picked: view.link.picked(),
        grid: view.toolbar.grid,
        plane: view.toolbar.plane,
        colliders: view.toolbar.colliders,
    }
}

/// What the Inspector has picked, as the handles see it, as `link`, its link, places it in the
/// game's world.
fn picked(link: &InspectorLink) -> Option<Picked> {
    let placed = link.placed()?;
    Some(Picked {
        entity: placed.id,
        serial: placed.generation,
        local: placed.local,
        parent: placed.space,
        stepped: link.stepped(),
        waiting: link.waiting(),
    })
}

/// `at`, in the main window's pixels, in the game's view's, whose top left is at `origin`.
fn in_view(at: [f64; 2], origin: [u32; 2]) -> [f64; 2] {
    [at[0] - f64::from(origin[0]), at[1] - f64::from(origin[1])]
}

/// `viewpoint` as the tool API gives one.
fn api_viewpoint(viewpoint: &Viewpoint) -> gg_api::Viewpoint {
    let projection = match viewpoint.projection {
        Projection::Orthographic { height, near, far } => {
            gg_api::Projection::Orthographic { height, near, far }
        }
        Projection::Perspective { fov, near } => gg_api::Projection::Perspective { fov, near },
    };
    gg_api::Viewpoint {
        position: viewpoint.position.to_array(),
        rotation: viewpoint.rotation.to_array(),
        projection,
    }
}

/// The viewpoint the tool API's `given` is.
fn engine_viewpoint(given: &gg_api::Viewpoint) -> Viewpoint {
    let projection = match given.projection {
        gg_api::Projection::Orthographic { height, near, far } => {
            Projection::Orthographic { height, near, far }
        }
        gg_api::Projection::Perspective { fov, near } => Projection::Perspective { fov, near },
    };
    Viewpoint {
        position: Vec3::from_array(given.position),
        rotation: Quat::from_array(given.rotation),
        projection,
    }
}

/// `plane` as the tool API names it.
fn api_plane(plane: Plane) -> gg_api::Plane {
    match plane {
        Plane::XY => gg_api::Plane::Xy,
        Plane::XZ => gg_api::Plane::Xz,
        Plane::YZ => gg_api::Plane::Yz,
    }
}

/// The plane the tool API's `given` names.
fn engine_plane(given: gg_api::Plane) -> Plane {
    match given {
        gg_api::Plane::Xy => Plane::XY,
        gg_api::Plane::Xz => Plane::XZ,
        gg_api::Plane::Yz => Plane::YZ,
    }
}

impl Layer for Game {
    fn attach(&mut self, _: &mut Context) {
        self.start();
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        // Before `gg run`'s lines, which may say the game has gone, letting go of what it logged.
        self.take_logged();
        self.take_lines();
        // What the user changed since the last update, before what the game answered changes it,
        // and before the camera settles, which changes nothing the view shows.
        self.view.camera.update(dt);
        self.changes(wants(&self.view));
        self.view.settle();
        self.take_answers();
        self.take_picks();
        if let Some(view) = cx.view() {
            self.fit(view.size);
        }
        self.take_frame(cx);
        let driving = !self.view.toolbar.world_camera;
        self.view.shown = self.seen.filter(|_| driving);
        let picked = picked(&self.view.link);
        self.view.hold(picked.as_ref());
        self.tell(cx.input().cursor(), cx.view());
        self.wants = wants(&self.view);
        if let Some(talker) = &self.talker {
            let wanted = self.wanted(cx.scale() as f32);
            if let Some(look) = self.looking.ask(&wanted) {
                talker.ask("look", json!(look));
            }
        }
        if let Some(talker) = &self.talker
            && !self.input.is_empty()
            && !self.inputting
        {
            let events = mem::take(&mut self.input);
            talker.ask("input", json!({ "events": events }));
            self.inputting = true;
        }
    }

    /// Has the view's tools see what the view gets, the game too while its camera drives the view,
    /// as [`route`](Game::route) says.
    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        let reading = view::reading(cx, self.screen(cx.view()));
        self.route(event, &reading, cx.view())
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
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
                            Level::Debug => text.weak(),
                            Level::Info => text,
                            Level::Warn => text.color(Color32::from_rgb(230, 190, 60)),
                            Level::Error => text.color(Color32::from_rgb(240, 90, 80)),
                        };
                        ui.label(text);
                    }
                });
        });
        // What the game's view shows under the cursor, while the cursor's over it: a pick at the
        // cursor as each comes back, and what the latest answer found, painted over the handles.
        let origin = cx.view().map_or([0, 0], |view| view.origin);
        let over = ui.view().filter(|view| view.hovered());
        let at = over.and(cx.input().cursor());
        self.view.hover.see(at.is_some(), &self.answered);
        if let Some(at) = at
            && self.hovering.is_none()
        {
            self.hovering = self.ask_pick(at, origin);
            self.view.hover.ask(self.hovering);
        }
        let name = self.view.hover.under().map(|found| found.name);
        // Where the frame shows as this frame renders the view.
        let pixels = (ui.viewport().zip(self.size)).map(|(view, size)| Viewport {
            origin: view.origin,
            size,
        });
        let scale = cx.scale() as f32;
        let sight = (self.view.shown.zip(pixels))
            .and_then(|(viewpoint, pixels)| Sight::new(viewpoint, pixels, scale));
        let picked = picked(&self.view.link);
        let alt = view::alt_down(cx.input());
        self.view.paint(ui, sight, alt, picked.as_ref(), name);
        self.view.overlay(ui, true, self.screen(cx.view()));
        // As the toolbar's switch went.
        self.tell(cx.input().cursor(), cx.view());
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

/// The fields of an event of the game's JSON log that aren't shown among its other fields: where
/// and when it was logged, and those shown before them.
const PLACES: [&str; 9] = [
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

/// The Logs panel's line for `event`, an event of the game's JSON log, as the console shows it but
/// for where and when it was logged: `[code] message  name=value ...`, a string among the fields
/// quoted if it's empty or has spaces; at its level, `Debug` for a debug or trace event.
fn logged(event: &Value) -> Logged {
    let level = match event["level"].as_str() {
        Some("ERROR") => Level::Error,
        Some("WARN") => Level::Warn,
        Some("INFO") => Level::Info,
        _ => Level::Debug,
    };
    let mut text = match &event["code"] {
        Value::Null => String::new(),
        Value::String(code) => format!("[{code}] "),
        code => format!("[{code}] "),
    };
    text.push_str(event["message"].as_str().unwrap_or_default());
    let mut separator = "  ";
    for (name, value) in event.as_object().into_iter().flatten() {
        // `log.*` fields carry the metadata of events from the `log` crate.
        if PLACES.contains(&name.as_str()) || name.starts_with("log.") {
            continue;
        }
        let _ = match value {
            Value::String(value) if !value.is_empty() && !value.contains(char::is_whitespace) => {
                write!(text, "{separator}{name}={value}")
            }
            value => write!(text, "{separator}{name}={value}"),
        };
        separator = " ";
    }
    Logged { level, text }
}

/// Follows the log of the game `session` names on a connection of its own, as the editor, sending
/// `send` each `logs`'s events, from the oldest the game keeps, as each comes, until the game ends
/// or the editor lets go of it, `send`'s receiver dropping: each `logs` sends, events or none.
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
        Event::CursorMoved { x, y } => InputEvent::Cursor(in_view([x, y], origin)),
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

/// Asks the game `session` names, on a connection of its own, as the editor, for the entity its
/// view shows at each pixel `asks` gives, with its ticket, in turn, sending `send` each answer with
/// the ticket, or why there's none, until the editor lets go of the game, `asks`' sender dropping.
fn pick(
    session: &Session,
    asks: Receiver<(u64, [f64; 2])>,
    send: &Sender<(u64, Result<gg_api::Picked, Failure>)>,
) {
    let mut connected = Client::connect(session, CLIENT, CONNECT)
        .and_then(|mut client| client.set_timeout(Some(ANSWER)).map(|()| client));
    for (ticket, at) in asks {
        let answer = match &mut connected {
            Ok(client) => client.call_as("pick", gg_api::Pick { at }),
            Err(failure) => Err(failure.clone()),
        };
        if send.send((ticket, answer)).is_err() {
            return;
        }
    }
}

/// Connections to the game, each on a thread of its own, so that no frame waits for the game's
/// answer: one that requests go to in order, their answers coming back in order; one that follows
/// the game's log, as [`follow`] does, whose events come back as they're read, each read's
/// together; and one that picks go to, as [`pick`] does, each of which waits for a frame to read
/// it, their answers coming back in order, with their tickets.
struct Talker {
    asks: Sender<(String, Value)>,
    answers: Receiver<(String, Result<Value, Failure>)>,
    logged: Receiver<Vec<Value>>,
    picks: Sender<(u64, [f64; 2])>,
    picked: Receiver<(u64, Result<gg_api::Picked, Failure>)>,
}

impl Talker {
    /// Connects to the game `session` names, as the editor, the author of the edits it applies.
    fn connect(session: Session) -> Self {
        let (asks, asked) = mpsc::channel::<(String, Value)>();
        let (answer, answers) = mpsc::channel();
        let (log, logged) = mpsc::channel();
        let (picks, picking) = mpsc::channel();
        let (found, picked) = mpsc::channel();
        let followed = session.clone();
        thread::spawn(move || follow(&followed, &log));
        let picker = session.clone();
        thread::spawn(move || pick(&picker, picking, &found));
        thread::spawn(move || {
            let connected = Client::connect(&session, CLIENT, CONNECT)
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
        Self {
            asks,
            answers,
            logged,
            picks,
            picked,
        }
    }

    /// Sends request `method` with `params`, whose answer comes back in `answers`.
    fn ask(&self, method: &str, params: Value) {
        let _ = self.asks.send((method.to_owned(), params));
    }

    /// Asks for the entity the game's view shows at `at`, in its pixels from its top left, whose
    /// answer comes back in `picked` with `ticket`.
    fn pick(&self, ticket: u64, at: [f64; 2]) {
        let _ = self.picks.send((ticket, at));
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
    use gg_api::view::Writer;
    use gg_api::{By, Look, Outline};
    use gg_engine::Key;
    use gg_engine::math::Vec2;

    use super::*;
    use crate::camera::Tool;
    use crate::tests::{VIEW, reading};

    /// The game's ends of the connections of a layer that [`serving`] gives: the requests the layer
    /// sends, where their answers go back, and the picks it asks for.
    struct Ends {
        asked: Receiver<(String, Value)>,
        answer: Sender<(String, Result<Value, Failure>)>,
        picks: Receiver<(u64, [f64; 2])>,
    }

    /// The layer with a game serving, and the game's ends of its connections.
    fn serving() -> (Game, Ends) {
        let mut game = Game::new(PathBuf::from("."), Inspector::remote(CLIENT).link());
        let (asks, asked) = mpsc::channel();
        let (answer, answers) = mpsc::channel();
        let (_, logged) = mpsc::channel();
        let (picks, picking) = mpsc::channel();
        let (_, picked) = mpsc::channel();
        game.talker = Some(Talker {
            asks,
            answers,
            logged,
            picks,
            picked,
        });
        let ends = Ends {
            asked,
            answer,
            picks: picking,
        };
        (game, ends)
    }

    /// The looks the layer whose ends are `ends` asked the game for since.
    fn looks(ends: &Ends) -> Vec<Look> {
        let asked = ends.asked.try_iter().filter(|(method, _)| method == "look");
        let looks = asked.map(|(_, look)| serde_json::from_value(look).expect("a look"));
        looks.collect()
    }

    /// Through the game's camera, what the view gets goes to the game, in the view's pixels, and a
    /// left press off the handles asks for a pick too; but what the editor's camera takes doesn't,
    /// which turns the view to it, the game losing the focus. Through the editor's camera, nothing
    /// goes, and Q, W, E and R pick tools; turning back to the game's camera gives the game the
    /// cursor, if it's over the view.
    #[test]
    fn the_views_input_goes_to_the_game_through_its_camera_alone() {
        let (mut game, ends) = serving();
        // [`VIEW`]'s top left is at (100, 50).
        let (at, in_game) = ([300.0, 200.0], [200.0, 150.0]);
        let moved = Event::CursorMoved { x: at[0], y: at[1] };
        let w = Event::Key {
            key: Key::KeyW,
            pressed: true,
            repeat: false,
        };
        let press = |button, pressed| Event::MouseButton { button, pressed };
        let left = [
            press(MouseButton::Left, true),
            press(MouseButton::Left, false),
        ];
        for event in [moved, w, left[0], left[1]] {
            let routed = game.route(&event, &reading(at), Some(VIEW));
            assert_eq!(routed, ControlFlow::Continue(()), "{event:?}");
        }
        let button = |pressed| InputEvent::Button {
            button: gg_api::Button::Left,
            pressed,
        };
        let w_down = InputEvent::Key {
            key: "KeyW".to_owned(),
            pressed: true,
            repeat: false,
        };
        let expected = [
            InputEvent::Cursor(in_game),
            w_down,
            button(true),
            button(false),
        ];
        assert_eq!(game.input, expected);
        assert_eq!(ends.picks.try_iter().collect::<Vec<_>>(), [(0, in_game)]);
        assert_eq!(game.view.asked, Some(0));
        assert_eq!(game.view.toolbar.tool, Tool::Select, "the game's W");
        // What the editor's camera takes: a press of its buttons, Alt with the left, the left
        // under Pan or Orbit, the wheel and Home.
        let home = Event::Key {
            key: Key::Home,
            pressed: true,
            repeat: false,
        };
        let wheel = Event::Scroll(Scroll::Lines { x: 0.0, y: 1.0 });
        let mut alt = reading(at);
        alt.screen.alt = true;
        for (event, read, tool) in [
            (press(MouseButton::Middle, true), reading(at), Tool::Select),
            (press(MouseButton::Right, true), reading(at), Tool::Select),
            (left[0], alt, Tool::Select),
            (left[0], reading(at), Tool::Pan),
            (left[0], reading(at), Tool::Orbit),
            (wheel, reading(at), Tool::Select),
            (home, reading(at), Tool::Select),
        ] {
            let (mut game, _) = serving();
            game.view.toolbar.tool = tool;
            let routed = game.route(&event, &read, Some(VIEW));
            assert_eq!(routed, ControlFlow::Break(()), "{event:?} with {tool:?}");
            assert!(!game.view.toolbar.world_camera, "{event:?} with {tool:?}");
            assert_eq!(game.input, [InputEvent::Focused(false)], "{event:?}");
        }
        // Through the editor's camera, nothing goes, but a press off the handles asks for a pick.
        game.view.toolbar.world_camera = false;
        game.tell(Some(at), Some(VIEW));
        game.input.clear();
        for event in [moved, w, left[0], left[1], wheel] {
            let _ = game.route(&event, &reading(at), Some(VIEW));
        }
        assert_eq!(game.input, []);
        assert_eq!(game.view.toolbar.tool, Tool::Move, "the editor's W");
        assert_eq!(ends.picks.try_iter().collect::<Vec<_>>(), [(1, in_game)]);
        // Back to the game's camera, with the toolbar's switch, for which a test sets the field
        // and tells the game, since it can't run `ui`: the cursor over the view, and off it.
        for (cursor, told) in [
            (at, vec![InputEvent::Cursor(in_game)]),
            ([20.0, 20.0], vec![]),
        ] {
            game.view.toolbar.world_camera = false;
            game.tell(Some(cursor), Some(VIEW));
            game.input.clear();
            game.view.toolbar.world_camera = true;
            game.tell(Some(cursor), Some(VIEW));
            assert_eq!(game.input, told, "at {cursor:?}");
        }
    }

    /// A part of the look goes to the game only as the user changes it: not as the editor's camera
    /// adopts the frames' viewpoints through the game's camera, nor as it takes another client's
    /// viewpoint while it drives, nor as it settles after; nor as the Inspector makes the pick
    /// another client's outline asks for. A wheel turns the view to the editor's camera, from where
    /// the frames left it, whose viewpoint goes, with the grid; the switch back to the game's camera
    /// gives both back; and a pick of the user's goes as the outline, though the Inspector makes it
    /// in the pass that makes the outline's.
    #[test]
    fn the_look_goes_to_the_game_as_the_user_changes_it() {
        let (mut game, ends) = serving();
        let reader = Reader::create(4, 4).expect("memory");
        let mut writer = Writer::open(reader.name()).expect("opened");
        game.reader = Some(reader);
        // A viewpoint at `position`, turned as the editor's camera starts, 4 units tall: its pivot
        // off the grid's plane, onto which the camera settles it.
        let orthographic = |position| gg_api::Viewpoint {
            position,
            rotation: [0.0, 0.0, 0.0, 1.0],
            projection: gg_api::Projection::Orthographic {
                height: 4.0,
                near: -40.0,
                far: 40.0,
            },
        };
        let mut frame = |viewpoint, look| {
            let published = writer.publish([4, 4], 0, SystemTime::now(), viewpoint, look, |_| {});
            published.expect("published");
        };
        let mut cx = Context::seeded(1);
        let mut update = |game: &mut Game| game.update(&mut cx, Duration::ZERO);
        // The game answers a look with `parts`, each set by the client `by` names, at `revision`.
        let answer = |parts: Parts, by: [Option<&str>; 3], revision| {
            let [viewpoint, outline, grid] = by.map(|by| by.map(str::to_owned));
            let looked = Looked {
                viewpoint: parts.viewpoint,
                outline: parts.outline,
                grid: parts.grid,
                colliders: None,
                by: By {
                    viewpoint,
                    outline,
                    grid,
                    colliders: None,
                },
                revision,
                run: 1,
                seen: None,
            };
            let answer = ("look".to_owned(), Ok(json!(looked)));
            ends.answer.send(answer).expect("sent");
        };
        for _ in 0..2 {
            frame(orthographic([1.0, 2.0, 5.0]), 0);
            update(&mut game);
        }
        assert_eq!(looks(&ends), [], "the frames' viewpoints");
        let wheel = Event::Scroll(Scroll::Lines { x: 0.0, y: 1.0 });
        let _ = game.route(&wheel, &reading([300.0, 200.0]), Some(VIEW));
        update(&mut game);
        let seen = game.view.camera.viewpoint();
        assert_eq!(
            seen.position.truncate(),
            Vec2::new(1.0, 2.0),
            "from the frames'"
        );
        let mine = Parts {
            viewpoint: Some(api_viewpoint(&seen)),
            grid: Some(gg_api::Plane::Xy),
            ..Parts::default()
        };
        let sent = Look {
            viewpoint: Some(mine.viewpoint),
            grid: Some(mine.grid),
            ..Look::default()
        };
        assert_eq!(looks(&ends), [sent]);
        // An agent sets the viewpoint and an outline, which a frame draws with: the editor reads
        // the look, and takes them.
        answer(mine.clone(), [Some(CLIENT), None, Some(CLIENT)], 1);
        let agents = orthographic([-3.0, 1.0, 4.0]);
        frame(agents, 2);
        update(&mut game);
        assert_eq!(looks(&ends), [Look::default()], "a read");
        let x: EntityId = "0123456789abcdef".parse().expect("an ID");
        let outline = Outline {
            id: x.to_string(),
            color: [0.0, 1.0, 0.0, 1.0],
            width: 3.0,
        };
        let theirs = Parts {
            viewpoint: Some(agents),
            outline: Some(outline.clone()),
            ..mine
        };
        answer(theirs, [Some("agent"), Some("agent"), Some(CLIENT)], 2);
        for _ in 0..3 {
            update(&mut game);
        }
        let seen = game.view.camera.viewpoint();
        assert_eq!(seen.position.truncate(), Vec2::new(-3.0, 1.0), "taken");
        assert_eq!(looks(&ends), [], "taken, and settled");
        // Back to the game's camera, with the toolbar's switch, for which a test sets the field.
        game.view.toolbar.world_camera = true;
        update(&mut game);
        let back = Look {
            viewpoint: Some(None),
            grid: Some(None),
            ..Look::default()
        };
        assert_eq!(looks(&ends), [back]);
        let outlined = Parts {
            outline: Some(outline.clone()),
            ..Parts::default()
        };
        answer(outlined, [None, Some("agent"), None], 3);
        update(&mut game);
        // The Inspector's pass after the agent's outline is taken, another entity picked before
        // it: it picks the outline's, or unpicks, for an ID a read of the game has found gone,
        // neither the user's doing; but a pick of the user's in the same pass is theirs, whose
        // outline goes.
        let [before, users]: [EntityId; 2] =
            ["fedcba9876543210", "00000000000000ff"].map(|id| id.parse().expect("an ID"));
        let was = Wants {
            picked: Some(before),
            ..game.wants
        };
        game.wants = was;
        for (picked, sends) in [(Some(x), false), (None, false), (Some(users), true)] {
            game.take_look(Look {
                outline: Some(Some(outline.clone())),
                ..Look::default()
            });
            game.changes(Wants { picked, ..was });
            let wanted = Parts {
                outline: picked.map(|id| Outline {
                    id: id.to_string(),
                    color: OUTLINE.to_array(),
                    width: OUTLINE_WIDTH,
                }),
                ..Parts::default()
            };
            let sent = sends.then(|| Look {
                outline: Some(wanted.outline.clone()),
                ..Look::default()
            });
            assert_eq!(game.looking.ask(&wanted), sent, "{picked:?}");
        }
    }

    /// The toolbar's switch to the editor's camera sends its viewpoint, as it adopted the frames',
    /// with the grid, though the camera hasn't moved: from then on, the editor's camera drives the
    /// view.
    #[test]
    fn the_switch_to_the_editors_camera_sends_its_viewpoint() {
        let (mut game, ends) = serving();
        let reader = Reader::create(4, 4).expect("memory");
        let mut writer = Writer::open(reader.name()).expect("opened");
        game.reader = Some(reader);
        let viewpoint = gg_api::Viewpoint {
            position: [1.0, 2.0, 5.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            projection: gg_api::Projection::Orthographic {
                height: 4.0,
                near: -40.0,
                far: 40.0,
            },
        };
        let published = writer.publish([4, 4], 0, SystemTime::now(), viewpoint, 0, |_| {});
        published.expect("published");
        let mut cx = Context::seeded(1);
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(looks(&ends), [], "the frame's viewpoint");
        // The switch, for which a test sets the field, since it can't run `ui`.
        game.view.toolbar.world_camera = false;
        game.update(&mut cx, Duration::ZERO);
        let seen = game.view.camera.viewpoint();
        assert_eq!(seen.position.truncate(), Vec2::new(1.0, 2.0), "the frame's");
        let sent = Look {
            viewpoint: Some(Some(api_viewpoint(&seen))),
            grid: Some(Some(gg_api::Plane::Xy)),
            ..Look::default()
        };
        assert_eq!(looks(&ends), [sent]);
    }

    /// Another client's viewpoint, which turns the view to the editor's camera, sends the
    /// toolbar's grid, as the user's turn does; giving it back, which turns the view back to the
    /// game's, gives the grid back; but a grid another client set stays as the view turns.
    #[test]
    fn the_grid_follows_a_turn_another_clients_viewpoint_makes() {
        let (mut game, ends) = serving();
        let mut cx = Context::seeded(1);
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(looks(&ends), [], "all the game's own");
        let agents = gg_api::Viewpoint {
            position: [-3.0, 1.0, 4.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            projection: gg_api::Projection::Orthographic {
                height: 4.0,
                near: -40.0,
                far: 40.0,
            },
        };
        // The game answers a look of `viewpoint` and `grid`, each set by the client `by` names, at
        // `revision`; returns the looks the editor asks for in the update that takes it.
        let mut answer = |viewpoint, grid, by: [Option<&str>; 2], revision| {
            let [viewpoint_by, grid_by] = by.map(|by| by.map(str::to_owned));
            let looked = Looked {
                viewpoint,
                outline: None,
                grid,
                colliders: None,
                by: By {
                    viewpoint: viewpoint_by,
                    outline: None,
                    grid: grid_by,
                    colliders: None,
                },
                revision,
                run: 1,
                seen: None,
            };
            let answer = ("look".to_owned(), Ok(json!(looked)));
            ends.answer.send(answer).expect("sent");
            game.update(&mut cx, Duration::ZERO);
            looks(&ends)
        };
        let grid = |grid| Look {
            grid: Some(grid),
            ..Look::default()
        };
        let (xy, xz) = (gg_api::Plane::Xy, gg_api::Plane::Xz);
        let agent = Some("agent");
        let sent = answer(Some(agents), None, [agent, None], 1);
        assert_eq!(sent, [grid(Some(xy))], "the toolbar's");
        let sent = answer(Some(agents), Some(xy), [agent, Some(CLIENT)], 2);
        assert_eq!(sent, []);
        let sent = answer(None, Some(xy), [None, Some(CLIENT)], 3);
        assert_eq!(sent, [grid(None)], "given back");
        assert_eq!(answer(None, None, [None, None], 4), []);
        let sent = answer(Some(agents), Some(xz), [agent, agent], 5);
        assert_eq!(sent, [], "the agent's grid");
        let sent = answer(None, Some(xz), [None, agent], 6);
        assert_eq!(sent, [], "the agent's grid stays");
    }

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
        let mut game = Game::new(PathBuf::from("."), Inspector::remote(CLIENT).link());
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

    /// The game's events reach the Logs as they're read, each with its code, its message and its
    /// fields, but where and when it was logged, at its level.
    #[test]
    fn the_games_events_reach_the_logs_with_their_codes_and_fields() {
        let mut game = Game::new(PathBuf::from("."), Inspector::remote(CLIENT).link());
        let (asks, _) = mpsc::channel();
        let (_, answers) = mpsc::channel();
        let (log, logged) = mpsc::channel();
        let (picks, _) = mpsc::channel();
        let (_, picked) = mpsc::channel();
        game.talker = Some(Talker {
            asks,
            answers,
            logged,
            picks,
            picked,
        });
        let warned = r#"{"timestamp":"2026-10-30T12:00:00.000000Z","level":"WARN",
            "code":"asset.missing","path":"assets/hero sprite.png","tries":2,"empty":"",
            "message":"Can't load the texture","target":"shards",
            "filename":"games/shards/src/main.rs","line_number":40,"threadName":"main"}"#;
        let traced = r#"{"timestamp":"2026-10-30T12:00:00.000001Z","level":"TRACE",
            "message":"A tick","target":"gg_engine::main_loop","spans":[{"name":"tick"}],
            "threadName":"main"}"#;
        let events = [warned, traced].map(|line| serde_json::from_str(line).expect("JSON"));
        log.send(events.to_vec()).expect("sent");
        log.send(Vec::new()).expect("sent");
        game.take_logged();
        let logs: Vec<_> = (game.logs.iter())
            .map(|logged| (logged.level, logged.text.as_str()))
            .collect();
        let expected = [
            (
                Level::Warn,
                r#"[asset.missing] Can't load the texture  empty="" path="assets/hero sprite.png" tries=2"#,
            ),
            (Level::Debug, "A tick"),
        ];
        assert_eq!(logs, expected);
    }
}
