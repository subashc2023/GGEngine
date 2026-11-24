//! The editor's game layer: the project's game, run as an agent runs it, and every panel, menu and
//! control. The keys are in [`SHORTCUTS`](crate::shortcuts::SHORTCUTS).
//!
//! # The run
//!
//! - `gg run --embedded` in the project's directory, by the gg of the editor's engine
//!   ([`crate::gg`]): it builds the game, patches in code edits, and restarts after other changes.
//!   With `--release-game`, `gg run --release --embedded`: optimized, without patches.
//! - The editor closes `gg run`'s stdin as it ends, which ends the run.
//! - The game starts stopped (`--stopped`). Until Play, only edits, scene saves and File > New and
//!   Open change its world, the world as edited.
//! - It renders offscreen. The Viewport shows its view from memory they share, as the tool API's
//!   `view` asks.
//!
//! # Panels and the title bar
//!
//! - Entities and Inspector show and edit the game's world through the API, as a remote
//!   [`Inspector`].
//! - Logs: `gg run`'s events, build errors among them; the game's events, as the API's `logs`
//!   follows them; and what the game and the build print.
//! - Assets: the asset root's files. [Scenes](crate::scenes). [Debugger](crate::debugger): the
//!   scripts.
//! - The title bar shows how the run goes, and while a game serves, whether it's stopped, playing
//!   or simulating, and paused. It learns that from the game's answers and a `status` every quarter
//!   second, which catches other clients' changes and a wait at a script's stop.
//! - Its middle holds Play, Simulate, Stop and Step, then Render, which path traces the view
//!   ([`overlay::show_run`]).
//!
//! # The hub
//!
//! - Started outside a project, the editor shows the [`hub`] over its panels, unless it opens the
//!   latest at start. File > Open Project shows it over the open project; Back returns.
//! - A project picked replaces the open one as a close does, asking first to save unsaved edits.
//!   The old run goes on ending as the new one starts; in the same project, the new one waits for
//!   it. The view is empty until the new game's first frame.
//!
//! # Play, Simulate and Stop
//!
//! They're Unreal's Play and Simulate In Editor; switching between them is its Possess and Eject.
//!
//! - Play plays a copy of the world as edited in its place, as the API's `play` does, through the
//!   game's camera. The game gets all the view's input; the view's tools get only what lets go of
//!   what they hold.
//! - Simulate plays it through the editor's camera; the game gets no input.
//! - While the game plays, Simulate and the toolbar's camera switch swap the two, carrying on: the
//!   camera and the input move over, the game letting go of what's held.
//! - Play from stopped plays a game paused before its stop in real time. Playing, Play is Pause;
//!   paused, Resume.
//! - Step runs a tick while paused. Stopped, it first plays the game paused, through the camera the
//!   view shows.
//! - Stop ends the input at once and brings back the world as edited with its edits' history, as
//!   `stop` does. The view goes to the editor's camera, as at any client's `stop` that no Play
//!   clicked since follows.
//! - A game's Quit ([`Context::exit`]) in Play or Simulate stops it, as Stop does.
//! - Stopped, the camera switch shows the game's camera, as at start and after a New or an Open.
//!   The input stays the editor's; the view turns to the editor's camera once that takes some.
//!
//! # The cursor and the keys
//!
//! - In Play, a game that asks for the cursor grabbed, as the view's memory says, has the editor
//!   grab it while the view has the keys. The mouse's raw motion goes to the game.
//! - Shift+F1 lets go, as in Unreal: a game may not let go itself. The game never gets F1's press.
//!   Escape is the game's, as for its menu. A press in the view grabs again. Simulate and Stop let
//!   go.
//! - Play, and Resume in Play, give the view the keys their click took, and grab again what
//!   Shift+F1 let go of. A game that doesn't ask keeps a free cursor, as for a menu it clicks.
//! - In Play, an outline round the view says it has the keys.
//! - The editor's camera grabs the cursor while it looks about, orbits or dollies
//!   ([`camera`](crate::camera)).
//!
//! # Picking and handles
//!
//! - A click in the view, but in Play, picks what the game's view shows there, as the API's `pick`
//!   finds it. With Ctrl (Cmd on macOS) or Shift, it adds it to what's picked, or takes it out.
//! - A click on a model picks its root. While the root or one of its nodes was picked last, it
//!   picks the node there, so a second click goes into a model.
//! - Through the editor's camera, the [handles](crate::handles) at the entity picked last move,
//!   turn and scale all those picked, each about its own origin; one under another picked is
//!   carried by its drag. Move's and Rotate's go along the toolbar's space; Scale's along the
//!   entity's own axes.
//! - A drag is one edit, through the Inspector's link, which its Edit menu undoes. No handles show
//!   while one they'd move is a model's node, which the game refuses to change.
//! - The view's bottom left names what's under the cursor.
//! - Probe reads what shading took and made at the pixel clicked, through [`probe`](gg_api::probe),
//!   as `gg probe` does, pausing a playing game for its captures. The numbers show beside the pixel
//!   until Escape, another probe or another tool.
//!
//! # The look and frame rate limits
//!
//! - The editor's camera, the pick's outline, the grid, the colliders' switch, what the view shows
//!   in place of its light, and the passes left out go to the game as parts of its
//!   [`look`](crate::look) as the user changes them. The editor takes the parts another client,
//!   such as an agent, sets.
//! - The view's list shows, after shading's parts, the images the game's frame makes, as the API's
//!   `images` answers as the list opens.
//! - The title bar's settings limit the editor's frame rate, at the display's by default, and the
//!   game's, as the look's `max_fps`, and its render scale, as the look's `render_scale`: the
//!   editor takes both from another client too.
//! - The Performance panel shows the game's frames first, as each frame's memory carries them.
//!
//! # Drag and drop
//!
//! A texture, model, font or prefab dragged from Assets outlines the view while it's over it. Let
//! go there, it's an edit through the Inspector's link, by asset ID, and the panels show the result
//! picked.
//!
//! - A texture sets the texture of the sprite there, as `pick` finds it; a font, the font of the
//!   text there. The Inspector shows a refusal, as for an entity without one.
//! - A model is a root named after its file, where the cursor shows the grid's plane
//!   ([`landing`](Game::landing)).
//! - A prefab is a linked instance's root there; the game spawns its entities under it.
//!
//! # The File menu
//!
//! - It acts on the game's open scene through the API's `scene`, never touching a scene file
//!   itself. Each request, and a close's status, goes once the Inspector has nothing waiting, so
//!   the game has the edits first.
//! - New empties the scene. Open opens one the OS's dialog picks, as a click in Scenes does. Save
//!   and Save As save the world as edited to its file, or to one the dialog picks, which becomes
//!   its file.
//! - The title bar names the open scene's file, marked while the game says it has unsaved edits.
//!   New, Open, Quit and the window's close then ask whether to save, discard, or cancel. A close
//!   asks the game for a status first while one serves, and otherwise goes by the last.
//! - A refusal shows in a message and in the Logs.

