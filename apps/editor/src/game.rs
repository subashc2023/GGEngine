//! The game of the project the editor is started in, run as an agent runs it: under `cargo gg run
//! --embedded` in the project's directory, which builds it, patches it as its code changes and
//! restarts it after any other change, and which ends, with the game, as the editor ends, closing
//! its stdin. The game starts stopped, as `--stopped` has it: its world is the world as edited,
//! which only edits, scene files' saves and the File menu's New and Open change, until Play. It
//! renders offscreen for the editor, which shows its view in the Viewport from memory the two
//! share, as the tool API's `view` asks. The Entities and Inspector panels show the game's world,
//! and edit it, through the API, as a remote [`Inspector`] does; the Logs panel shows `gg run`'s
//! events, the build's errors among them, the game's events as it logs them, which the tool API's
//! `logs` follows, and what the game and the build print; the Assets panel, the files in the
//! project's asset root; the [Debugger](crate::debugger) panel, its scripts; and the title bar, how
//! the run goes, or while a game serves, whether it's stopped, playing or simulating, and whether
//! it's paused, as the game answers the editor's requests and the `status` it asks for every
//! quarter second, which catches another client's, or that it waits at a script's stop, as the
//! Debugger panel learns; with Play, Simulate, pause and step.
//!
//! Started outside a project, the editor shows the [`hub`] over its panels, where a
//! project is picked, unless it's to open the latest as it starts; and the File menu's Open Project
//! shows it over the project open, which its Back goes back to. A project picked there opens in
//! place of the one open as a close of the editor goes, asking whether to save the open scene's
//! unsaved edits first: the run of the one open ends, going on ending as the new project's starts,
//! but in the same project, which waits for it, and the panels show the new project's.
//!
//! Play has a copy of the world as edited play in its place, as the API's `play` does, the view
//! seeing through the game's own camera, which gets all the input the view gets, the view's tools
//! only what lets go of what they hold; Simulate plays it the same, through the editor's camera,
//! the game getting no input. Play from Simulate, or Simulate from Play, carries on, moving the
//! camera and the input over, the game letting go of what's held as the input leaves it, as the
//! toolbar's camera switch does while the game plays. Stop, lit in place of either, stops the
//! input at once, and brings back the world as edited, as `stop` does, with its edits' history,
//! the view going to the editor's camera, as it does at any client's `stop` that no Play clicked
//! since follows. Stopped, the camera switch has the view show what the game's camera sees, as it
//! does at first and as a New or an Open opens a scene, the input staying the editor's, until the
//! editor's camera takes some, which turns the view to it.
//!
//! A game that asks for the cursor grabbed, for looking about with the mouse, as the view's memory
//! says it does, has the editor grab its own window's in Play while the view has the keys, the
//! mouse's raw motion going to the game; Escape lets go of it, whatever the game does with the key,
//! which it gets, as a game may not let go itself, and a press in the view grabs it again;
//! Simulate and Stop let go.
//!
//! Through the editor's camera, the handles show. A click in the view, but in Play, picks what the
//! game's view shows there, as the API's `pick` finds it, which the panels show picked; the
//! handles move, turn and scale it through the Inspector's link, each drag one edit of the
//! editor's, which its Edit menu undoes; and the view's bottom left names what's under the cursor.
//! The editor's camera, the outline round what's picked, the grid and the colliders' switch reach
//! the game as parts of its look, as [`look`](crate::look) says: each as the user changes it, the
//! editor taking the parts another client, such as an agent, sets.
//!
//! A texture, a model, a font or a prefab dragged from the Assets panel outlines the view while it's
//! held over it, and is let go there as an edit of the editor's, through the Inspector's link,
//! which names it by its asset ID: a texture sets the texture of the sprite the game's view shows
//! there, as the API's `pick` finds it, and a font the font of the text there, which the panels
//! show picked, and whose refusal, as of an entity without a sprite or a text, the Inspector shows;
//! a model is a root, named after its file, where the cursor shows the grid's plane, as
//! [`landing`](Game::landing) says, and a prefab an instance's root there, linked, which the game
//! spawns the prefab's entities under; each of which the panels show picked.
//!
//! The File menu acts on the game's open scene through the API's `scene`, never reading or writing
//! a scene file itself: New empties it, Open opens a scene the OS's dialog picks in its place, as a
//! click in the [Scenes](crate::scenes) panel opens one, and Save and Save As save the world as
//! edited to its file, or to one the dialog picks, which is then its file; each request, and a
//! close's status, sent once the Inspector has nothing waiting for the game, so that the game has
//! its edits first. The title bar names the open scene's file,
//! marked while the game says it has unsaved edits, which New, Open, Quit and the window's close
//! button ask whether to save first, discard or keep, cancelling; a close asks the game for a
//! status first, while one serves, and otherwise goes by the last. A refusal shows in a message,
//! and in the Logs.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read};
use std::mem;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gg_api::asset::{AssetKind, Meta};
use gg_api::view::Reader;
use gg_api::{Client, Failure, InputEvent, LogEvents, Looked, Scene, SceneAction, Session, Status};
use gg_engine::egui::{
    self, Color32, Response, RichText, ScrollArea, TextStyle, TextWrapMode, ViewportCommand,
    ViewportId,
};
use gg_engine::grid::{self, Plane};
use gg_engine::math::{Quat, Vec2, Vec3};
use gg_engine::reflect::{self, Reflect};
use gg_engine::{
    App, Context, Docked, EntityId, Event, FileDialog, Inspector, InspectorLink, Key, Layer,
    MouseButton, Name, PerfPanel, Projection, Scroll, Transform, Ui, Viewpoint, Viewport,
};
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::assets::{Assets, Dragged};
use crate::debugger::Debugger;
use crate::file::{self, Choice, Item, Message};
use crate::handles::{Picked, Reading, Sight};
use crate::hub::{self, Hub};
use crate::look::{Looking, Part, Parts};
use crate::overlay::{self, Mode, Toolbar};
use crate::scenes::{self, Scenes};
use crate::view::{self, OUTLINE, OUTLINE_WIDTH, ViewTools};

/// The most lines the Logs panel keeps, past which the oldest go.
const LOGGED: usize = 5000;

/// How long the connection to the game waits to connect. It waits for each answer as long as it
/// takes: a script stopped in the game holds the answers for as long as it's debugged, and `gg run`
/// ends a game that hangs, which closes the connection.
const CONNECT: Duration = Duration::from_secs(10);

/// How long each `logs` that follows the game's events waits for them to come, in milliseconds.
const FOLLOWING_MS: u64 = 1_000;

/// How long the connection that follows the game's events waits for each answer: well past the
/// time each `logs` waits.
const FOLLOWING: Duration = Duration::from_secs(30);

/// How long the editor, as it ends, waits for `gg run` to end the game and itself.
const ENDING: Duration = Duration::from_secs(15);

/// How many frames the measures of the view's frames keep, from the last; 68 s at 240 fps.
const MEASURED: usize = 16_384;

/// How often the editor asks the game how it runs, `status`, which another client, such as an
/// agent, may change: the title bar lags such a change by as much.
const STATUS_EVERY: Duration = Duration::from_millis(250);

/// The editor of the project in directory `dir`, if it's started in one, and otherwise of the one
/// the hub picks, the hub finding projects in `workspace`, the engine's, if it runs in one: the
/// game's view in the Viewport, its entities left of it, above its scenes, the one picked right of
/// it, above the Performance panel, and the Logs below it, the Assets and the Debugger among their
/// tabs.
pub(crate) fn app(dir: Option<PathBuf>, workspace: Option<PathBuf>) -> App {
    let app = App::new("Editor").custom_title_bar().view_panel("Viewport");
    let app = app
        .dock("Inspector", Docked::RightOf("Viewport"))
        .dock("Entities", Docked::LeftOf("Viewport"))
        .dock("Scenes", Docked::Below("Entities"))
        .dock("Performance", Docked::Below("Inspector"))
        .dock("Logs", Docked::Below("Viewport"))
        .dock("Assets", Docked::With("Logs"))
        .dock("Debugger", Docked::With("Logs"));
    let inspector = Inspector::remote(CLIENT);
    let mut game = Game::new(dir, inspector.link());
    game.hub = Hub::new(workspace);
    app.layer(game).overlay(PerfPanel).overlay(inspector)
}

/// The client the editor is to the game, the author of its edits.
const CLIENT: &str = "editor";

/// The editor's layer: the `gg run` of the project's game, the connection to the game that serves,
/// the memory its view is shared through, how the game runs, and what the Logs, Assets and Scenes
/// panels and the title bar show; the game's open scene, what the File menu asks of it, and how far
/// a close has got; the view's tools, the picks they ask of the game and the look they keep with
/// it; and the hub, with the runs of the projects it switched from that are ending.
struct Game {
    /// The project's directory, while one's open: the one it was started in, or the hub opened.
    dir: Option<PathBuf>,
    /// `gg run`, until it ends, with its stdin, which the editor closes as it ends.
    run: Option<(Child, ChildStdin)>,
    /// The `gg run`s of the projects open before, each with its project's directory, which go on
    /// ending once their stdins are closed, until they've ended; and whether this project's waits
    /// to start until the one in its directory has, as [`start`](Self::start) says.
    ending: Vec<(PathBuf, Child)>,
    waiting: bool,
    hub: Hub,
    /// The project a close under way opens in place of the one open, rather than closing the
    /// editor, as [`switch_to`](Self::switch_to) says.
    switching: Option<PathBuf>,
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
    /// The texture or font let go over the view whose pick waits for its answer, if one does: the
    /// latest.
    dropping: Option<Dropping>,
    /// The answers to picks that came back since the last update, each with its ticket.
    answered: Vec<(u64, Option<Found>)>,
    /// The game's look, as the editor keeps it.
    looking: Looking,
    /// What the editor's wanted parts of the look went by as the last update left them, and the
    /// pick that update asked the Inspector for as the look's outline, if it asked for one:
    /// `Some(None)` unpicks.
    wants: Wants,
    took_pick: Option<Option<EntityId>>,
    /// The entity the Entities panel asked to frame in the view, while the link doesn't place it
    /// yet, as it doesn't a pick just made until a read of it comes.
    framing: Option<EntityId>,
    /// Whether the view's input went to the game as the game was last told, as
    /// [`through`](Self::through) says: as it stops, the game lets go of what's held, and as it
    /// starts, it gets the cursor.
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
    /// Whether Escape let go of the cursor grabbed for the game, until a press in the view.
    escaped: bool,
    run_state: RunState,
    /// Whether the game's simulation is paused, and whether the game is stopped, as it last
    /// answered: stopped until a game that serves answers, so that nothing goes to it before.
    paused: bool,
    stopped: bool,
    /// Whether the editor's Stop waits for the game's answer, whatever it is: until it comes, the
    /// view's input doesn't go to the game.
    stopping: bool,
    /// The `play`s the editor asked that the game hasn't answered yet: a stop answered meanwhile,
    /// to a request before them, leaves the view where the last click turned it.
    playing: u32,
    /// Whether a `status` waits for its answer, one at a time, and when the last was asked, the
    /// next waiting until [`STATUS_EVERY`] after.
    statusing: bool,
    polled: Option<Instant>,
    logs: VecDeque<Logged>,
    /// The Assets panel, at the project's asset root, the Scenes panel, of its scenes folders, and
    /// the project's folders; none outside a project.
    assets: Option<Assets>,
    scenes: Option<Scenes>,
    folders: Option<Folders>,
    /// The Debugger panel, of the scripts of the game that serves.
    debugger: Debugger,
    /// Whether a game has served since the editor started, from when a close asks whether to save.
    served: bool,
    /// The open scene's file, as the asset root names it, `None` while it's untitled, and whether
    /// it has unsaved edits, as the game last answered: kept while no game serves, as `gg run`
    /// restarts it.
    scene: Option<String>,
    unsaved: bool,
    /// The File menu's `scene` request, until the game answers it.
    request: Option<Request>,
    /// What the prompt asks whether to save the open scene's unsaved edits before, while it shows;
    /// what the save its Save chose goes on to once the game has saved them; and the File menu's
    /// item that the next UI chooses, as Save As for a Save the game refused for an untitled
    /// scene, since its dialog shows from the UI.
    prompt: Option<Action>,
    then: Option<Action>,
    due: Option<Item>,
    closing: Closing,
    /// Why the game, or the editor, refused the File menu's last request, until it's dismissed.
    message: Option<Message>,
    measures: Measures,
}

/// The project's folders, as its manifest names them: its asset root, which the open scene's file
/// is named from, and its scene folders, where the dialogs start without an open scene's file; and
/// its start scene.
struct Folders {
    root: PathBuf,
    scenes: Vec<PathBuf>,
    start: Option<PathBuf>,
}

/// A `scene` request of the File menu's, and whether it's sent: once the Inspector has nothing
/// waiting for the game, as [`InspectorLink::idle`] says, so that the game has its edits first.
struct Request {
    scene: Scene,
    sent: bool,
}

/// What asks whether to save the open scene's unsaved edits first: New and Open, which the game
/// refuses while it has some, unless it's to discard them, and a close of the editor.
#[derive(Clone, Debug, PartialEq)]
enum Action {
    New,
    /// Opens the scene at this path, as the dialog picked it, which a prompt keeps, so that the
    /// dialog doesn't show again.
    Open(PathBuf),
    Close,
}

impl Action {
    /// The `scene` request that does it, discarding the unsaved edits if `discard`: none for a
    /// close.
    fn scene(&self, discard: bool) -> Option<Scene> {
        let (action, path) = match self {
            Self::New => (SceneAction::New, None),
            Self::Open(path) => (SceneAction::Open, Some(path.clone())),
            Self::Close => return None,
        };
        Some(Scene {
            action,
            path,
            discard,
        })
    }
}

/// How far a close of the editor, by Quit or the window's close button, has got, as
/// [`Game::close`] says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Closing {
    /// None asked for, or the last cancelled.
    No,
    /// Vetoed, until the answer to a status asked after it says whether the open scene has unsaved
    /// edits: `true` once that's asked.
    Checking(bool),
    /// Vetoed, until the prompt is answered, or the save its Save chose is done.
    Prompting,
    /// Granted: the UI asks the main window to close, and the close that makes goes ahead.
    Granted,
}