mod controls;
mod frames;
mod input;
mod logs;
mod picks;
mod project;
mod run;
mod scene;

use std::collections::VecDeque;
use std::mem;
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process::{Child, ChildStdin};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use gg_api::InputEvent;
use gg_api::view::Reader;
use gg_engine::dock::Docked;
use gg_engine::egui::{self, ViewportCommand, ViewportId};
use gg_engine::inspector::InspectorLink;
use gg_engine::math::DVec3;
use gg_engine::{
    App, Context, EntityId, Event, Inspector, Layer, PerfPanel, Ui, Viewpoint, Viewport,
};
use serde_json::{Value, json};
use tracing::warn;

use crate::assets::{Assets, Dragged};
use crate::debugger::Debugger;
use crate::file::{self, Item, Message};
use crate::handles::Sight;
use crate::hub::{self, Hub};
use crate::look::{Looking, Part};
use crate::overlay::{self, Toolbar};
use crate::scenes::{self, Scenes};
use crate::view::{self, ViewTools};
use frames::{Measures, Wants, wants};
use input::in_view;
use logs::{Level, Logged, show_logs};
use picks::{Dropping, Found, Probing, along, picked};
use project::{Folders, folders};
use run::{ENDING, Line, PLACES, RunState, STATUS_EVERY, Talker};
use scene::{Action, Closing, Request};