/// A texture or a font let go over the view: the ticket of the pick at where it was let go, which
/// it is, its name, as the game takes it, and its file's name, as the Logs say it.
#[derive(Debug, PartialEq)]
struct Dropping {
    ticket: u64,
    kind: AssetKind,
    name: String,
    file: String,
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

/// How the run goes, as the title bar says, but `Running`, while a game serves, for which it says
/// how the game runs, as [`Game::label`] has it. A build says where it has come to once `gg run`
/// does, as [`progress`] puts it.
#[derive(Clone, Debug, PartialEq)]
enum RunState {
    Building(Option<String>),
    Starting,
    Running,
    Rebuilding(Option<String>),
    Restarting,
    Waiting,
    Ended(String),
}

/// A line of the Logs panel, which shows each of its text's lines, between its newlines, as a row.
struct Logged {
    level: Level,
    text: String,
    /// The row its first line has among those logged since the editor started, so that the panel
    /// finds the rows it shows without laying out the rest, as [`show_logs`] does.
    line: usize,
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
    /// The layer of the project in directory `dir`, or of none, through the game's camera at
    /// first, whose tools pick and edit through `inspector`, the Inspector's link.
    fn new(dir: Option<PathBuf>, inspector: InspectorLink) -> Self {
        let (send, lines) = mpsc::channel();
        let toolbar = Toolbar {
            world_camera: true,
            ..Toolbar::default()
        };
        let view = ViewTools::new(inspector, toolbar);
        let wants = wants(&view);
        let folders = dir.as_deref().map(folders);
        let assets = (folders.as_ref()).map(|folders| Assets::new(folders.root.clone()));
        let scenes = (folders.as_ref()).map(|folders| {
            let (root, scenes) = (folders.root.clone(), folders.scenes.clone());
            Scenes::new(root, scenes, folders.start.as_deref())
        });
        let debugger = Debugger::new(folders.as_ref().map(|folders| folders.root.clone()), CLIENT);
        Self {
            dir,
            run: None,
            ending: Vec::new(),
            waiting: false,
            hub: Hub::default(),
            switching: None,
            lines,
            send: Some(send),
            talker: None,
            view,
            tickets: 0,
            hovering: None,
            dropping: None,
            answered: Vec::new(),
            looking: Looking::new(CLIENT),
            wants,
            took_pick: None,
            framing: None,
            told: false,
            size: None,
            seen: None,
            reader: None,
            asked: None,
            refused: false,
            input: Vec::new(),
            inputting: false,
            escaped: false,
            run_state: RunState::Building(None),
            paused: false,
            stopped: true,
            stopping: false,
            playing: 0,
            statusing: false,
            polled: None,
            logs: VecDeque::new(),
            assets,
            scenes,
            folders,
            debugger,
            served: false,
            scene: None,
            unsaved: false,
            request: None,
            prompt: None,
            then: None,
            due: None,
            closing: Closing::No,
            message: None,
            measures: Measures::default(),
        }
    }