/// The editor of project `dir`, or else of the one the hub picks, finding projects in `workspace`,
/// the engine's, if it runs in one. Its game is optimized if `release`. The side panels' widths fit
/// the Inspector's vectors and the Entities header in a 1920 by 1080 point window.
pub(crate) fn app(dir: Option<PathBuf>, workspace: Option<PathBuf>, release: bool) -> App {
    let app = App::new("Editor")
        .embeds_game()
        .custom_title_bar()
        .view_panel("Viewport")
        .window_size(1920, 1080);
    let app = app
        .dock("Inspector", Docked::RightOf("Viewport", 0.22))
        .dock("Entities", Docked::LeftOf("Viewport", 0.2))
        .dock("Scenes", Docked::Below("Entities", 0.5))
        .dock("Performance", Docked::Below("Inspector", 0.5))
        .dock("Logs", Docked::Below("Viewport", 0.32))
        .dock("Assets", Docked::With("Logs"))
        .dock("Debugger", Docked::With("Logs"));
    let inspector = Inspector::remote(CLIENT);
    let mut game = Game::new(dir, inspector.link());
    game.hub = Hub::new(workspace);
    game.release = release;
    app.layer(game).overlay(PerfPanel).overlay(inspector)
}

/// The client the editor is to the game, the author of its edits.
const CLIENT: &str = "editor";

/// The editor's layer: the project's `gg run`, its connection to the game, and everything the
/// panels, title bar and File menu show of both.
struct Game {
    dir: Option<PathBuf>,
    release: bool,
    /// `gg run`, until it ends, with its stdin, which the editor closes as it ends.
    run: Option<(Child, ChildStdin)>,
    /// The `gg run`s of earlier projects, each with its directory, until they've ended.
    ending: Vec<(PathBuf, Child)>,
    /// Whether this project's run waits to start until the one in its directory has ended, as
    /// [`start`](Self::start) says.
    waiting: bool,
    hub: Hub,
    /// The project a close under way opens instead of closing the editor, as
    /// [`switch_to`](Self::switch_to) says.
    switching: Option<PathBuf>,
    lines: Receiver<Line>,
    /// The sender the threads reading `gg run`'s output share, until the run starts.
    send: Option<Sender<Line>>,
    talker: Option<Talker>,
    /// The view's tools, whose link is the Inspector's. Each pick's ticket is the next of
    /// `tickets`.
    view: ViewTools<u64, Found>,
    tickets: u64,
    /// The ticket of the pick at the cursor that waits for its answer: one at a time.
    hovering: Option<u64>,
    /// The latest texture or font let go over the view whose pick waits for its answer.
    dropping: Option<Dropping>,
    /// The Probe tool's last probe, shown until Escape, another probe or another tool.
    probing: Option<Probing>,
    /// The answers to picks that came back since the last update, each with its ticket.
    answered: Vec<(u64, Option<Found>)>,
    looking: Looking,
    /// What the editor's wanted parts of the look went by as the last update left them, and the
    /// picks that update asked the Inspector for as the look's outline: none unpicks.
    wants: Wants,
    took_pick: Option<Vec<EntityId>>,
    /// The entity the Entities panel asked to frame, while the link doesn't place it yet, as it
    /// doesn't a pick just made until a read of it comes.
    framing: Option<EntityId>,
    /// Whether the view's input went to the game as the game was last told, as
    /// [`through`](Self::through) says: as it stops, the game lets go of what's held, and as it
    /// starts, it gets the cursor.
    told: bool,
    /// The last frame of the game's view the editor showed: its size, and what it was drawn from,
    /// if that projects.
    size: Option<[u32; 2]>,
    seen: Option<Viewpoint>,
    /// Whether the view sees through the game's camera only for the editor's to adopt its
    /// viewpoint, at start and after a New or an Open, so the view frames the game's scene. In
    /// Edit, the first frame drawn through it turns the view to the editor's camera. Any other turn
    /// of the view, or a Play or Simulate, ends it.
    adopting: bool,
    /// The memory the game's view is shared through, and memory a `view` asked the game to share it
    /// through instead, until the game answers.
    reader: Option<Reader>,
    asked: Option<Reader>,
    /// Whether the game refused a `view`: the editor asks for none until another game serves.
    refused: bool,
    /// The input the view got for the game since the last `input` sent, and whether that's still
    /// to be answered: one at a time, as the game answers one a frame.
    input: Vec<InputEvent>,
    inputting: bool,
    grab: Grab,
    run_state: RunState,
    /// As the game last answered: stopped until a game that serves answers, so that nothing goes
    /// to it before.
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
    /// None outside a project.
    assets: Option<Assets>,
    scenes: Option<Scenes>,
    folders: Option<Folders>,
    debugger: Debugger,
    /// Whether a game has served since the editor started, from when a close asks whether to save.
    served: bool,
    /// The open scene's file, as the asset root names it (`None` while untitled), and whether it
    /// has unsaved edits, as the game last answered. Kept while no game serves, as `gg run`
    /// restarts it.
    scene: Option<String>,
    unsaved: bool,
    /// Whether the game's world has cells, as its last status said, which a `cells` asked with
    /// each status reads; and its cells, as the last `cells` answered, for the Scenes panel.
    streams: bool,
    cells: Option<gg_api::WorldCells>,
    /// The File menu's `scene` request, until the game answers it.
    request: Option<Request>,
    /// What the prompt asks whether to save unsaved edits before, while it shows; what the save
    /// its Save chose goes on to once the game has saved them; and the File menu's item that the
    /// next UI chooses, as Save As for a Save the game refused for an untitled scene, since its
    /// dialog shows from the UI.
    prompt: Option<Action>,
    then: Option<Action>,
    due: Option<Item>,
    closing: Closing,
    /// Why the game, or the editor, refused the File menu's last request, until it's dismissed.
    message: Option<Message>,
    measures: Measures,
    /// The cap on the game's frames, as the settings' game frame rate limit last said or another
    /// client set it.
    max_fps: Option<NonZeroU32>,
    /// The game's render scale, as the settings' game render scale last said or another client set
    /// it; `None` for the game's own.
    render_scale: Option<f32>,
}

/// The cursor's grab for the game, in Play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grab {
    /// Grabbed as the game asks, as the view's memory says.
    Asks,
    /// Shift+F1 let go of the cursor, until a press in the view, a click of Play or of Resume in
    /// Play, or the input stopping going to the game.
    Released,
}

impl Game {
    /// The layer of the project in directory `dir`, or of none, seeing through the game's camera
    /// until the editor's adopts where it sees from. Its tools pick and edit through `inspector`.
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
            release: false,
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
            probing: None,
            answered: Vec::new(),
            looking: Looking::new(CLIENT),
            wants,
            took_pick: None,
            framing: None,
            told: false,
            size: None,
            seen: None,
            adopting: true,
            reader: None,
            asked: None,
            refused: false,
            input: Vec::new(),
            inputting: false,
            grab: Grab::Asks,
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
            streams: false,
            cells: None,
            request: None,
            prompt: None,
            then: None,
            due: None,
            closing: Closing::No,
            message: None,
            measures: Measures::default(),
            max_fps: None,
            render_scale: None,
        }
    }
}