    /// Starts `cargo gg run --embedded -- --stopped` in the project's directory, once, with threads
    /// that read what it prints, saying why if it can't start it; but nothing outside a project,
    /// and while the run of the project open before in the same directory goes on ending, it waits,
    /// each update starting it again, until that has ended.
    fn start(&mut self) {
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
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let started = Command::new(cargo)
            .args(["gg", "run", "--embedded", "--", "--stopped"])
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match started {
            Ok(child) => child,
            Err(error) => {
                let why = format!("can't run `cargo gg run` in {}: {error}", dir.display());
                self.log(Level::Error, why.clone());
                self.run_state = RunState::Ended(why);
                return;
            }
        };
        let started = format!("cargo gg run --embedded -- --stopped, in {}", dir.display());
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

    fn log(&mut self, level: Level, text: String) {
        if self.logs.len() == LOGGED {
            self.logs.pop_front();
        }
        let line = (self.logs.back()).map_or(0, |last| last.line + last.text.split('\n').count());
        self.logs.push_back(Logged { level, text, line });
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

    /// Takes the game's events logged since, each a line of the Logs panel, as [`logged`] makes it,
    /// but `gg run`'s notes of a patch or a rebuild that failed, which its own events show.
    fn take_logged(&mut self) {
        let Some(talker) = &self.talker else {
            return;
        };
        let reads: Vec<Vec<Value>> = talker.logged.try_iter().collect();
        for event in reads.iter().flatten() {
            let code = event["code"].as_str();
            if event["client"] == "gg"
                && matches!(code, Some("gg.patch_failed" | "gg.rebuild_failed"))
            {
                continue;
            }
            let (level, text) = logged(event);
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

    /// Lets go of the connection to the game, which has gone, of the input it didn't get, of the
    /// picks it didn't answer, a drop's among them, of its look and of how it ran, which the next
    /// to serve answers anew; and the Inspector, of its world. The File menu's request goes,
    /// unanswered, with what was to follow it, and a prompt for New or Open; a close being checked
    /// or prompted for goes by the last status, as [`close`](Self::close) says of one while no game
    /// serves.
    fn disconnect(&mut self) {
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
        self.debugger.serve(session.clone());
        self.talker = Some(Talker::connect(session));
        self.served = true;
        // The game before's memory, which this one makes anew; the view shows its last frame
        // until this one's first.
        self.reader = None;
        self.asked = None;
        self.refused = false;
        self.paused = false;
        self.run_state = RunState::Running;
    }

    /// Takes the game's answers since: to `view`, which puts the memory it asked for in place of
    /// the one before; to `status`, `play`, `stop`, `pause`, `resume`, `step` and `scene`, which
    /// say how it runs, as [`take_status`](Self::take_status) takes it, a `scene` going on as
    /// [`scene_done`](Self::scene_done) says, and a status asked after a close settling it; to
    /// `look`, what the editor takes of which it takes; and errors, which it logs, a `scene`'s as
    /// [`scene_refused`](Self::scene_refused) says.
    fn take_answers(&mut self) {
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
                    // What the File menu goes on from, rather than refusals to show.
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
            // The answer to the status asked after a close, statuses being asked one at a time:
            // what it says, as `take_status` took it, or, failed, what the last said.
            if method == "status" && self.closing == Closing::Checking(true) {
                self.settle_close();
            }
        }
    }

    /// Takes `status`, how the game runs as it answered: whether it's paused, whether it's
    /// stopped, and its open scene's file, and whether it has unsaved edits. As it stops, at the
    /// editor's Stop, another client's `stop`, or a New or Open of the File menu's that stopped a
    /// copy playing, the input meant for it that wasn't sent goes, but its letting go, and the view
    /// turns to the editor's camera, if it was through the game's, whose viewpoint goes to the game
    /// with the grid, as the user's turn would, a New or an Open turning it back to the game's, as
    /// [`scene_done`](Self::scene_done) says; but neither while a Play or Simulate clicked since
    /// waits for its answer, the input then going to the copy it plays.
    fn take_status(&mut self, status: &Status) {
        self.paused = status.paused;
        self.scene.clone_from(&status.scene);
        self.unsaved = status.unsaved;
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

    /// How the game runs, as the title bar shows it: stopped, or as good as, once the editor's
    /// Stop is asked; or playing, Play through the game's camera, Simulate through the editor's.
    fn mode(&self) -> Mode {
        if self.stopped || self.stopping {
            Mode::Edit
        } else if self.view.toolbar.world_camera {
            Mode::Play
        } else {
            Mode::Simulate
        }
    }

    /// Whether the view's input goes to the game: in Play alone.
    fn through(&self) -> bool {
        self.mode() == Mode::Play
    }

    /// Does what a click of the title bar's `to` asks, while a game serves, whatever the editor
    /// last knew of how it runs: Play and Simulate ask the game to play, which changes nothing
    /// while it plays, so that a switch between them carries on, and turn the view to the game's
    /// camera or the editor's; Stop asks it to stop, and stops the view's input going to it at
    /// once, dropping what wasn't sent yet but its letting go, until it answers.
    fn run(&mut self, to: Mode) {
        let Some(talker) = &self.talker else {
            return;
        };
        match to {
            Mode::Play | Mode::Simulate => {
                talker.ask("play", Value::Null);
                self.playing += 1;
                self.view.toolbar.world_camera = to == Mode::Play;
            }
            Mode::Edit => {
                talker.ask("stop", Value::Null);
                self.stopping = true;
                self.drop_unsent();
            }
        }
    }

    /// Drops the input meant for the game that wasn't sent, but its letting go of what's held,
    /// which would stay held through a Stop otherwise, the game's input not being the world's.
    fn drop_unsent(&mut self) {
        self.input
            .retain(|event| *event == InputEvent::Focused(false));
    }

    /// What the title bar says: while a game serves, how it runs, as [`mode`](Self::mode) says,
    /// and whether it's paused; otherwise how the run goes, or that it waits for the run before it
    /// to end, as [`start`](Self::start) does.
    fn label(&self) -> String {
        if self.dir.is_none() {
            return "No project open".to_owned();
        }
        if self.waiting {
            return "Waiting for the last run to end…".to_owned();
        }
        let running = match self.mode() {
            Mode::Edit => "Stopped",
            Mode::Play => "Playing",
            Mode::Simulate => "Simulating",
        };
        match &self.run_state {
            // Its status waits too, so what it last said may be from before.
            RunState::Running if self.debugger.stopped() => {
                "Waiting at a script's stop: see the Debugger".to_owned()
            }
            RunState::Running if self.paused => format!("{running}, paused"),
            RunState::Running => running.to_owned(),
            RunState::Building(None) => "Building…".to_owned(),
            RunState::Building(Some(at)) => format!("Building… {at}"),
            RunState::Starting => "Starting…".to_owned(),
            RunState::Rebuilding(None) => "Rebuilding…".to_owned(),
            RunState::Rebuilding(Some(at)) => format!("Rebuilding… {at}"),
            RunState::Restarting => "Restarting…".to_owned(),
            RunState::Waiting => "Waiting for a rebuild".to_owned(),
            RunState::Ended(why) => format!("Ended: {why}"),
        }
    }

    /// What the title bar says of the open scene, once a game has served: its file's name, or
    /// Untitled, with a `*` after it while it has unsaved edits.
    fn scene_label(&self) -> Option<String> {
        let name = self.scene_name().unwrap_or("Untitled");
        let mark = if self.unsaved { "*" } else { "" };
        self.served.then(|| format!("{name}{mark}"))
    }

    /// The name of the open scene's file, such as `arena.ron`; `None` while it's untitled.
    fn scene_name(&self) -> Option<&str> {
        let scene = self.scene.as_deref()?;
        scene.rsplit('/').next()
    }

    /// Whether the File menu's items on the open scene, their shortcuts and the Scenes panel do
    /// anything: while a game serves, which needs a project, the hub doesn't show, and neither a
    /// prompt, a request of theirs nor a close waits.
    fn files(&self) -> bool {
        let waits = self.prompt.is_some() || self.request.is_some() || self.due.is_some();
        self.talker.is_some() && !waits && self.closing == Closing::No && !self.hub_shows()
    }

    /// Whether the hub shows over the panels: while no project is open, and from the File menu's
    /// Open Project until it's gone back from, or another project opens.
    fn hub_shows(&self) -> bool {
        self.dir.is_none() || self.hub.asked
    }

    /// Opens the project in `dir`, as the hub picked it, in place of the one open: at once while
    /// no game has served; otherwise as a close of the editor goes, as [`close`](Self::close)
    /// says, the prompt asking whether to save the open scene's unsaved edits before the project
    /// opens, and the switch going, as [`switch`](Self::switch) does, once the close is granted.
    /// The project open itself opens nothing, the hub giving way to it; and none opens while a
    /// close is under way.
    fn switch_to(&mut self, dir: PathBuf) {
        if self
            .dir
            .as_deref()
            .is_some_and(|open| hub::same(open, &dir))
        {
            self.hub.asked = false;
            return;
        }
        if self.closing != Closing::No {
            return;
        }
        self.switching = Some(dir);
        if self.close().is_continue() {
            self.closing = Closing::Granted;
        }
    }

    /// Opens the project in `dir` in place of the one open, if one is: lets go of its game, closing
    /// its run's stdin, which ends it, the run going on ending meanwhile; logs what was measured of
    /// its view; and makes the layer the new project's, as it starts but for the hub, the Logs, the
    /// runs ending and where the toolbar is, noting the project opened, and starting its run, as
    /// [`start`](Self::start) does.
    fn switch(&mut self, cx: &mut Context, dir: PathBuf) {
        self.disconnect();
        if let Some((child, stdin)) = self.run.take() {
            drop(stdin);
            let ran = self.dir.clone().unwrap_or_default();
            self.ending.push((ran, child));
        }
        self.measures.report();
        let mut opened = Game::new(Some(dir.clone()), self.view.link.clone());
        opened.hub = mem::take(&mut self.hub);
        opened.ending = mem::take(&mut self.ending);
        opened.logs = mem::take(&mut self.logs);
        opened.view.toolbar.placement = mem::take(&mut self.view.toolbar.placement);
        *self = opened;
        info!(code = "editor.project", dir = %dir.display(), "Opened a project");
        self.log(
            Level::Info,
            format!("Opened the project in {}", dir.display()),
        );
        self.hub.opened(&dir);
        self.hub.save(cx);
        self.start();
    }

    /// Picks a project's manifest with the OS's dialog, from [`Layer::ui`], and opens its project,
    /// as [`switch_to`](Self::switch_to) does; the hub says why if the folder it's in isn't a
    /// project's, or its manifest doesn't read.
    fn browse(&mut self, cx: &mut Context) {
        let games = self.hub_games();
        let picked = cx.pick_file(&FileDialog {
            title: "Open a project's manifest",
            save: false,
            filter: Some(("Project manifests", &["ron"])),
            directory: games.as_deref(),
            file_name: Some(gg_api::Project::FILE),
        });
        let Some(picked) = picked else {
            return;
        };
        let dir = picked.parent().map(Path::to_owned).unwrap_or_default();
        let found = gg_api::Project::find(&dir).is_some_and(|found| found == dir);
        let read = gg_api::Project::read(&dir);
        match read {
            Ok(_) if found => self.switch_to(dir),
            Ok(_) => self.hub.refuse(format!(
                "{} isn't a game's project: its folder holds no Cargo.toml beside its manifest",
                dir.display()
            )),
            Err(failure) => self
                .hub
                .refuse(format!("[{}] {}", failure.code, failure.message)),
        }
    }

    /// The folder Open's dialog starts in: the open project's parent, or else the workspace's games.
    fn hub_games(&self) -> Option<PathBuf> {
        let parent = self.dir.as_deref().and_then(Path::parent);
        parent.map(Path::to_owned).or_else(|| self.hub.games())
    }

    /// Does what the File menu's `item` does, from [`Layer::ui`], as the menu, a shortcut or a
    /// refusal chooses it: Open and Save As show the OS's dialog, which blocks the frame until it
    /// closes, and then do as [`open`](Self::open) and [`save_as`](Self::save_as) do with the file
    /// it picked; New prompts or asks the game, as [`act`](Self::act) says; and Save has the game
    /// save the open scene to its file, or, without one, does as Save As does. A Save As cancelled
    /// cancels what was to follow it.
    fn choose(&mut self, cx: &mut Context, item: Item) {
        match item {
            Item::New => self.act(Action::New),
            Item::Open => {
                if let Some(path) = self.pick(cx, false) {
                    self.open(path);
                }
            }
            Item::Save if self.scene.is_some() => self.save(),
            Item::Save | Item::SaveAs => match self.pick(cx, true) {
                Some(path) => self.save_as(path),
                None => self.cancel_then(),
            },
        }
    }

    /// Shows the dialog that picks a scene to open, or, if `save`, a file to save the open scene
    /// as, and returns its path, absolute, as the game takes it: one that shows scenes, in the open
    /// scene's folder, or without its file, the project's first scene folder, and suggests its
    /// file's name to save as.
    fn pick(&self, cx: &mut Context, save: bool) -> Option<PathBuf> {
        let folders = self.folders.as_ref()?;
        let file = (self.scene.as_deref()).map(|scene| under(&folders.root, scene));
        let directory = file.as_deref().and_then(Path::parent);
        let file_name = file.as_deref().filter(|_| save).and_then(Path::file_name);
        cx.pick_file(&FileDialog {
            title: if save {
                "Save the scene as"
            } else {
                "Open a scene"
            },
            save,
            filter: Some(("Scenes", &["ron"])),
            directory: directory.or(folders.scenes.first().map(PathBuf::as_path)),
            file_name: file_name.and_then(OsStr::to_str),
        })
    }

    /// Has the game open the scene at `path` in place of the open one, as [`act`](Self::act) says.
    fn open(&mut self, path: PathBuf) {
        self.act(Action::Open(path));
    }

    /// Has the game save the open scene to `picked`, the file Save As's dialog picked, with `.ron`
    /// after its name unless it ends in it, in any case, as the XDG portal's dialog may leave it. A
    /// file there at the name with `.ron` after is one the dialog didn't ask about replacing, so it
    /// isn't replaced: the editor says so, `editor.scene_exists`, and what was to follow the save
    /// is cancelled.
    fn save_as(&mut self, mut picked: PathBuf) {
        let extension = picked.extension();
        if !extension.is_some_and(|extension| extension.eq_ignore_ascii_case("ron")) {
            picked.as_mut_os_string().push(".ron");
            if picked.exists() {
                let code = "editor.scene_exists";
                let shown = picked.display();
                let message =
                    format!("{shown} is there already: pick it in the dialog to replace it");
                warn!(code, path = %shown, "{message}");
                self.log(Level::Warn, format!("[{code}] {message}"));
                let name = picked.file_name().map(OsStr::to_string_lossy);
                self.message = Some(Message {
                    title: format!("Can't save {}", name.unwrap_or_default()),
                    text: format!("[{code}] {message}"),
                });
                self.cancel_then();
                return;
            }
        }
        self.request(Scene {
            action: SceneAction::Save,
            path: Some(picked),
            discard: false,
        });
    }

    /// Has the game save the open scene to its file, or, without one as the last status said, has
    /// the next UI choose Save As.
    fn save(&mut self) {
        if self.scene.is_none() {
            self.due = Some(Item::SaveAs);
            return;
        }
        self.request(Scene {
            action: SceneAction::Save,
            path: None,
            discard: false,
        });
    }

    /// Does `action`, a New or an Open: prompts first, if the last status said the open scene has
    /// unsaved edits; otherwise asks the game, which refuses if it has some by then.
    fn act(&mut self, action: Action) {
        if self.unsaved {
            self.prompt = Some(action);
        } else if let Some(scene) = action.scene(false) {
            self.request(scene);
        }
    }

    /// Has the File menu's request, `scene`, sent to the game, as [`Request`] says.
    fn request(&mut self, scene: Scene) {
        self.request = Some(Request { scene, sent: false });
    }

    /// Does what `choice` answers the prompt with, for what it asks about: Save saves the open
    /// scene, or saves it as, then does it once the game has saved, a New or an Open again and a
    /// close going ahead, but does nothing while no game serves; Don't Save does it, discarding the
    /// unsaved edits, as a close going ahead does; Cancel does nothing, and a close, or a switch to
    /// another project, goes back to none.
    fn answer(&mut self, choice: Choice) {
        let Some(action) = self.prompt.take() else {
            return;
        };
        match choice {
            Choice::Save if self.talker.is_none() => self.prompt = Some(action),
            Choice::Save => {
                self.then = Some(action);
                self.save();
            }
            Choice::DontSave => match action.scene(true) {
                Some(scene) => self.request(scene),
                None => self.closing = Closing::Granted,
            },
            Choice::Cancel if action == Action::Close => self.cancel_close(),
            Choice::Cancel => {}
        }
    }

    /// Cancels what was to follow a save that won't be made: a close going back to none.
    fn cancel_then(&mut self) {
        if self.then.take() == Some(Action::Close) {
            self.cancel_close();
        }
    }

    /// Has a close under way, or a switch to another project, go back to none.
    fn cancel_close(&mut self) {
        self.closing = Closing::No;
        self.switching = None;
    }

    /// Goes on from the File menu's request, `scene`, that the game did: from a New or an Open, to
    /// the view through the game's camera, as the editor starts, so that it frames the scene the
    /// game has now, rather than the editor's camera framing the one before, whose grid goes too,
    /// as the user's turn would; but not while a Play or Simulate clicked since waits for its
    /// answer, which turned the view itself; from a save, to what was to follow it.
    fn scene_done(&mut self, scene: &Scene) {
        if scene.action != SceneAction::Save {
            let toolbar = &mut self.view.toolbar;
            if !toolbar.world_camera && self.playing == 0 {
                toolbar.world_camera = true;
                // `changes`, which runs before the answers are taken, doesn't see this turn.
                self.looking.change(Part::Viewpoint);
                self.looking.follow(Part::Grid);
            }
            return;
        }
        match self.then.take() {
            Some(Action::Close) => self.closing = Closing::Granted,
            Some(action) => self.act(action),
            None => {}
        }
    }

    /// Goes on from the File menu's request, `scene`, that the game refused with `failure`: a New
    /// or an Open refused for unsaved edits the last status didn't know of prompts, but while a
    /// close goes on; a Save refused for an untitled scene has the next UI choose Save As, but
    /// while a close goes on, unless it's the close prompt's own Save, the refusal logged alone;
    /// any other refusal cancels what was to follow, and shows in a message.
    fn scene_refused(&mut self, scene: Scene, failure: Failure) {
        let action = match (scene.action, scene.path) {
            (SceneAction::New, _) => Action::New,
            (SceneAction::Open, Some(path)) => Action::Open(path),
            (SceneAction::Save, path) => {
                if failure.code == "scene.untitled" && path.is_none() {
                    // A close clears what was to follow, so a Save followed by one is its prompt's.
                    if self.closing == Closing::No || self.then == Some(Action::Close) {
                        self.due = Some(Item::SaveAs);
                    }
                    return;
                }
                let name = path.as_deref().and_then(Path::file_name);
                let name = name.and_then(OsStr::to_str).or(self.scene_name());
                self.refused(
                    format!("Can't save {}", name.unwrap_or("the scene")),
                    failure,
                );
                return;
            }
            (SceneAction::Open, None) => unreachable!("the File menu opens a file"),
        };
        if failure.code == "scene.unsaved" {
            if self.closing == Closing::No {
                self.prompt = Some(action);
            }
            return;
        }
        let title = match &action {
            Action::Open(path) => {
                let name = path.file_name().map(OsStr::to_string_lossy);
                format!("Can't open {}", name.unwrap_or_default())
            }
            _ => "Can't make a new scene".to_owned(),
        };
        self.refused(title, failure);
    }

    /// Shows `failure`, a refusal of the File menu's request, in a message titled `title`,
    /// cancelling what was to follow it.
    fn refused(&mut self, title: String, failure: Failure) {
        let Failure { code, message } = failure;
        self.message = Some(Message {
            title,
            text: format!("[{code}] {message}"),
        });
        self.cancel_then();
    }

    /// What the prompt asks about `action`, and says of it: whether to save the open scene's
    /// unsaved edits first.
    fn asks(&self, action: &Action) -> (String, String) {
        let title = match self.scene_name() {
            Some(name) => format!("Save the edits to {name}?"),
            None => "Save the untitled scene?".to_owned(),
        };
        let then = match action {
            Action::New => "a new scene".to_owned(),
            Action::Open(path) => {
                let name = path.file_name().map(OsStr::to_string_lossy);
                format!("{} opens", name.unwrap_or_default())
            }
            Action::Close => match &self.switching {
                Some(dir) => {
                    let name = dir.file_name().map(OsStr::to_string_lossy);
                    format!("the project {} opens", name.unwrap_or_default())
                }
                None => "the editor closes".to_owned(),
            },
        };
        let text = format!("Its unsaved edits are lost unless they're saved before {then}.");
        (title, text)
    }

    /// Whether a close of the editor, by Quit or the window's close button, goes ahead, as
    /// [`Event::CloseRequested`] asks: at once outside a project and before any game has served,
    /// and once it's granted. Otherwise it's vetoed, `Break`, while the editor checks whether the
    /// open scene has unsaved edits, asking the game for a status once the Inspector has nothing
    /// waiting for it; granted if it has none, and prompted for if it has, a prompt for a New or an
    /// Open giving way to it. While no game serves after one has, as `gg run` restarts it, waits
    /// for a rebuild or the game crashed, only the last status answers: granted if it said none,
    /// and otherwise prompted for without Save, which no game would do; and a close being checked,
    /// or prompted for, as the game goes goes the same way. A close while the window is minimized
    /// waits until it's restored, since no frames run meanwhile. A switch to another project goes
    /// the same way, as [`switch_to`](Self::switch_to) says.
    fn close(&mut self) -> ControlFlow<()> {
        match self.closing {
            Closing::Granted => return ControlFlow::Continue(()),
            Closing::Checking(_) | Closing::Prompting => return ControlFlow::Break(()),
            Closing::No if !self.served => return ControlFlow::Continue(()),
            Closing::No => {}
        }
        self.prompt = None;
        self.then = None;
        self.due = None;
        self.closing = Closing::Checking(false);
        if self.talker.is_none() {
            self.settle_close();
        }
        if self.closing == Closing::Granted {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        }
    }

    /// Settles a close by whether the open scene has unsaved edits, as the last status said: it's
    /// prompted for if it has some, and granted otherwise.
    fn settle_close(&mut self) {
        if self.unsaved {
            self.prompt = Some(Action::Close);
            self.closing = Closing::Prompting;
        } else {
            self.prompt = None;
            self.closing = Closing::Granted;
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
    /// picks; a dropped texture's or font's, which [`take_dropped`](Self::take_dropped) takes; and
    /// all of them for what's under the cursor, which [`ui`](Layer::ui) reads; and failures, which
    /// it logs.
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
            let dropped = self.dropping.take_if(|dropping| dropping.ticket == ticket);
            match answer {
                Ok(picked) => {
                    let found = Found::of(picked);
                    if let Some(dropped) = dropped {
                        self.take_dropped(dropped, found.clone());
                    }
                    self.answered.push((ticket, found));
                }
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

    /// Takes what the Assets panel let go over `response`, the view's, this frame, while a game
    /// serves, as [`let_go`](Self::let_go) says, with the view at `view` and the window at `scale`
    /// pixels a point; and outlines the view while an asset is held over it.
    fn take_drop(
        &mut self,
        egui: &egui::Context,
        response: &Response,
        view: Option<Viewport>,
        scale: f32,
    ) {
        if self.talker.is_none() {
            return;
        }
        if response.dnd_hover_payload::<Dragged>().is_some() {
            overlay::outline_drop(egui, response);
        }
        let Some(dropped) = response.dnd_release_payload::<Dragged>() else {
            return;
        };
        // Where it was let go, as egui places what it hits; kept for its frame by a tap, as the
        // cursor isn't once the window loses it.
        let Some(at) = egui.pointer_interact_pos() else {
            return;
        };
        let ppp = egui.pixels_per_point();
        let at = [at.x * ppp, at.y * ppp].map(f64::from);
        self.let_go(&dropped, at, view, scale);
    }

    /// Takes `dropped`, let go at `at`, in the main window's pixels, over the view, at `view`,
    /// with the window at `scale` pixels a point, as the module says, naming it as [`asset_name`]
    /// does: a texture or a font asks the game for a pick there, whose answer
    /// [`take_picks`](Self::take_picks) takes; a model is a root the Inspector's link creates where
    /// it [`lands`](Self::landing), named after its file, and a prefab an instance's root there,
    /// which takes its prefab's root's name; or where neither can land yet, before a frame has
    /// come, is logged as taken by nothing.
    fn let_go(&mut self, dropped: &Dragged, at: [f64; 2], view: Option<Viewport>, scale: f32) {
        let root = self.folders.as_ref().map(|folders| folders.root.as_path());
        let name = asset_name(root, &dropped.path);
        let file = dropped.path.file_name().unwrap_or_default();
        let file = file.to_string_lossy().into_owned();
        match dropped.kind {
            kind @ (AssetKind::Texture | AssetKind::Font) => {
                let origin = view.map_or([0, 0], |view| view.origin);
                if let Some(ticket) = self.ask_pick(at, origin) {
                    self.dropping = Some(Dropping {
                        ticket,
                        kind,
                        name,
                        file,
                    });
                }
            }
            kind @ (AssetKind::Model | AssetKind::Prefab) => {
                let Some(landing) = self.landing(at, view, scale) else {
                    let why = "the view has shown no frame to place it in yet";
                    self.log(Level::Info, format!("{file} was let go, but {why}"));
                    return;
                };
                let transform = Transform {
                    translation: landing,
                    ..Transform::IDENTITY
                };
                // A tuple struct's field, `0`, as the game reads one.
                let root = vec![("0".into(), reflect::Value::String(name))];
                let (root, transform) = (reflect::Value::Struct(root), transform.to_value());
                let components = if kind == AssetKind::Model {
                    let stem = dropped.path.file_stem().unwrap_or_default();
                    let called = Name(stem.to_string_lossy().into_owned());
                    vec![
                        ("Name".into(), called.to_value()),
                        ("Transform".into(), transform),
                        ("ModelRoot".into(), root),
                    ]
                } else {
                    // Named as its prefab's root is, which it takes.
                    vec![("Transform".into(), transform), ("PrefabRoot".into(), root)]
                };
                self.view.link.create(components);
            }
            // The Assets panel drags none: nothing in the view takes a UI.
            AssetKind::Ui => {}
        }
    }

    /// Takes `found`, what the game's view shows where `dropped`, a texture or a font, was let go:
    /// the entity there, which the Inspector picks, its sprite's texture or its text's font set
    /// through the Inspector's link as an edit of its own, which the game refuses for an entity
    /// without one, the Inspector showing why; and where no entity draws, nothing, which the Logs
    /// say.
    fn take_dropped(&mut self, dropped: Dropping, found: Option<Found>) {
        let Some(found) = found else {
            let said = format!("{} was let go where no entity draws", dropped.file);
            self.log(Level::Info, said);
            return;
        };
        let (component, field) = match dropped.kind {
            AssetKind::Font => ("Text", "font"),
            AssetKind::Texture | AssetKind::Model | AssetKind::Prefab | AssetKind::Ui => {
                ("Sprite", "texture")
            }
        };
        let link = &self.view.link;
        link.pick(Some(found.id));
        self.took_pick = None;
        let asset = reflect::Value::String(dropped.name);
        let asset = reflect::Value::Option(Some(Box::new(asset)));
        link.set(found.id, component, field, asset);
        link.seal();
    }

    /// Where a model let go at `at`, in the main window's pixels, lands, with the view at `view`,
    /// and the window at `scale` pixels a point: where the view's last frame shows the grid's
    /// plane through the origin, as the grid would lie, whether or not it shows; or where its ray
    /// runs along the plane, or meets it behind the camera, under `at` on the plane through the
    /// editor's camera's pivot facing the view, which sees as the game's camera does while that
    /// drives the view. `None` until a frame that projects has come.
    fn landing(&self, at: [f64; 2], view: Option<Viewport>, scale: f32) -> Option<Vec3> {
        let (seen, screen) = self.seen.zip(self.screen(view))?;
        let plane = grid::shown(&seen, self.view.toolbar.plane);
        let sight = Sight::new(seen, screen, scale)?;
        let pixel = Vec2::new(at[0] as f32, at[1] as f32);
        let met = sight.on_plane(pixel, plane.normal());
        Some(met.unwrap_or_else(|| self.view.camera.under(at, screen)))
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

    /// Frames the entity the Entities panel asked to frame, as the view's F does, once the link
    /// places it: the pick, as the panel picks what it frames; or drops the ask as the pick changes.
    fn frame_asked(&mut self) {
        let link = &self.view.link;
        if let Some(asked) = link.framed() {
            self.framing = Some(asked);
        }
        let Some(asked) = self.framing else {
            return;
        };
        if link.picked() != Some(asked) {
            self.framing = None;
        } else if let Some(placed) = picked(link).filter(|placed| placed.entity == asked) {
            self.view.frame(&placed);
            self.framing = None;
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
    /// the view at `view`: in Play, it goes to the game, in the view's pixels, and to the view's
    /// tools only if it lets go of what's held, so that no click picks, and no wheel or button
    /// turns the view to the editor's camera; otherwise the tools take it, the game getting
    /// nothing, and a press off the handles asks the game for a pick. In every mode, neither gets
    /// a key of the File menu's shortcuts, the key at N, O or S's position, as [`shortcuts_key`]
    /// says. `Break` for what the tools took.
    fn route(
        &mut self,
        event: &Event<'_>,
        reading: &Reading,
        view: Option<Viewport>,
    ) -> ControlFlow<()> {
        let origin = view.map_or([0, 0], |view| view.origin);
        let through = self.through();
        // Escape lets go of the cursor grabbed for the game, whatever the game does with it, as a
        // game may not let go itself; a press in the view grabs it again. The game gets both, as
        // its own window would give them.
        let escape = matches!(
            *event,
            Event::Key {
                key: Key::Escape,
                pressed: true,
                ..
            }
        );
        if through && escape {
            self.escaped = true;
        }
        if through && matches!(*event, Event::MouseButton { pressed: true, .. }) {
            self.escaped = false;
        }
        let shortcut = shortcuts_key(event, reading.command);
        let handled = if shortcut || (through && !lets_go(event)) {
            ControlFlow::Continue(())
        } else {
            self.view.take(event, reading, picked(&self.view.link))
        };
        if through
            && !shortcut
            && self.talker.is_some()
            && let Some(input) = forwarded(event, origin)
        {
            self.queue(input);
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

    /// Tells the game the view's input started or stopped going to it, as
    /// [`through`](Self::through) says, if it did since it was last told: as it stops, the game
    /// loses the focus, letting go of what's held; as it starts, it gets the cursor, at `cursor`,
    /// in the main window's pixels, if it's over the view, at `view`.
    fn tell(&mut self, cursor: Option<[f64; 2]>, view: Option<Viewport>) {
        let through = self.through();
        if through == mem::replace(&mut self.told, through) || self.talker.is_none() {
            return;
        }
        if !through {
            self.queue(InputEvent::Focused(false));
            return;
        }
        if let (Some(at), Some(view)) = (cursor, view) {
            let [x, y] = in_view(at, view.origin);
            let [width, height] = view.size.map(f64::from);
            if (0.0..width).contains(&x) && (0.0..height).contains(&y) {
                self.queue(InputEvent::Cursor([x, y]));
            }
        }
    }

    /// Queues `input` for the game's next `input`, a move of the cursor taking the place of one
    /// queued just before it, and the mouse's raw motion adding to the motion queued just before
    /// it: so that while an `input` waits for its answer, as while a script has stopped the game,
    /// what's queued grows with presses, as a key's, rather than with every move of the mouse. The
    /// game gets what's queued then in one frame, as a game's window gets what its OS queued while
    /// its frames waited.
    fn queue(&mut self, input: InputEvent) {
        match (self.input.last_mut(), input) {
            (Some(InputEvent::Cursor(last)), InputEvent::Cursor(at)) => *last = at,
            (Some(InputEvent::Motion([x, y])), InputEvent::Motion([by_x, by_y])) => {
                *x += by_x;
                *y += by_y;
            }
            (_, input) => self.input.push(input),
        }
    }
}

/// The folders of the project in directory `dir`, as its manifest names them; or if the manifest
/// doesn't read, which `gg run` says why of, `dir` as the asset root, and no scene folders.
fn folders(dir: &Path) -> Folders {
    match gg_api::Project::read(dir) {
        Ok(project) => Folders {
            root: project.assets_dir(dir),
            scenes: project.scene_dirs(dir),
            start: Some(project.start_path(dir)),
        },
        Err(_) => Folders {
            root: dir.to_owned(),
            scenes: Vec::new(),
            start: None,
        },
    }
}

/// What the game names the asset at `path` by: its ID, as its ID file beside it gives it; or,
/// without one that reads, as before the game's scan has written one, its path from the asset
/// root, `root`, with `/` between its parts, which the game loads it by, naming it by the ID its
/// scan finds for it.
fn asset_name(root: Option<&Path>, path: &Path) -> String {
    if let Ok(meta) = Meta::read(&Meta::path(path)) {
        return meta.id.to_string();
    }
    let under = root.and_then(|root| path.strip_prefix(root).ok());
    let parts = under.unwrap_or(path).iter().map(OsStr::to_string_lossy);
    parts.collect::<Vec<_>>().join("/")
}

/// `path`, relative, with `/` between its parts, as manifests and the API name files, joined to
/// `dir` a part at a time, so that its separators are the OS's.
fn under(dir: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(dir.to_owned(), |under, part| under.join(part))
}

/// Whether `event` is a press, or a repeat, of a key of the File menu's shortcuts, the key at N, O
/// or S's position, whatever the layout calls it, as the menu matches it, with `command` held,
/// Ctrl, or Cmd on macOS, which the menu takes from egui: neither the game nor the view's tools
/// get it. Its release does, which changes nothing without its press.
fn shortcuts_key(event: &Event<'_>, command: bool) -> bool {
    let key = matches!(
        *event,
        Event::Key {
            key: Key::KeyN | Key::KeyO | Key::KeyS,
            pressed: true,
            ..
        }
    );
    command && key
}

/// Whether `event` lets go of what's held: a key's or a button's release, or the loss of the
/// focus, as the engine hands each past a layer that takes it.
fn lets_go(event: &Event<'_>) -> bool {
    match *event {
        Event::Key { pressed, .. } | Event::MouseButton { pressed, .. } => !pressed,
        Event::Focused(focused) => !focused,
        _ => false,
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
    /// Reads the projects the hub keeps, and where the toolbar was, and opens the project the
    /// editor was started in, noting it, or else the latest opened, if the hub's to skip to it; or,
    /// with neither, opens none, the hub showing.
    fn attach(&mut self, cx: &mut Context) {
        self.hub.read(cx);
        self.view.toolbar.read(cx);
        match self.dir.clone() {
            Some(dir) => {
                self.hub.opened(&dir);
                self.hub.save(cx);
                self.start();
            }
            None => {
                if let Some(latest) = self.hub.skip_to() {
                    self.switch(cx, latest);
                }
            }
        }
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        // The runs of the projects open before, which may hold up this project's.
        (self.ending).retain_mut(|(_, child)| matches!(child.try_wait(), Ok(None)));
        if self.waiting {
            self.start();
        }
        // Before `gg run`'s lines, which may say the game has gone, letting go of what it logged.
        self.take_logged();
        self.take_lines();
        // What the user changed since the last update, before what the game answered changes it,
        // and before the camera settles, which changes nothing the view shows.
        self.view.camera.update(dt);
        self.frame_asked();
        self.changes(wants(&self.view));
        self.view.settle();
        self.take_answers();
        self.debugger.update();
        self.take_picks();
        if let Some(view) = cx.view() {
            self.fit(view.size);
        }
        self.take_frame(cx);
        // The game's grab, in Play alone; a Stop or Simulate ends what Escape did too.
        let through = self.through();
        self.escaped &= through;
        let asked = self.reader.as_ref().is_some_and(Reader::cursor_grabbed);
        cx.set_cursor_grabbed(through && asked && !self.escaped);
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
        // The File menu's request, and a close's status, once the game has answered the Inspector's
        // edits, which go on a connection of their own: so that the game has them first.
        let idle = self.view.link.idle();
        if let Some(talker) = &self.talker
            && let Some(request) = &mut self.request
            && !request.sent
            && idle
        {
            talker.ask("scene", json!(request.scene));
            request.sent = true;
        }
        let checking = self.closing == Closing::Checking(false) && idle;
        if let Some(talker) = &self.talker
            && !self.statusing
            && (checking
                || self
                    .polled
                    .is_none_or(|asked| asked.elapsed() >= STATUS_EVERY))
        {
            talker.ask("status", Value::Null);
            self.statusing = true;
            self.polled = Some(Instant::now());
            if checking {
                self.closing = Closing::Checking(true);
            }
        }
    }

    /// Has the view's tools see what the view gets, or in Play the game, as [`route`](Game::route)
    /// says; and vetoes a close of the editor until it's granted, as [`close`](Game::close) says, a
    /// switch to another project under way becoming the close.
    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        if *event == Event::CloseRequested {
            self.switching = None;
            return self.close();
        }
        let reading = view::reading(cx, self.screen(cx.view()));
        self.route(event, &reading, cx.view())
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        let files = self.files();
        let projects = self.closing == Closing::No && self.prompt.is_none() && !self.hub_shows();
        let (mut clicked, mut to_hub) = (None, false);
        ui.menu("File", |ui| {
            ui.add_enabled_ui(files, |ui| {
                for item in Item::MENU {
                    let shortcut = ui.ctx().format_shortcut(&item.shortcut());
                    let button = egui::Button::new(item.label()).shortcut_text(shortcut);
                    if ui.add(button).clicked() {
                        clicked = Some(item);
                    }
                }
            });
            ui.separator();
            let open = ui.add_enabled(projects, egui::Button::new("Open Project..."));
            let tip = "Show the projects, to open another in this one's place";
            to_hub = open.on_hover_text(tip).clicked();
            ui.separator();
            if ui.button("Quit").clicked() {
                ui.ctx()
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
        });
        if to_hub {
            self.hub.asked = true;
        }
        // The hub, over the panels, which go on showing the project open, if one is, behind it.
        if self.hub_shows() {
            let current = self.dir.clone();
            let hub = &mut self.hub;
            match ui.cover(|ui| hub.show(ui, current.as_deref())) {
                Some(hub::Chosen::Open(dir)) => self.switch_to(dir),
                Some(hub::Chosen::Browse) => self.browse(cx),
                Some(hub::Chosen::Back) => self.hub.asked = false,
                None => {}
            }
            self.hub.save(cx);
        }
        // The shortcuts, whichever panel of the main window has the keys. What's chosen is done
        // once the menu is drawn, since Open and Save As show a dialog, which takes the context
        // and blocks the frame.
        let pressed = ui
            .egui()
            .input_mut(|input| file::shortcut(&mut input.events));
        let chosen = clicked.or(pressed.filter(|_| files));
        if let Some(item) = self.due.take().or(chosen) {
            self.choose(cx, item);
        }
        let (label, mode) = (self.label(), self.mode());
        let unsaved = if self.unsaved {
            ", with unsaved edits"
        } else {
            ""
        };
        let scene = self.scene_label().map(|scene| {
            let tip = match &self.scene {
                Some(file) => format!("The open scene, {file}{unsaved}"),
                None => format!("The open scene, which has no file yet{unsaved}"),
            };
            (scene, tip)
        });
        let paused = self.paused;
        let talker = self.talker.as_ref();
        // None over the hub, where the bar shows the editor's title.
        let bar = !self.hub_shows();
        let title_bar = |ui: &mut egui::Ui| {
            if let Some((scene, tip)) = scene {
                ui.label(scene).on_hover_text(tip);
            }
            ui.label(label);
            let serves = talker.is_some();
            let clicked = ui.add_enabled_ui(serves, |ui| overlay::show_run(ui, mode));
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
            let step = ui
                .add_enabled(serves && mode != Mode::Edit, egui::Button::new("Step"))
                .on_hover_text("Run a tick of the game's simulation, and pause it");
            let step = if serves {
                step.on_disabled_hover_text("Play first: a stopped game runs no ticks")
            } else {
                step
            };
            if step.clicked()
                && let Some(talker) = talker
            {
                talker.ask("step", Value::Null);
            }
            clicked.inner
        };
        let clicked = bar.then(|| ui.title_bar(title_bar)).flatten();
        if let Some(to) = clicked.flatten() {
            self.run(to);
        }
        // A click on a scene, once the menus' choices are done, as a New of the File menu.
        let (open, unsaved, enabled) = (self.scene.clone(), self.unsaved, self.files());
        let scenes = &mut self.scenes;
        let mut chosen = None;
        ui.panel("Scenes", |ui| match scenes {
            Some(scenes) => chosen = scenes.show(ui, open.as_deref(), unsaved, enabled),
            None => {
                ui.weak("Not in a game's project");
            }
        });
        match chosen {
            Some(scenes::Chosen::Open(path)) => self.open(path),
            Some(scenes::Chosen::New) => self.act(Action::New),
            None => {}
        }
        let assets = &mut self.assets;
        ui.panel("Assets", |ui| match assets {
            Some(assets) => assets.show(ui),
            None => {
                ui.weak("Not in a game's project");
            }
        });
        let debugger = &mut self.debugger;
        ui.panel("Debugger", |ui| debugger.show(ui));
        // What's held over the view counts as over it, as egui's hover doesn't while another
        // widget drags, so that the corner names what would take it.
        let view = ui.view();
        let held = self.talker.is_some()
            && (view.as_ref()).is_some_and(|view| view.dnd_hover_payload::<Dragged>().is_some());
        if let Some(response) = &view {
            self.take_drop(ui.egui(), response, cx.view(), cx.scale() as f32);
        }
        let logs = &self.logs;
        ui.panel("Logs", |ui| show_logs(ui, logs));
        // What the game's view shows under the cursor, while the cursor's over it: a pick at the
        // cursor as each comes back, and what the latest answer found, painted over the handles.
        let origin = cx.view().map_or([0, 0], |view| view.origin);
        let over = view.filter(|view| view.hovered() || held);
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
        self.view.overlay(ui, self.screen(cx.view()));
        self.view.toolbar.save(cx);
        // As Play, Simulate, Stop and the toolbar's switch went.
        self.tell(cx.input().cursor(), cx.view());
        if let Some(action) = &self.prompt {
            let (title, text) = self.asks(action);
            let saves = self.talker.is_some();
            if let Some(choice) = file::prompt(ui.egui(), &title, &text, saves) {
                self.answer(choice);
            }
        }
        if let Some(message) = &self.message
            && message.show(ui.egui())
        {
            self.message = None;
        }
        // A close granted: a switch to another project, which goes now, or the editor's, which the
        // main window's close makes again, to go ahead.
        if self.closing == Closing::Granted {
            match self.switching.take() {
                Some(dir) => self.switch(cx, dir),
                None => ui
                    .egui()
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close),
            }
        }
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

/// What `gg run`'s `building` event `event` says the build has come to, for the title bar: as it
/// compiles, how many of the crates dx counts on it has compiled, as `212 of 530 crates`; else its
/// stage, as `linking`.
fn progress(event: &Value) -> String {
    match (event["compiled"].as_u64(), event["crates"].as_u64()) {
        (Some(compiled), Some(crates)) => format!("{compiled} of {crates} crates"),
        _ => event["stage"]
            .as_str()
            .unwrap_or_default()
            .replace('_', " "),
    }
}

/// The Logs panel's line for `event`, an event of the game's JSON log, as the console shows it but
/// for where and when it was logged: `[code] message  name=value ...`, a string among the fields
/// quoted if it's empty or has spaces; at its level, `Debug` for a debug or trace event.
fn logged(event: &Value) -> (Level, String) {
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
    (level, text)
}

/// Shows the Logs panel's lines, `logs`, in `ui`: a row to each line of their text, unwrapped, as
/// rustc renders its errors, the newest showing while the panel's scrolled to the bottom. It lays
/// out only the rows in view: laying out all the [`LOGGED`] the panel keeps took 5 ms a frame in a
/// debug build, its egui optimized.
fn show_logs(ui: &mut egui::Ui, logs: &VecDeque<Logged>) {
    let first = logs.front().map_or(0, |logged| logged.line);
    let rows = (logs.back()).map_or(0, |last| last.line + last.text.split('\n').count() - first);
    let height = ui.text_style_height(&TextStyle::Monospace);
    ScrollArea::both()
        .stick_to_bottom(true)
        .auto_shrink(false)
        .show_rows(ui, height, rows, |ui, shown| {
            ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
            let (start, end) = (first + shown.start, first + shown.end);
            let from = logs.partition_point(|logged| logged.line <= start);
            for logged in (logs.range(from.saturating_sub(1)..)).take_while(|l| l.line < end) {
                let (skip, take) = (
                    start.saturating_sub(logged.line),
                    end - logged.line.max(start),
                );
                for line in logged.text.split('\n').skip(skip).take(take) {
                    let text = RichText::new(line).monospace();
                    let text = match logged.level {
                        Level::Debug => text.weak(),
                        Level::Info => text,
                        Level::Warn => text.color(Color32::from_rgb(230, 190, 60)),
                        Level::Error => text.color(Color32::from_rgb(240, 90, 80)),
                    };
                    ui.label(text);
                }
            }
        });
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
        Event::MouseMotion { x, y } => InputEvent::Motion([x, y]),
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
        .and_then(|mut client| client.set_timeout(None).map(|()| client));
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
    use std::fs;

    use gg_api::view::Writer;
    use gg_api::{By, Look, Outline};
    use gg_engine::Key;

    use super::*;
    use crate::assets::tests::{Scratch, find, texts};
    use crate::camera::Tool;
    use crate::tests::{SCALE, VIEW, reading};

    /// The game's ends of the connections of a layer that [`serving`] gives: the requests the layer
    /// sends, where their answers go back, the picks it asks for, and where their answers go back.
    struct Ends {
        asked: Receiver<(String, Value)>,
        answer: Sender<(String, Result<Value, Failure>)>,
        picks: Receiver<(u64, [f64; 2])>,
        found: Sender<(u64, Result<gg_api::Picked, Failure>)>,
    }

    /// The layer with a game serving, as `serve` leaves it, that plays, through its camera, as the
    /// game was told; and the game's ends of its connections.
    fn serving() -> (Game, Ends) {
        let mut game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
        let (asks, asked) = mpsc::channel();
        let (answer, answers) = mpsc::channel();
        let (_, logged) = mpsc::channel();
        let (picks, picking) = mpsc::channel();
        let (found, picked) = mpsc::channel();
        game.talker = Some(Talker {
            asks,
            answers,
            logged,
            picks,
            picked,
        });
        game.run_state = RunState::Running;
        (game.stopped, game.told, game.served) = (false, true, true);
        let ends = Ends {
            asked,
            answer,
            picks: picking,
            found,
        };
        (game, ends)
    }

    /// The game's answer to a `status`, `play`, `stop`, `pause`, `resume` or `step`, stopped or
    /// not, and paused or not.
    fn status(stopped: bool, paused: bool) -> Result<Value, Failure> {
        let status = Status {
            ticks: 0,
            paused,
            stopped,
            frames: 1,
            entities: 0,
            view: None,
            scene: None,
            unsaved: false,
            edited: false,
        };
        Ok(json!(status))
    }

    /// The game's answer to a `status` or a `scene`, stopped, its open scene's file `scene`, with
    /// unsaved edits or not.
    fn scene_status(scene: Option<&str>, unsaved: bool) -> Result<Value, Failure> {
        let status = Status {
            ticks: 0,
            paused: false,
            stopped: true,
            frames: 1,
            entities: 0,
            view: None,
            scene: scene.map(str::to_owned),
            unsaved,
            edited: unsaved,
        };
        Ok(json!(status))
    }

    /// The `scene` requests the layer whose ends are `ends` sent since.
    fn scenes(ends: &Ends) -> Vec<Scene> {
        let asked = ends
            .asked
            .try_iter()
            .filter(|(method, _)| method == "scene");
        let scenes = asked.map(|(_, scene)| serde_json::from_value(scene).expect("a scene"));
        scenes.collect()
    }

    /// The `scene` request that saves the open scene, to `path`, or to its file.
    fn save(path: Option<PathBuf>) -> Scene {
        Scene {
            action: SceneAction::Save,
            path,
            discard: false,
        }
    }

    /// The methods of the requests the layer whose ends are `ends` sent since.
    fn methods(ends: &Ends) -> Vec<String> {
        ends.asked.try_iter().map(|(method, _)| method).collect()
    }

    /// The looks the layer whose ends are `ends` asked the game for since.
    fn looks(ends: &Ends) -> Vec<Look> {
        let asked = ends.asked.try_iter().filter(|(method, _)| method == "look");
        let looks = asked.map(|(_, look)| serde_json::from_value(look).expect("a look"));
        looks.collect()
    }

    /// In Play, all the view gets goes to the game, in the view's pixels, whatever the tool: what
    /// the editor's camera takes otherwise among it, a press of its buttons, Alt with the left, the
    /// left under Pan or Orbit, the wheel and Home, which leave the view on the game's camera; and
    /// a press asks for no pick. Stopped, through the game's camera, as the game starts, nothing
    /// goes, but a press off the handles asks for a pick, and what the editor's camera takes turns
    /// the view to it; in Simulate, nothing goes either, and Q, W, E and R pick tools. Turning back
    /// to the game's camera while it plays gives the game the cursor, if it's over the view.
    #[test]
    fn in_play_the_game_gets_all_the_views_input() {
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
        assert_eq!(ends.picks.try_iter().collect::<Vec<_>>(), [], "no pick");
        assert_eq!(game.view.asked, None);
        assert_eq!(game.view.toolbar.tool, Tool::Select, "the game's W");
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
            let (mut game, ends) = serving();
            game.view.toolbar.tool = tool;
            let routed = game.route(&event, &read, Some(VIEW));
            assert_eq!(routed, ControlFlow::Continue(()), "{event:?} with {tool:?}");
            assert!(game.view.toolbar.world_camera, "{event:?} with {tool:?}");
            let sent = forwarded(&event, VIEW.origin).expect("the game gets it");
            assert_eq!(game.input, [sent], "{event:?}");
            assert_eq!(ends.picks.try_iter().count(), 0, "{event:?} with {tool:?}");
        }
        // Stopped, through the game's camera.
        let (mut game, ends) = serving();
        (game.stopped, game.told) = (true, false);
        for event in [moved, w, left[0], left[1]] {
            let routed = game.route(&event, &reading(at), Some(VIEW));
            assert_eq!(routed, ControlFlow::Continue(()), "{event:?}");
        }
        assert_eq!(ends.picks.try_iter().collect::<Vec<_>>(), [(0, in_game)]);
        assert!(game.view.toolbar.world_camera);
        let routed = game.route(&wheel, &reading(at), Some(VIEW));
        assert_eq!(routed, ControlFlow::Break(()));
        assert!(!game.view.toolbar.world_camera, "the editor's camera");
        assert_eq!(game.input, []);
        // In Simulate, through the editor's camera.
        let (mut game, ends) = serving();
        game.view.toolbar.world_camera = false;
        game.tell(Some(at), Some(VIEW));
        assert_eq!(game.input, [InputEvent::Focused(false)], "the game lets go");
        game.input.clear();
        for event in [moved, w, left[0], left[1], wheel] {
            let _ = game.route(&event, &reading(at), Some(VIEW));
        }
        assert_eq!(game.input, []);
        assert_eq!(game.view.toolbar.tool, Tool::Move, "the editor's W");
        assert_eq!(ends.picks.try_iter().collect::<Vec<_>>(), [(0, in_game)]);
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

    /// Play and Simulate each ask the game to play, whatever the editor last knew of it, Play
    /// turning the view to the game's camera and Simulate to the editor's, so that a switch between
    /// them carries on; Stop asks it to stop, after which none of the view's input goes to the
    /// game, what wasn't sent yet among it, until it answers, but its letting go. Its answer ends
    /// that, whether the game refused, playing on, or stopped, the view turning to the editor's
    /// camera; but not if a Play clicked since waits for its answer.
    #[test]
    fn play_and_simulate_ask_the_game_to_play_and_stop_to_stop() {
        let (mut game, ends) = serving();
        let at = [300.0, 200.0];
        let w = Event::Key {
            key: Key::KeyW,
            pressed: true,
            repeat: false,
        };
        assert_eq!(game.mode(), Mode::Play);
        for (to, through) in [
            (Mode::Simulate, false),
            (Mode::Play, true),
            (Mode::Play, true),
        ] {
            game.run(to);
            assert_eq!(methods(&ends), ["play"], "{to:?}");
            assert_eq!(game.view.toolbar.world_camera, through, "{to:?}");
            assert_eq!(game.mode(), to);
            let played = ("play".to_owned(), status(false, false));
            ends.answer.send(played).expect("sent");
            game.take_answers();
        }
        let _ = game.route(&w, &reading(at), Some(VIEW));
        assert_eq!(game.input.len(), 1, "W goes to the game");
        game.run(Mode::Edit);
        assert_eq!(methods(&ends), ["stop"]);
        assert_eq!(game.mode(), Mode::Edit, "until it answers");
        for _ in 0..2 {
            let _ = game.route(&w, &reading(at), Some(VIEW));
        }
        assert_eq!(
            game.input,
            [InputEvent::Focused(false)],
            "its letting go alone"
        );
        let refused = Failure::new("api.no_stop", "a recorded run plays from start to end");
        ends.answer
            .send(("stop".to_owned(), Err(refused)))
            .expect("sent");
        game.take_answers();
        assert_eq!(game.mode(), Mode::Play, "refused");
        game.run(Mode::Edit);
        let stopped = ("stop".to_owned(), status(true, false));
        ends.answer.send(stopped).expect("sent");
        game.take_answers();
        assert!(!game.stopping);
        assert_eq!(game.mode(), Mode::Edit, "stopped");
        assert!(!game.view.toolbar.world_camera, "the editor's camera");
        // Stop, then Play before Stop's answer, which, to the request before, leaves the view
        // where Play turned it, the play's answer finding it there.
        game.run(Mode::Play);
        let played = ("play".to_owned(), status(false, false));
        ends.answer.send(played).expect("sent");
        game.take_answers();
        game.run(Mode::Edit);
        game.run(Mode::Play);
        for (method, stopped) in [("stop", true), ("play", false)] {
            let answer = (method.to_owned(), status(stopped, false));
            ends.answer.send(answer).expect("sent");
        }
        game.take_answers();
        assert_eq!(game.mode(), Mode::Play, "the last click's");
        assert!(game.view.toolbar.world_camera, "the game's camera");
    }

    /// Another client's `play`, `pause` and `stop` reach the editor in the `status` it asks for:
    /// playing, Play or Simulate lights as the toolbar's switch says, and in Play the view's input
    /// goes to the game; stopped, the view turns to the editor's camera, the input not sent going
    /// and the game letting go of what's held, and the next look sets the editor's viewpoint and
    /// the grid, though the user didn't turn the view. A game that answers stopped as it starts
    /// leaves the view as it was.
    #[test]
    fn another_clients_play_and_stop_reach_the_view() {
        let (mut game, ends) = serving();
        (game.stopped, game.told) = (true, false);
        let mut cx = Context::seeded(1);
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(methods(&ends), ["status"]);
        ends.answer
            .send(("status".to_owned(), status(true, false)))
            .expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(game.label(), "Stopped");
        assert!(game.view.toolbar.world_camera, "as it starts");
        ends.answer
            .send(("status".to_owned(), status(false, true)))
            .expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(game.mode(), Mode::Play);
        assert_eq!(game.label(), "Playing, paused");
        game.view.toolbar.world_camera = false;
        assert_eq!(game.label(), "Simulating, paused", "by the switch");
        game.view.toolbar.world_camera = true;
        let w = Event::Key {
            key: Key::KeyW,
            pressed: true,
            repeat: false,
        };
        let _ = game.route(&w, &reading([300.0, 200.0]), Some(VIEW));
        let w_down = InputEvent::Key {
            key: "KeyW".to_owned(),
            pressed: true,
            repeat: false,
        };
        assert_eq!(game.input, std::slice::from_ref(&w_down));
        ends.answer
            .send(("status".to_owned(), status(true, false)))
            .expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(game.mode(), Mode::Edit);
        assert!(!game.view.toolbar.world_camera, "the editor's camera");
        let sent: Vec<_> = ends.asked.try_iter().collect();
        let asked = |method: &str| {
            let request = sent.iter().find(|(asked, _)| asked == method);
            request
                .unwrap_or_else(|| panic!("no {method}: {sent:?}"))
                .1
                .clone()
        };
        let events: Vec<InputEvent> =
            serde_json::from_value(asked("input")["events"].clone()).expect("events");
        assert_eq!(events, [InputEvent::Focused(false)], "the W not sent going");
        let look: Look = serde_json::from_value(asked("look")).expect("a look");
        let expected = Look {
            viewpoint: Some(Some(api_viewpoint(&game.view.camera.viewpoint()))),
            grid: Some(Some(gg_api::Plane::Xy)),
            ..Look::default()
        };
        assert_eq!(look, expected);
    }

    /// A stop the game answers drops the input meant for it that wasn't sent, but its letting go of
    /// what's held, which a turn from Play to Simulate queued after it; but none while a Play
    /// clicked since waits for its answer, the input then going to the copy that Play starts.
    #[test]
    fn a_stop_drops_the_input_not_sent_but_the_letting_go() {
        let at = [300.0, 200.0];
        let w = Event::Key {
            key: Key::KeyW,
            pressed: true,
            repeat: false,
        };
        let w_down = InputEvent::Key {
            key: "KeyW".to_owned(),
            pressed: true,
            repeat: false,
        };
        // In Play, W, and the camera switch's turn to Simulate, before either is sent.
        let (mut game, ends) = serving();
        let _ = game.route(&w, &reading(at), Some(VIEW));
        game.view.toolbar.world_camera = false;
        game.tell(Some(at), Some(VIEW));
        assert_eq!(game.input, [w_down.clone(), InputEvent::Focused(false)]);
        ends.answer
            .send(("status".to_owned(), status(true, false)))
            .expect("sent");
        game.take_answers();
        assert_eq!(
            game.input,
            [InputEvent::Focused(false)],
            "the W not sent going"
        );
        // In Simulate, Play, and W, before the answer to a status asked before the play, which
        // says another client stopped the game.
        let (mut game, ends) = serving();
        game.view.toolbar.world_camera = false;
        game.tell(Some(at), Some(VIEW));
        game.input.clear();
        game.run(Mode::Play);
        game.tell(Some(at), Some(VIEW));
        let _ = game.route(&w, &reading(at), Some(VIEW));
        ends.answer
            .send(("status".to_owned(), status(true, false)))
            .expect("sent");
        game.take_answers();
        assert!(
            game.input.contains(&w_down),
            "for the copy: {:?}",
            game.input
        );
        assert!(game.view.toolbar.world_camera, "the game's camera");
    }

    /// The editor asks the game how it runs as a game serves, and every quarter second after, a
    /// `status` at a time.
    #[test]
    fn the_game_is_asked_how_it_runs_a_status_at_a_time() {
        let (mut game, ends) = serving();
        let mut cx = Context::seeded(1);
        let mut asks = |game: &mut Game| {
            game.update(&mut cx, Duration::ZERO);
            let asked = ends
                .asked
                .try_iter()
                .filter(|(method, _)| method == "status");
            asked.count()
        };
        // As though the last was asked a quarter second earlier.
        let ago = |game: &mut Game| {
            game.polled = game.polled.and_then(|at| at.checked_sub(STATUS_EVERY));
        };
        assert_eq!(asks(&mut game), 1, "at once");
        ago(&mut game);
        assert_eq!(asks(&mut game), 0, "until the last is answered");
        let answered = || ("status".to_owned(), status(false, false));
        ends.answer.send(answered()).expect("sent");
        assert_eq!(asks(&mut game), 1, "a quarter second on");
        ends.answer.send(answered()).expect("sent");
        assert_eq!(asks(&mut game), 0, "until a quarter second on");
        ago(&mut game);
        assert_eq!(asks(&mut game), 1);
    }

    /// An Open goes to the game at once while the last status said the open scene has no unsaved
    /// edits; refused for edits made since, it prompts, logged but in no message, and Don't Save
    /// asks again, discarding them, with the path the dialog picked.
    #[test]
    fn a_refusal_for_unsaved_edits_prompts_and_dont_save_discards_them() {
        let (mut game, ends) = serving();
        let mut cx = Context::seeded(1);
        let arena = (
            "status".to_owned(),
            scene_status(Some("scenes/arena.ron"), false),
        );
        ends.answer.send(arena).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        let level = PathBuf::from("scenes/level2.ron");
        game.open(level.clone());
        assert_eq!(game.prompt, None, "as the status said");
        game.update(&mut cx, Duration::ZERO);
        let open = |discard| Scene {
            action: SceneAction::Open,
            path: Some(level.clone()),
            discard,
        };
        assert_eq!(scenes(&ends), [open(false)]);
        let unsaved = Failure::new("scene.unsaved", "the open scene has unsaved edits");
        ends.answer
            .send(("scene".to_owned(), Err(unsaved)))
            .expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(game.prompt, Some(Action::Open(level.clone())));
        assert!(game.message.is_none());
        assert_eq!(
            game.logs.back().map(|logged| logged.level),
            Some(Level::Info)
        );
        assert!(!game.files(), "while it prompts");
        game.answer(Choice::DontSave);
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(scenes(&ends), [open(true)]);
    }

    /// The prompt's Save has the game save the open scene, then does what it asked about again,
    /// an Open with the path the dialog picked; a save refused does nothing more, saying why in a
    /// message.
    #[test]
    fn the_prompts_save_saves_then_does_it_again() {
        let (mut game, ends) = serving();
        let mut cx = Context::seeded(1);
        let (arena, level2) = (Some("scenes/arena.ron"), Some("scenes/level2.ron"));
        let status = |scene, unsaved| ("status".to_owned(), scene_status(scene, unsaved));
        ends.answer.send(status(arena, true)).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        let level = PathBuf::from("scenes/level2.ron");
        game.open(level.clone());
        assert_eq!(
            game.prompt,
            Some(Action::Open(level.clone())),
            "as the status said"
        );
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(scenes(&ends), [], "until it's answered");
        game.answer(Choice::Save);
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(scenes(&ends), [save(None)]);
        let saved = ("scene".to_owned(), scene_status(arena, false));
        ends.answer.send(saved).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        let open = Scene {
            action: SceneAction::Open,
            path: Some(level.clone()),
            discard: false,
        };
        assert_eq!(scenes(&ends), [open]);
        let opened = ("scene".to_owned(), scene_status(level2, false));
        ends.answer.send(opened).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        // Edited, and opened again, the save refused.
        ends.answer.send(status(level2, true)).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        game.open(level.clone());
        game.answer(Choice::Save);
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(scenes(&ends), [save(None)]);
        let changed = Failure::new("scene.changed", "scenes/level2.ron changed since");
        ends.answer
            .send(("scene".to_owned(), Err(changed)))
            .expect("sent");
        game.update(&mut cx, Duration::ZERO);
        let message = game.message.take().expect("refused");
        assert_eq!(message.title, "Can't save level2.ron");
        assert_eq!(
            message.text,
            "[scene.changed] scenes/level2.ron changed since"
        );
        assert_eq!(scenes(&ends), [], "nothing more");
        assert!(game.files());
    }

    /// Save As has the game save to the file the dialog picked, with `.ron` after its name unless
    /// it ends in it, in any case, as the XDG portal's dialog may leave it; and refuses a name
    /// with `.ron` after it that a file has already, which the dialog didn't ask about replacing,
    /// saying so.
    #[test]
    fn save_as_ends_the_name_in_ron_and_replaces_no_file_unasked() {
        let dir = std::env::temp_dir().join(format!("gg-editor-save-as-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).expect("made");
        let (mut game, ends) = serving();
        let mut cx = Context::seeded(1);
        for (picked, saved) in [
            ("level", "level.ron"),
            ("level.1", "level.1.ron"),
            ("Boss.RON", "Boss.RON"),
        ] {
            game.save_as(dir.join(picked));
            game.update(&mut cx, Duration::ZERO);
            assert_eq!(scenes(&ends), [save(Some(dir.join(saved)))], "{picked}");
            let answer = ("scene".to_owned(), scene_status(Some(saved), false));
            ends.answer.send(answer).expect("sent");
            game.update(&mut cx, Duration::ZERO);
        }
        std::fs::write(dir.join("kept.ron"), "kept").expect("written");
        game.save_as(dir.join("kept"));
        let message = game.message.take().expect("refused");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(message.title, "Can't save kept.ron");
        assert!(
            message.text.starts_with("[editor.scene_exists]"),
            "{message:?}"
        );
        assert!(game.request.is_none());
    }

    /// Save has the game save the open scene to its file, as the last status named one; refused as
    /// untitled, as after another client's `new`, it goes to Save As, whose dialog the next UI
    /// shows, with no message.
    #[test]
    fn a_save_refused_as_untitled_goes_to_save_as() {
        let (mut game, ends) = serving();
        let mut cx = Context::seeded(1);
        let arena = (
            "status".to_owned(),
            scene_status(Some("scenes/arena.ron"), true),
        );
        ends.answer.send(arena).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        game.choose(&mut cx, Item::Save);
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(scenes(&ends), [save(None)]);
        let untitled = Failure::new("scene.untitled", "the open scene has no file");
        ends.answer
            .send(("scene".to_owned(), Err(untitled)))
            .expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(game.due, Some(Item::SaveAs));
        assert!(game.message.is_none());
        assert!(!game.files(), "until the dialog shows");
    }

    /// A Save refused as untitled while a close is checked goes to no Save As, which would open in
    /// the close's way; the close prompt's own Save, refused so, goes to Save As.
    #[test]
    fn a_save_refused_as_untitled_mid_close_goes_to_save_as_from_the_prompt_alone() {
        let (mut game, ends) = serving();
        let mut cx = Context::seeded(1);
        let status = |unsaved| {
            let scene = scene_status(Some("scenes/arena.ron"), unsaved);
            ("status".to_owned(), scene)
        };
        let untitled = || {
            let untitled = Failure::new("scene.untitled", "the open scene has no file");
            ("scene".to_owned(), Err(untitled))
        };
        ends.answer.send(status(true)).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        game.choose(&mut cx, Item::Save);
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(scenes(&ends), [save(None)]);
        let close = Event::CloseRequested;
        assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
        ends.answer.send(untitled()).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(game.due, None, "while the close is checked");
        // The poll's answer, then the close's status's.
        for _ in 0..2 {
            ends.answer.send(status(true)).expect("sent");
            game.update(&mut cx, Duration::ZERO);
        }
        assert_eq!(game.prompt, Some(Action::Close));
        game.answer(Choice::Save);
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(scenes(&ends), [save(None)]);
        ends.answer.send(untitled()).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(game.due, Some(Item::SaveAs), "the prompt's own Save");
    }

    /// A close is vetoed while a game serves, until the answer to a status asked after it says
    /// whether the open scene has unsaved edits, the answer to one asked before not counting: then
    /// it's prompted for, Cancel going back to none, and Don't Save granting it, which lets the
    /// next close through.
    #[test]
    fn a_close_asks_a_fresh_status_and_prompts_for_unsaved_edits() {
        let (mut game, ends) = serving();
        // Stopped, as it starts, so that it asks for statuses alone.
        (game.stopped, game.told) = (true, false);
        let mut cx = Context::seeded(1);
        let close = Event::CloseRequested;
        let status = |unsaved| {
            let scene = scene_status(Some("scenes/arena.ron"), unsaved);
            ("status".to_owned(), scene)
        };
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(methods(&ends), ["status"], "a poll");
        assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
        game.update(&mut cx, Duration::ZERO);
        assert!(methods(&ends).is_empty(), "a status at a time");
        ends.answer.send(status(false)).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(game.closing, Closing::Checking(true), "the poll's skipped");
        assert_eq!(methods(&ends), ["status"]);
        assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
        ends.answer.send(status(true)).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(game.prompt, Some(Action::Close));
        game.answer(Choice::Cancel);
        assert_eq!(game.closing, Closing::No);
        assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(methods(&ends), ["status"]);
        ends.answer.send(status(true)).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        game.answer(Choice::DontSave);
        assert_eq!(game.closing, Closing::Granted);
        assert_eq!(game.event(&mut cx, &close), ControlFlow::Continue(()));
    }

    /// While no game serves after one has, a close goes by the last status: at once if it said
    /// the open scene has no unsaved edits, and otherwise prompted for without Save, which does
    /// nothing, Don't Save granting it; as a close being checked as the game goes does. Before any
    /// game serves, it goes at once.
    #[test]
    fn a_close_while_no_game_serves_goes_by_the_last_status() {
        let mut cx = Context::seeded(1);
        let close = Event::CloseRequested;
        let mut game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
        assert_eq!(game.event(&mut cx, &close), ControlFlow::Continue(()));
        for checking in [false, true] {
            let (mut game, ends) = serving();
            let unsaved = scene_status(Some("scenes/arena.ron"), true);
            ends.answer
                .send(("status".to_owned(), unsaved))
                .expect("sent");
            game.update(&mut cx, Duration::ZERO);
            if checking {
                assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
            }
            // As `gg run` restarts the game.
            game.disconnect();
            if !checking {
                assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
            }
            assert_eq!(game.prompt, Some(Action::Close), "checking: {checking}");
            game.answer(Choice::Save);
            assert_eq!(game.prompt, Some(Action::Close), "no game to save");
            game.answer(Choice::DontSave);
            assert_eq!(game.event(&mut cx, &close), ControlFlow::Continue(()));
        }
        let (mut game, _ends) = serving();
        game.disconnect();
        assert_eq!(game.event(&mut cx, &close), ControlFlow::Continue(()));
    }

    /// Neither the File menu's request nor a close's status goes to the game while the Inspector
    /// has an edit waiting for it, which goes on a connection of its own; once it has none, both
    /// go, the request first.
    #[test]
    fn nothing_goes_while_the_inspector_has_edits_waiting() {
        let (mut game, ends) = serving();
        // Stopped, as it starts, so that it asks for statuses alone.
        (game.stopped, game.told) = (true, false);
        let mut cx = Context::seeded(1);
        game.update(&mut cx, Duration::ZERO);
        let arena = (
            "status".to_owned(),
            scene_status(Some("scenes/arena.ron"), false),
        );
        ends.answer.send(arena).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(methods(&ends), ["status"], "a poll");
        let id: EntityId = "0123456789abcdef".parse().expect("an ID");
        let x = gg_engine::reflect::Value::F32(1.0);
        game.view.link.set(id, "Transform", "translation.x", x);
        game.choose(&mut cx, Item::Save);
        assert_eq!(
            game.event(&mut cx, &Event::CloseRequested),
            ControlFlow::Break(())
        );
        game.update(&mut cx, Duration::ZERO);
        assert!(methods(&ends).is_empty(), "the edit first");
        assert_eq!(game.closing, Closing::Checking(false));
        // The Inspector's pass sends the set, and the game answers it, for which a test gives the
        // layer a link with nothing waiting.
        game.view.link = Inspector::remote(CLIENT).link();
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(methods(&ends), ["scene", "status"]);
        assert_eq!(game.closing, Closing::Checking(true));
    }

    /// A New or an Open turns the view to the game's camera, as the editor starts, so that it
    /// frames the scene the game has now: one that stopped a copy playing, though the stop turns
    /// the view to the editor's camera, as another client's `stop` does; and one from the editor's
    /// camera, as a Stop leaves the view, the next look giving its viewpoint and the grid back to
    /// the game. Not one refused, nor one whose answer comes before that of a Simulate clicked
    /// since. And the title bar names the open scene's file once a game has served, Untitled after
    /// a New, marked while it has unsaved edits.
    #[test]
    fn a_new_or_an_open_turns_the_view_to_the_games_camera() {
        let game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
        assert_eq!(game.scene_label(), None, "before a game serves");
        // Playing, through the game's camera.
        let (mut game, ends) = serving();
        let mut cx = Context::seeded(1);
        let arena = scene_status(Some("scenes/arena.ron"), true);
        let playing = arena.map(|mut status| {
            status["stopped"] = json!(false);
            status
        });
        ends.answer
            .send(("status".to_owned(), playing))
            .expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(game.scene_label().as_deref(), Some("arena.ron*"));
        assert_eq!(game.mode(), Mode::Play);
        game.act(Action::New);
        game.answer(Choice::DontSave);
        game.update(&mut cx, Duration::ZERO);
        let new = Scene {
            action: SceneAction::New,
            path: None,
            discard: true,
        };
        assert_eq!(scenes(&ends), [new]);
        let emptied = ("scene".to_owned(), scene_status(None, false));
        ends.answer.send(emptied).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(game.mode(), Mode::Edit);
        assert!(game.view.toolbar.world_camera, "the game's camera");
        assert_eq!(game.scene_label().as_deref(), Some("Untitled"));
        // Through the editor's camera, for which a test sets the switch, the game's look holding
        // its viewpoint and the grid.
        game.view.toolbar.world_camera = false;
        game.update(&mut cx, Duration::ZERO);
        let sent = looks(&ends);
        let [sent] = &sent[..] else {
            panic!("a look: {sent:?}");
        };
        let (Some(viewpoint), Some(grid)) = (sent.viewpoint.flatten(), sent.grid.flatten()) else {
            panic!("the editor's viewpoint and grid: {sent:?}");
        };
        let held = Looked {
            viewpoint: Some(viewpoint),
            outline: None,
            grid: Some(grid),
            colliders: None,
            by: By {
                viewpoint: Some(CLIENT.to_owned()),
                outline: None,
                grid: Some(CLIENT.to_owned()),
                colliders: None,
            },
            revision: 1,
            run: 1,
            seen: None,
        };
        ends.answer
            .send(("look".to_owned(), Ok(json!(held))))
            .expect("sent");
        let level = PathBuf::from("scenes/level2.ron");
        let opened = || {
            let level2 = scene_status(Some("scenes/level2.ron"), false);
            ("scene".to_owned(), level2)
        };
        game.open(level.clone());
        game.update(&mut cx, Duration::ZERO);
        let missing = Failure::new("scene.no_file", "scenes/level2.ron isn't there");
        ends.answer
            .send(("scene".to_owned(), Err(missing)))
            .expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert!(game.message.take().is_some(), "refused");
        assert!(!game.view.toolbar.world_camera, "refused");
        // A Simulate clicked after the Open was asked, which its answer comes before.
        game.open(level.clone());
        game.update(&mut cx, Duration::ZERO);
        game.run(Mode::Simulate);
        ends.answer.send(opened()).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert!(!game.view.toolbar.world_camera, "as Simulate turned it");
        ends.answer
            .send(("play".to_owned(), status(false, false)))
            .expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(game.mode(), Mode::Simulate);
        game.run(Mode::Edit);
        ends.answer
            .send(("stop".to_owned(), status(true, false)))
            .expect("sent");
        game.update(&mut cx, Duration::ZERO);
        let _ = looks(&ends);
        game.open(level);
        game.update(&mut cx, Duration::ZERO);
        ends.answer.send(opened()).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        assert!(game.view.toolbar.world_camera, "the game's camera");
        let back = Look {
            viewpoint: Some(None),
            grid: Some(None),
            ..Look::default()
        };
        assert_eq!(looks(&ends), [back]);
    }

    /// In Play, a press or a repeat of N, O or S with the command key held, a key of the File
    /// menu's shortcuts, goes neither to the game nor to the view's tools; its release does, and so
    /// do other keys with it, and those keys without it.
    #[test]
    fn the_file_menus_shortcuts_keys_dont_reach_the_game() {
        let (mut game, _ends) = serving();
        let at = [300.0, 200.0];
        let mut command = reading(at);
        command.command = true;
        let key = |key, pressed, repeat| Event::Key {
            key,
            pressed,
            repeat,
        };
        for event in [
            key(Key::KeyS, true, false),
            key(Key::KeyS, true, true),
            key(Key::KeyN, true, false),
            key(Key::KeyO, true, false),
        ] {
            let routed = game.route(&event, &command, Some(VIEW));
            assert_eq!(routed, ControlFlow::Continue(()), "{event:?}");
        }
        assert_eq!(game.input, []);
        let _ = game.route(&key(Key::KeyS, false, false), &command, Some(VIEW));
        let _ = game.route(&key(Key::KeyW, true, false), &command, Some(VIEW));
        let _ = game.route(&key(Key::KeyS, true, false), &reading(at), Some(VIEW));
        let sent = |key: &str, pressed| InputEvent::Key {
            key: key.to_owned(),
            pressed,
            repeat: false,
        };
        let expected = [sent("KeyS", false), sent("KeyW", true), sent("KeyS", true)];
        assert_eq!(game.input, expected);
    }

    /// Started outside a project, with none opened before, the layer opens none as it attaches,
    /// and runs nothing: the hub shows, the File menu's items on the open scene and the Scenes
    /// panel doing nothing, and the Assets and Scenes panels have no folder to show.
    #[test]
    fn outside_a_project_it_runs_nothing_and_shows_the_hub() {
        let mut game = Game::new(None, Inspector::remote(CLIENT).link());
        game.attach(&mut Context::seeded(1));
        assert!(game.run.is_none() && game.talker.is_none() && game.dir.is_none());
        assert!(game.hub_shows() && !game.files());
        assert_eq!(game.label(), "No project open");
        assert!(game.assets.is_none() && game.scenes.is_none());
    }

    /// The hub's pick of another project goes as a close does: at once before any game has served;
    /// otherwise, while the open scene has unsaved edits, as a fresh status says, the prompt asks
    /// whether to save them before that project opens, Cancel keeping the project open, and Don't
    /// Save granting the switch, to that project rather than the editor's close. A close of the
    /// editor while the prompt shows is the editor's own. A pick of the project open opens
    /// nothing, the hub giving way to it.
    #[test]
    fn another_project_opens_as_a_close_goes() {
        let other = PathBuf::from("other");
        let mut fresh = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
        fresh.switch_to(other.clone());
        assert_eq!(fresh.closing, Closing::Granted, "no game has served");
        assert_eq!(fresh.switching.as_ref(), Some(&other));
        let mut cx = Context::seeded(1);
        let status = |unsaved| {
            let scene = scene_status(Some("scenes/arena.ron"), unsaved);
            ("status".to_owned(), scene)
        };
        let prompted = |game: &mut Game, ends: &Ends, cx: &mut Context| {
            game.switch_to(other.clone());
            game.update(cx, Duration::ZERO);
            assert_eq!(methods(ends), ["status"]);
            ends.answer.send(status(true)).expect("sent");
            game.update(cx, Duration::ZERO);
            assert_eq!(game.prompt, Some(Action::Close));
        };
        let (mut game, ends) = serving();
        (game.stopped, game.told) = (true, false);
        game.hub.asked = true;
        game.switch_to(PathBuf::from("."));
        assert!(
            !game.hub.asked && game.closing == Closing::No,
            "the project open"
        );
        prompted(&mut game, &ends, &mut cx);
        let (_, text) = game.asks(&Action::Close);
        assert!(text.contains("the project other opens"), "{text}");
        game.answer(Choice::Cancel);
        assert_eq!((game.closing, game.switching.as_ref()), (Closing::No, None));
        prompted(&mut game, &ends, &mut cx);
        game.answer(Choice::DontSave);
        assert_eq!(game.closing, Closing::Granted);
        assert_eq!(game.switching.as_ref(), Some(&other));

        let (mut game, ends) = serving();
        (game.stopped, game.told) = (true, false);
        prompted(&mut game, &ends, &mut cx);
        let close = Event::CloseRequested;
        assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
        assert_eq!(game.switching, None);
        let (_, text) = game.asks(&Action::Close);
        assert!(text.contains("the editor closes"), "{text}");
        game.answer(Choice::DontSave);
        assert_eq!(game.event(&mut cx, &close), ControlFlow::Continue(()));
    }

    /// A part of the look goes to the game only as the user changes it: not as the editor's camera
    /// adopts the frames' viewpoints through the game's camera, nor as it takes another client's
    /// viewpoint while it drives, nor as it settles after; nor as the Inspector makes the pick
    /// another client's outline asks for. A wheel, while the game is stopped, turns the view to the
    /// editor's camera, from where the frames left it, whose viewpoint goes, with the grid; the
    /// switch back to the game's camera gives both back; and a pick of the user's goes as the
    /// outline, though the Inspector makes it in the pass that makes the outline's.
    #[test]
    fn the_look_goes_to_the_game_as_the_user_changes_it() {
        let (mut game, ends) = serving();
        // As it starts, so that the wheel is the editor's camera's.
        (game.stopped, game.told) = (true, false);
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

    /// While the game hasn't answered the last `input`, as while a script has stopped it, the
    /// cursor's moves queued take each other's places, the presses between them kept in order, so
    /// that what goes once it answers grows with the presses alone.
    #[test]
    fn the_cursors_moves_queued_for_the_game_take_each_others_places() {
        let (mut game, _ends) = serving();
        game.inputting = true;
        let at = |i: u32| [101.0 + f64::from(i % 400), 60.0];
        let w = Event::Key {
            key: Key::KeyW,
            pressed: true,
            repeat: false,
        };
        for i in 0..1_000 {
            if i == 500 {
                let _ = game.route(&w, &reading(at(i)), Some(VIEW));
            }
            let [x, y] = at(i);
            let moved = Event::CursorMoved { x, y };
            let _ = game.route(&moved, &reading(at(i)), Some(VIEW));
        }
        let w_down = InputEvent::Key {
            key: "KeyW".to_owned(),
            pressed: true,
            repeat: false,
        };
        let in_game = |[x, y]: [f64; 2]| InputEvent::Cursor(in_view([x, y], VIEW.origin));
        assert_eq!(game.input, [in_game(at(499)), w_down, in_game(at(999))]);
    }

    /// A game that asks for the cursor grabbed has the editor ask for its own in Play: Escape lets
    /// go of it, though the game asks still, until a press in the view, the game getting both;
    /// Simulate lets go; and the mouse's raw motion queued for the game adds up.
    #[test]
    fn a_games_grab_holds_the_editors_cursor_in_play_until_escape() {
        let (mut game, _ends) = serving();
        let reader = Reader::create(4, 4).expect("memory");
        let writer = Writer::open(reader.name()).expect("opened");
        game.reader = Some(reader);
        let mut cx = Context::seeded(1);
        game.update(&mut cx, Duration::ZERO);
        assert!(!cx.cursor_grabbed(), "before the game asks");
        writer.set_cursor_grabbed(true);
        game.update(&mut cx, Duration::ZERO);
        assert!(cx.cursor_grabbed());
        // As while the game hasn't answered the last `input`, so that what's queued stays.
        game.inputting = true;
        let at = reading([300.0, 200.0]);
        for (x, y) in [(2.0, 1.0), (-0.5, 3.0)] {
            let _ = game.route(&Event::MouseMotion { x, y }, &at, Some(VIEW));
        }
        let escape = Event::Key {
            key: Key::Escape,
            pressed: true,
            repeat: false,
        };
        let _ = game.route(&escape, &at, Some(VIEW));
        game.update(&mut cx, Duration::ZERO);
        assert!(!cx.cursor_grabbed(), "let go at Escape");
        let press = Event::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        };
        let _ = game.route(&press, &at, Some(VIEW));
        game.update(&mut cx, Duration::ZERO);
        assert!(cx.cursor_grabbed(), "grabbed again at a press in the view");
        let escaped = InputEvent::Key {
            key: "Escape".to_owned(),
            pressed: true,
            repeat: false,
        };
        let pressed = InputEvent::Button {
            button: gg_api::Button::Left,
            pressed: true,
        };
        assert_eq!(
            game.input,
            [InputEvent::Motion([1.5, 4.0]), escaped, pressed]
        );
        game.view.toolbar.world_camera = false;
        game.update(&mut cx, Duration::ZERO);
        assert!(!cx.cursor_grabbed(), "Simulate lets go");
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
        let mut game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
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

    /// The Logs panel, holding all it keeps of more logged, the oldest gone, lays out only the
    /// rows in view, a row to each line, and fills itself with the latest lines in order, entries
    /// of more than one line among them, the last at its bottom.
    #[test]
    fn the_logs_lay_out_only_the_rows_in_view_and_show_the_latest() {
        let mut game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
        // An entry of one to three lines, each its own text; but the one before the last, of more
        // than the panel holds, so that its top row is inside an entry.
        let logged = LOGGED + 1000;
        let entry = |at: usize| {
            let lines = if at == logged - 2 { 40 } else { at % 3 + 1 };
            (0..lines).map(move |line| format!("{at}.{line}"))
        };
        for at in 0..logged {
            game.log(Level::Info, entry(at).collect::<Vec<_>>().join("\n"));
        }
        let lines: Vec<String> = (0..logged).flat_map(entry).collect();
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0));
        let panel = egui::Rect::from_min_max(egui::pos2(0.0, 100.0), egui::pos2(400.0, 300.0));
        let frame = || {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..egui::RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let builder = egui::UiBuilder::new().max_rect(panel);
                ui.scope_builder(builder, |ui| show_logs(ui, &game.logs));
            });
            output.textures_delta.clear();
            output.shapes
        };
        // The first finds how tall its rows are, the bottom of which the second scrolls to, and
        // the third lays out only what the second did.
        frame();
        frame();
        let mut shown = texts(&frame());
        shown.sort_by(|(_, a), (_, b)| a.top().total_cmp(&b.top()));
        assert!(shown.len() > 10, "{shown:?}");
        let laid_out = ctx.fonts(|fonts| fonts.num_galleys_in_cache());
        assert!(laid_out <= 4 * shown.len(), "{laid_out} laid out");
        let texts: Vec<_> = shown.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(texts, lines[lines.len() - shown.len()..]);
        let pitch = shown[1].1.top() - shown[0].1.top();
        let (top, bottom) = (shown[0].1.top(), shown[shown.len() - 1].1.bottom());
        assert!(top < panel.top() + pitch && bottom > panel.bottom() - pitch);
    }

    #[test]
    fn a_build_says_where_it_has_come_to_in_the_title_bar() {
        let mut game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
        let send = game.send.take().expect("not started");
        assert_eq!(game.label(), "Building…");
        let building = |mut event: Value| {
            event["event"] = json!("building");
            event
        };
        let events = [
            (
                building(json!({"stage": "starting", "crates": 293})),
                "Building… starting",
            ),
            (
                building(
                    json!({"stage": "compiling", "compiled": 212, "crates": 293, "crate": "gg_engine"}),
                ),
                "Building… 212 of 293 crates",
            ),
            (building(json!({"stage": "linking"})), "Building… linking"),
            (
                json!({"event": "built", "build_ms": 14580}),
                "Building… linking",
            ),
            (json!({"event": "started", "pid": 7}), "Starting…"),
            (
                json!({"event": "rebuilding", "file": "src/main.rs"}),
                "Rebuilding…",
            ),
            (
                building(json!({"stage": "extracting_assets"})),
                "Rebuilding… extracting assets",
            ),
        ];
        let count = events.len();
        for (event, label) in events {
            send.send(Line::Out(event.to_string())).expect("sent");
            game.take_lines();
            assert_eq!(game.label(), label, "after {event}");
        }
        assert_eq!(game.logs.len(), count, "each reaches the Logs too");
    }

    /// The game's events reach the Logs as they're read, each with its code, its message and its
    /// fields, but where and when it was logged, at its level; but `gg run`'s notes of a patch that
    /// failed, which its own events show.
    #[test]
    fn the_games_events_reach_the_logs_with_their_codes_and_fields() {
        let mut game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
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
        let failed = r#"{"level":"WARN","code":"gg.patch_failed","client":"gg",
            "message":"A patch of src/main.rs failed"}"#;
        let attached = r#"{"level":"WARN","code":"gg.attach_changed","client":"gg",
            "message":"A patch changed a layer's attach"}"#;
        let events = [warned, traced, failed, attached];
        let events = events.map(|line| serde_json::from_str(line).expect("JSON"));
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
            (
                Level::Warn,
                "[gg.attach_changed] A patch changed a layer's attach  client=gg",
            ),
        ];
        assert_eq!(logs, expected);
    }

    /// The events of a drag with the primary button from `from` to `to`, a frame's each, as egui
    /// gets them: past how far a click goes in the frame after the press, then to `to`, then let go.
    fn drag(from: egui::Pos2, to: egui::Pos2) -> Vec<egui::Event> {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![
            egui::Event::PointerMoved(from),
            button(from, true),
            egui::Event::PointerMoved(from + egui::vec2(12.0, 0.0)),
            egui::Event::PointerMoved(to),
            egui::Event::PointerMoved(to + egui::vec2(1.0, 1.0)),
            button(to + egui::vec2(1.0, 1.0), false),
        ]
    }

    /// A texture dragged from the Assets panel outlines the view once it's held over it, until
    /// it's let go there, which asks the game for a pick where it was let go, in the view's pixels.
    /// One let go elsewhere asks nothing, nor does a file that isn't a texture or a model, which
    /// doesn't drag.
    #[test]
    fn a_texture_dragged_from_the_assets_panel_is_let_go_on_the_view() {
        let scratch = Scratch::new("dragged");
        for file in ["stone.png", "notes.txt"] {
            fs::write(scratch.0.join(file), "").expect("written");
        }
        let (mut game, ends) = serving();
        game.folders = Some(folders(&scratch.0));
        game.assets = Some(Assets::new(scratch.0.clone()));
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 400.0));
        let (panel, view) = screen.split_left_right_at_x(400.0);
        let place = Viewport {
            origin: [400, 0],
            size: [400, 400],
        };
        // A frame of the panel beside a view that senses presses as the dock's does.
        let frame = |game: &mut Game, event: Option<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events: event.into_iter().collect(),
                ..egui::RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let sense = egui::Sense::CLICK | egui::Sense::DRAG;
                let response = ui.interact(view, egui::Id::unique("test view"), sense);
                if let Some(assets) = &mut game.assets {
                    let builder = egui::UiBuilder::new().max_rect(panel);
                    ui.scope_builder(builder, |ui| assets.show(ui));
                }
                game.take_drop(ui.ctx(), &response, Some(place), 1.0);
            });
            output.textures_delta.clear();
            output.shapes
        };
        // The drag from `from` to `to`, and whether each of its frames outlined the view.
        let drop = |game: &mut Game, from: egui::Rect, to| {
            let frames = drag(from.center(), to).into_iter();
            let shapes = frames.map(|event| frame(game, Some(event)));
            let outlined = |shapes: Vec<egui::epaint::ClippedShape>| {
                let outline = |shape: &_| matches!(shape, egui::Shape::Rect(r) if r.rect == view);
                shapes.iter().any(|clipped| outline(&clipped.shape))
            };
            shapes.map(outlined).collect::<Vec<bool>>()
        };
        frame(&mut game, None);
        let shown = texts(&frame(&mut game, None));
        let (stone, notes) = (find(&shown, "stone.png"), find(&shown, "notes.txt"));
        let elsewhere = egui::pos2(200.0, 350.0);
        for (from, to) in [(notes, view.center()), (stone, elsewhere)] {
            assert_eq!(drop(&mut game, from, to), [false; 6]);
            assert!(ends.picks.try_recv().is_err());
        }
        // Outlined once it's over the view, until it's let go.
        let outlined = drop(&mut game, stone, view.center());
        assert_eq!(outlined, [false, false, false, true, true, true]);
        let (ticket, at) = ends.picks.try_recv().expect("a pick");
        // Let go a pixel right of and below the view's middle, (600, 200).
        assert_eq!(at, [201.0, 201.0]);
        assert_eq!(game.dropping.map(|dropping| dropping.ticket), Some(ticket));
    }

    /// A texture let go over the view asks the game for a pick there; the entity the answer finds
    /// is picked, and its sprite's texture set through the link, as an edit of its own, to the
    /// texture's ID, as its ID file gives it, or without one, its path from the asset root. Where
    /// no entity draws, nothing is set, and the Logs say so; and of two let go before an answer,
    /// the latest alone is set.
    #[test]
    fn a_texture_let_go_on_the_view_sets_the_texture_of_the_sprite_there() {
        let scratch = Scratch::new("texture-let-go");
        let textures = scratch.0.join("textures");
        fs::create_dir(&textures).expect("made");
        for file in ["stone.png", "new.png"] {
            fs::write(textures.join(file), "").expect("written");
        }
        let id = "5f3c9a1e07b2d846";
        let meta = Meta {
            id: id.parse().expect("an ID"),
        };
        fs::write(Meta::path(&textures.join("stone.png")), meta.text()).expect("written");
        let (mut game, ends) = serving();
        game.folders = Some(folders(&scratch.0));
        let texture = |name: &str| Dragged {
            path: textures.join(name),
            kind: AssetKind::Texture,
        };
        let entity: EntityId = "0123456789abcdef".parse().expect("an ID");
        let found = |id: Option<EntityId>| gg_api::Picked {
            id: id.map(|id| id.to_string()),
            name: id.map(|_| "Pillar".to_owned()),
        };
        let set = |name: &str| {
            let texture = reflect::Value::String(name.to_owned());
            let texture = reflect::Value::Option(Some(Box::new(texture)));
            Some((entity, "Sprite".to_owned(), "texture".to_owned(), texture))
        };
        game.let_go(&texture("stone.png"), [300.0, 200.0], Some(VIEW), SCALE);
        let (ticket, at) = ends.picks.try_recv().expect("a pick");
        // [`VIEW`]'s top left is at (100, 50).
        assert_eq!(at, [200.0, 150.0]);
        ends.found
            .send((ticket, Ok(found(Some(entity)))))
            .expect("sent");
        game.take_picks();
        assert_eq!(game.view.link.pending(), [set(id), None]);
        assert_eq!(game.dropping, None);
        // Where no entity draws.
        game.let_go(&texture("new.png"), [300.0, 200.0], Some(VIEW), SCALE);
        let (ticket, _) = ends.picks.try_recv().expect("a pick");
        ends.found.send((ticket, Ok(found(None)))).expect("sent");
        game.take_picks();
        assert_eq!(game.view.link.pending().len(), 2, "nothing more set");
        let said = game.logs.back().map(|logged| logged.text.as_str());
        assert_eq!(said, Some("new.png was let go where no entity draws"));
        // Two before an answer: the latest's alone.
        game.let_go(&texture("stone.png"), [300.0, 200.0], Some(VIEW), SCALE);
        game.let_go(&texture("new.png"), [300.0, 200.0], Some(VIEW), SCALE);
        let tickets: Vec<u64> = ends.picks.try_iter().map(|(ticket, _)| ticket).collect();
        for ticket in tickets {
            ends.found
                .send((ticket, Ok(found(Some(entity)))))
                .expect("sent");
        }
        game.take_picks();
        let pending = game.view.link.pending();
        assert_eq!(pending[2..], [set("textures/new.png"), None]);
    }

    /// A font let go over the view, as a texture is, sets the font of the text the game's view
    /// shows there through the link, as an edit of its own: by its path from the asset root,
    /// without an ID file.
    #[test]
    fn a_font_let_go_on_the_view_sets_the_font_of_the_text_there() {
        let scratch = Scratch::new("font-let-go");
        let (mut game, ends) = serving();
        game.folders = Some(folders(&scratch.0));
        let title = Dragged {
            path: scratch.0.join("fonts").join("title.ttf"),
            kind: AssetKind::Font,
        };
        let entity: EntityId = "0123456789abcdef".parse().expect("an ID");
        game.let_go(&title, [300.0, 200.0], Some(VIEW), SCALE);
        let (ticket, _) = ends.picks.try_recv().expect("a pick");
        let found = gg_api::Picked {
            id: Some(entity.to_string()),
            name: Some("Target".to_owned()),
        };
        ends.found.send((ticket, Ok(found))).expect("sent");
        game.take_picks();
        let font = reflect::Value::String("fonts/title.ttf".to_owned());
        let font = reflect::Value::Option(Some(Box::new(font)));
        let set = Some((entity, "Text".to_owned(), "font".to_owned(), font));
        assert_eq!(game.view.link.pending(), [set, None]);
    }

    /// A model let go over the view is a root the link creates, named after its file, by its path
    /// from the asset root without an ID file, where the view's last frame shows the grid's plane
    /// there: through the editor's camera as it starts, the plane the 2D games are in. In
    /// perspective, where the ray meets the ground in front of the camera, and above the horizon,
    /// where it doesn't, on the plane through the camera's pivot facing the view. Before a frame
    /// has come, it lands nowhere, and the Logs say so.
    #[test]
    fn a_model_let_go_on_the_view_lands_where_the_cursor_shows_the_grids_plane() {
        let scratch = Scratch::new("model-let-go");
        let (mut game, _ends) = serving();
        game.folders = Some(folders(&scratch.0));
        let rock = Dragged {
            path: scratch.0.join("models").join("rock.gltf"),
            kind: AssetKind::Model,
        };
        game.let_go(&rock, [560.0, 290.0], Some(VIEW), SCALE);
        assert!(game.view.link.creates().is_empty());
        let said = game.logs.back().map(|logged| logged.text.as_str());
        let why = "the view has shown no frame to place it in yet";
        assert_eq!(
            said,
            Some(format!("rock.gltf was let go, but {why}").as_str())
        );
        // Where a create put it, its name and its model as they're given.
        let landed = |game: &Game| {
            let creates = game.view.link.creates();
            let last = creates.last().expect("a create");
            let [(name, named), (transform, placed), (root, rooted)] = &last[..] else {
                panic!("a name, a transform and a model's root: {creates:?}");
            };
            let fields = [name, transform, root].map(|ident| ident.as_ref());
            assert_eq!(fields, ["Name", "Transform", "ModelRoot"]);
            assert_eq!(*named, Name("rock".to_owned()).to_value());
            let model = reflect::Value::String("models/rock.gltf".to_owned());
            assert_eq!(*rooted, reflect::Value::Struct(vec![("0".into(), model)]));
            Transform::from_value(placed)
                .expect("a transform")
                .translation
        };
        // As the editor's camera starts: the origin at (500, 350), and a unit 60 pixels across.
        game.seen = Some(game.view.camera.viewpoint());
        game.size = Some(VIEW.size);
        game.let_go(&rock, [560.0, 290.0], Some(VIEW), SCALE);
        let at = landed(&game);
        assert!(at.abs_diff_eq(Vec3::new(1.0, 1.0, 0.0), 1e-4), "{at}");
        // Looking along the ground, 1 unit above it.
        let along = Viewpoint {
            position: Vec3::new(0.0, 1.0, 10.0),
            rotation: Quat::IDENTITY,
            projection: Projection::Perspective {
                fov: 1.0,
                near: 0.1,
            },
        };
        game.seen = Some(along);
        game.view.camera.adopt(&along);
        game.view.toolbar.plane = Plane::XZ;
        // Below the view's middle, (500, 350), and above it.
        game.let_go(&rock, [500.0, 500.0], Some(VIEW), SCALE);
        let ground = landed(&game);
        assert!(ground.y.abs() < 1e-4 && ground.z < 10.0, "{ground}");
        game.let_go(&rock, [500.0, 200.0], Some(VIEW), SCALE);
        let screen = game.screen(Some(VIEW)).expect("a frame");
        assert_eq!(
            landed(&game),
            game.view.camera.under([500.0, 200.0], screen)
        );
        assert_eq!(game.view.link.creates().len(), 3);
    }

    /// A prefab let go over the view is an instance's root the link creates where it lands, as a
    /// model's is: its transform there and its link, by its asset ID where its ID file holds one,
    /// without a name, which it takes from its prefab's root; and before a frame has come, nowhere.
    #[test]
    fn a_prefab_let_go_on_the_view_is_an_instance_where_it_lands() {
        let scratch = Scratch::new("prefab-let-go");
        let (mut game, _ends) = serving();
        game.folders = Some(folders(&scratch.0));
        let prefabs = scratch.0.join("prefabs");
        fs::create_dir(&prefabs).expect("made");
        let file = prefabs.join("lamp.prefab");
        fs::write(&file, "(\n    entities: {},\n)\n").expect("written");
        let id = "5f3c9a1e07b2d846";
        fs::write(
            Meta::path(&file),
            Meta {
                id: id.parse().expect("an ID"),
            }
            .text(),
        )
        .expect("written");
        let lamp = Dragged {
            path: file,
            kind: AssetKind::Prefab,
        };
        game.let_go(&lamp, [560.0, 290.0], Some(VIEW), SCALE);
        assert!(game.view.link.creates().is_empty(), "no frame yet");
        game.seen = Some(game.view.camera.viewpoint());
        game.size = Some(VIEW.size);
        game.let_go(&lamp, [560.0, 290.0], Some(VIEW), SCALE);
        let creates = game.view.link.creates();
        let [(transform, placed), (root, linked)] = &creates[0][..] else {
            panic!("a transform and a link: {creates:?}");
        };
        assert_eq!(
            [transform, root].map(|i| i.as_ref()),
            ["Transform", "PrefabRoot"]
        );
        let name = reflect::Value::String(id.to_owned());
        assert_eq!(*linked, reflect::Value::Struct(vec![("0".into(), name)]));
        let at = Transform::from_value(placed)
            .expect("a transform")
            .translation;
        assert!(at.abs_diff_eq(Vec3::new(1.0, 1.0, 0.0), 1e-4), "{at}");
    }
}