impl Layer for Game {
    /// Reads the hub's projects and the toolbar's state, then opens the project the editor was
    /// started in, or else the latest if the hub skips to it, or none, the hub showing.
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
        // Earlier projects' runs may hold up this one's.
        (self.ending).retain_mut(|(_, child)| matches!(child.try_wait(), Ok(None)));
        if self.waiting {
            self.start();
        }
        // Before `gg run`'s lines, which may say the game has gone, letting go of what it logged.
        self.take_logged();
        self.take_lines();
        // The user's changes, before the game's answers change them and before the camera
        // settles, which changes nothing the view shows.
        self.view.camera.update(dt);
        self.frame_asked();
        self.changes(wants(&self.view));
        // The settings' game frame rate limit caps the game, and its game render scale scales it;
        // ones another client set show there.
        let max_fps = cx.game_max_fps();
        if max_fps != self.max_fps {
            self.max_fps = max_fps;
            self.looking.change(Part::MaxFps);
        }
        let render_scale = cx.game_render_scale();
        if render_scale != self.render_scale {
            self.render_scale = render_scale;
            self.looking.change(Part::RenderScale);
        }
        self.view.settle();
        self.take_answers();
        cx.set_game_max_fps(self.max_fps);
        cx.set_game_render_scale(self.render_scale);
        self.debugger.update();
        self.take_picks();
        self.take_probes();
        if let Some(view) = cx.view() {
            self.fit(view.size);
        }
        // The game's grab, in Play alone: Stop or Simulate end what Shift+F1 did. Also the editor's
        // camera's grab.
        let through = self.through();
        if !through {
            self.grab = Grab::Asks;
        }
        let asked = self.reader.as_ref().is_some_and(Reader::cursor_grabbed);
        let games = through && self.grab == Grab::Asks && asked;
        cx.set_cursor_grabbed(games || self.view.camera.grabs());
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
        // The images the game's frame makes, as the list of what the view shows opens.
        if mem::take(&mut self.view.toolbar.ask_images)
            && let Some(talker) = &self.talker
        {
            talker.ask("images", Value::Null);
        }
        if let Some(talker) = &self.talker
            && !self.input.is_empty()
            && !self.inputting
        {
            let events = mem::take(&mut self.input);
            talker.ask("input", json!({ "events": events }));
            self.inputting = true;
        }
        // After the look and the input: the game starts its next frame as the editor takes this
        // one, and draws it with what came before. Sent after the take, a steadily moving camera
        // showed the previous frame's viewpoint in a third of the game's frames, and twice the
        // step in the next.
        self.take_frame(cx);
        let driving = !self.view.toolbar.world_camera;
        self.view.shown = self.seen.filter(|_| driving);
        // The File menu's request and a close's status wait for the game to answer the Inspector's
        // edits, which go on a connection of their own, so that the game has them first.
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
            // The world's cells, for the Scenes panel.
            if self.streams {
                talker.ask("cells", Value::Null);
            }
            self.statusing = true;
            self.polled = Some(Instant::now());
            if checking {
                self.closing = Closing::Checking(true);
            }
        }
    }

    /// Routes events as [`route`](Game::route) says, and vetoes a close of the editor until it's
    /// granted, as [`close`](Game::close) says. A switch under way becomes the close.
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
        // The hub, over the panels, which go on showing the open project behind it.
        if self.hub_shows() {
            let current = self.dir.clone();
            let hub = &mut self.hub;
            match ui.cover(|ui| hub.show(ui, current.as_deref())) {
                Some(hub::Chosen::Open(dir)) => self.switch_to(dir),
                Some(hub::Chosen::Browse) => self.browse(cx),
                Some(hub::Chosen::Folder(start)) => self.pick_folder(cx, start.as_deref()),
                Some(hub::Chosen::Back) => self.hub.asked = false,
                None => {}
            }
            self.hub.save(cx);
        }
        // The shortcuts, whichever panel has the keys. What's chosen is done once the menu is
        // drawn: Open and Save As show a dialog, which takes the context and blocks the frame.
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
        let (paused, serves) = (self.paused, self.talker.is_some());
        let traced = self.view.toolbar.show == Some(gg_api::Shown::PathTraced);
        // None over the hub, where the bar shows the title.
        let bar = !self.hub_shows();
        let title_bar = |ui: &mut egui::Ui| {
            if let Some((scene, tip)) = scene {
                ui.label(scene).on_hover_text(tip);
            }
            ui.label(label);
            let run = |ui: &mut egui::Ui| overlay::show_run(ui, mode, paused, traced);
            let clicked = ui.add_enabled_ui(serves, run);
            clicked.inner
        };
        let clicked = bar.then(|| ui.title_bar(title_bar)).flatten();
        // The click's press, off the view, took the keys Play gives the view.
        if let Some(clicked) = clicked.flatten()
            && self.click(clicked)
        {
            ui.focus_view();
        }
        // A click on a scene, after the menus' choices, as a File menu New.
        let (open, unsaved, enabled) = (self.scene.clone(), self.unsaved, self.files());
        let (scenes, cells) = (&mut self.scenes, self.cells.as_ref());
        let mut chosen = None;
        ui.panel("Scenes", |ui| match scenes {
            Some(scenes) => chosen = scenes.show(ui, open.as_deref(), unsaved, enabled, cells),
            None => {
                ui.weak("Not in a game's project");
            }
        });
        match chosen {
            Some(scenes::Chosen::Open(path)) => self.open(path),
            Some(scenes::Chosen::New) => self.act(Action::New),
            Some(scenes::Chosen::Frame(center, radius)) => {
                (self.view.camera).frame_sphere(DVec3::from_array(center), radius);
            }
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
        // What's held over the view counts as over it, unlike egui's hover while another widget
        // drags, so that the corner names what would take it.
        let view = ui.view();
        let held = self.talker.is_some()
            && (view.as_ref()).is_some_and(|view| view.dnd_hover_payload::<Dragged>().is_some());
        if let Some(response) = &view {
            self.take_drop(ui.egui(), response, cx.view(), cx.scale() as f32);
        }
        let logs = &self.logs;
        ui.panel("Logs", |ui| show_logs(ui, logs));
        // What the game's view shows under the cursor: a pick as each comes back, the latest
        // answer's find painted over the handles.
        let origin = cx.view().map_or([0, 0], |view| view.origin);
        let over = view.filter(|view| view.hovered() || held);
        let at = over.and(cx.input().cursor());
        self.view.hover.see(at.is_some(), &self.answered);
        if let Some(at) = at
            && self.hovering.is_none()
        {
            // As a click there would pick.
            self.hovering = self.ask_pick(at, origin, self.view.link.picked());
            self.view.hover.ask(self.hovering);
        }
        let name = self.view.hover.under().map(|found| found.name);
        // Where the frame shows in this frame's render of the view.
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
        if let Some(view) = ui.view() {
            if let Some(probing) = &self.probing {
                let points = ui.egui().pixels_per_point();
                let [x, y] = probing.at.map(|axis| axis as f32 / points);
                let at = egui::Pos2::new(x, y);
                let text = overlay::probe_text(probing.answer.as_ref());
                overlay::probe_readout(ui.egui(), &view, at, text);
            }
            // In Play the keys are the game's.
            if self.through() && ui.view_focused() {
                overlay::outline_view(ui.egui(), &view);
            }
        }
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
        // A close granted: a switch to another project goes now; the editor's close is made again
        // to go ahead.
        if self.closing == Closing::Granted {
            match self.switching.take() {
                Some(dir) => self.switch(cx, dir),
                None => ui
                    .egui()
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close),
            }
        }
    }

    /// Closes `gg run`'s stdin, which ends the game and the run. While a game serves, waits for the
    /// end for at most [`ENDING`], but not for a build under way, which `gg run` finishes first.
    /// Then logs what it measured of the view's frames.
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

#[cfg(test)]
mod tests;
