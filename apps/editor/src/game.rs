//! The game of the project the editor is started in, run as an agent runs it: under `gg run
//! --embedded` in the project's directory, wherever it is, with the gg of the engine the editor
//! is built from, as [`crate::gg`] has it, which builds it, patches it as its code changes and
//! restarts it after any other change, and which ends, with the game, as the editor ends,
//! closing its stdin; or, given `--release-game`, under `gg run --release --embedded`, which builds
//! it optimized, without patches. The game starts stopped, as `--stopped` has it: its world is the
//! world as edited, which only edits, scene files' saves and the File menu's New and Open change,
//! until Play. It renders offscreen for the editor, which shows its view in the Viewport from
//! memory the two share, as the tool API's `view` asks. The Entities and Inspector panels show the
//! game's world, and edit it, through the API, as a remote [`Inspector`] does; the Logs panel
//! shows `gg run`'s events, the build's errors among them, the game's events as it logs them,
//! which the tool API's `logs` follows, and what the game and the build print; the Assets panel,
//! the files in the project's asset root; the [Debugger](crate::debugger) panel, its scripts; and
//! the title bar, how the run goes, or while a game serves, whether it's stopped, playing or
//! simulating, and whether it's paused, as the game answers the editor's requests and the `status`
//! it asks for every quarter second, which catches another client's, or that it waits at a
//! script's stop, as the Debugger panel learns; with the controls [`overlay::show_run`] draws:
//! Play, which pauses and resumes the game while it plays, Simulate, Stop and Step.
//!
//! Started outside a project, the editor shows the [`hub`] over its panels, where a
//! project is picked, unless it's to open the latest as it starts; and the File menu's Open Project
//! shows it over the project open, which its Back goes back to. A project picked there opens in
//! place of the one open as a close of the editor goes, asking whether to save the open scene's
//! unsaved edits first: the run of the one open ends, going on ending as the new project's starts,
//! but in the same project, which waits for it, and the panels show the new project's, the view
//! empty until its game's first frame.
//!
//! Play has a copy of the world as edited play in its place, as the API's `play` does, the view
//! seeing through the game's own camera, which gets all the input the view gets, the view's tools
//! only what lets go of what they hold; Simulate plays it the same, through the editor's camera,
//! the game getting no input. Simulate, while the game plays, switches between the two, carrying
//! on, moving the camera and the input over, the game letting go of what's held as the input
//! leaves it, as the toolbar's camera switch does while the game plays. Play from stopped plays a
//! game paused since before its stop in real time, as Play plays; while it plays, Play is Pause,
//! and while it's paused, Resume. Step, while it's paused, runs a tick, and while it's stopped
//! plays it paused first, through the camera the view shows. Stop stops the input at once, and
//! brings back the world as edited, as `stop` does, with its edits' history, the view going to the
//! editor's camera, as it does at any client's `stop` that no Play clicked since follows. Stopped,
//! the camera switch has the view show what the game's camera sees, as it does at first and as a
//! New or an Open opens a scene, the input staying the editor's, until the editor's camera takes
//! some, which turns the view to it. Play and Simulate are Unreal's Play and Simulate In Editor,
//! and a switch between them its Possess and Eject.
//!
//! A game that asks for the cursor grabbed, for looking about with the mouse, as the view's memory
//! says it does, has the editor grab its own window's in Play while the view has the keys, the
//! mouse's raw motion going to the game; Escape lets go of it, whatever the game does with the key,
//! which it gets, as a game may not let go itself, and a press in the view grabs it again;
//! Simulate and Stop let go. Play, and Resume in Play, capture the view's input for any game: they
//! give the view the keys, which their click in the title bar took, and grab the cursor, which the
//! engine moves onto the view, until Escape, after which a press in the view grabs it again only for
//! a game that asks. In Play, an outline round the view says while it has the keys, which go to the
//! game. The editor's camera grabs the cursor too, while it looks about, orbits or dollies, as
//! [`camera`](crate::camera) says.
//!
//! Through the editor's camera, the handles show. A click in the view, but in Play, picks what the
//! game's view shows there alone, as the API's `pick` finds it, which the panels show picked, and
//! with Ctrl (Cmd on macOS) or Shift held adds it to what's picked, or takes it out: of a model,
//! its root, but while the root or one of its nodes is the entity picked last, the node there, so
//! that a second click goes into a model. The handles, at the entity picked last, move, turn and
//! scale it, and the others picked with it by the same change, each about its own origin, but
//! those under another picked, whose drag carries them, through the Inspector's link, each drag
//! one edit of the editor's, which its Edit menu undoes; none show while one they'd move is a
//! model's node, which the game refuses to change. F frames them all; and the view's bottom left
//! names what's under the cursor, as a click there would pick.
//! Move's and Rotate's go along the entity's own axes or the world's, as the toolbar's space switch
//! says, which X flips; Scale's are always the entity's own.
//! The toolbar's Probe tool reads what shading took and made at the pixel clicked, through gg-api's
//! [`probe`](gg_api::probe), as `gg probe` does, pausing a playing game for its captures, and shows
//! the numbers beside the pixel until Escape, another probe or another tool.
//! The title bar's settings switch on the game's frame rate and the editor's at the view's bottom
//! right, as [`rates`](crate::rates) says.
//! The editor's camera, the outline round what's picked, the grid, the colliders' switch, what the
//! toolbar has the view show in place of its light, and the passes its Passes list leaves out, to
//! narrow down which one makes what the view shows, reach the game as parts of its look, as
//! [`look`](crate::look) says: each as the user changes it, the editor taking the parts another
//! client, such as an agent, sets. The list of what the view shows lists, after the parts of
//! shading, the images the game's frame makes, as the API's `images` answers them as it opens.
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
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process::{Child, ChildStdin};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use gg_api::InputEvent;
use gg_api::view::Reader;
use gg_engine::egui::{self, ViewportCommand, ViewportId};
use gg_engine::math::DVec3;
use gg_engine::{
    App, Context, Docked, EntityId, Event, Inspector, InspectorLink, Layer, PerfPanel, Ui,
    Viewpoint, Viewport,
};
use serde_json::{Value, json};
use tracing::warn;

use crate::assets::{Assets, Dragged};
use crate::debugger::Debugger;
use crate::file::{self, Item, Message};
use crate::handles::Sight;
use crate::hub::{self, Hub};
use crate::look::Looking;
use crate::overlay::{self, Toolbar};
use crate::rates::Rates;
use crate::scenes::{self, Scenes};
use crate::view::{self, ViewTools};
use frames::{Measures, Wants, wants};
use input::in_view;
use logs::{Level, Logged, show_logs};
use picks::{Dropping, Found, Probing, along, picked};
use project::{Folders, folders};
use run::{ENDING, Line, PLACES, RunState, STATUS_EVERY, Talker};
use scene::{Action, Closing, Request};

/// The editor of the project in directory `dir`, if it's started in one, and otherwise of the one
/// the hub picks, the hub finding projects in `workspace`, the engine's, if it runs in one: in a
/// window 1920 by 1080 points as it opens, the game's view in the Viewport, its entities left of
/// it, above its scenes, the one picked right of it, above the Performance panel, and the Logs
/// below it, the Assets and the Debugger among their tabs; the sides wide enough, at that size,
/// for the Inspector's vectors and the Entities panel's header. Its game is built optimized if
/// `release`.
pub(crate) fn app(dir: Option<PathBuf>, workspace: Option<PathBuf>, release: bool) -> App {
    let app = App::new("Editor")
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

/// The editor's layer: the `gg run` of the project's game, the connection to the game that serves,
/// the memory its view is shared through, how the game runs, and what the Logs, Assets and Scenes
/// panels and the title bar show; the game's open scene, what the File menu asks of it, and how far
/// a close has got; the view's tools, the picks they ask of the game and the look they keep with
/// it; and the hub, with the runs of the projects it switched from that are ending.
struct Game {
    /// The project's directory, while one's open: the one it was started in, or the hub opened.
    dir: Option<PathBuf>,
    /// Whether `gg run` builds the game optimized, as the editor's `--release-game` asks.
    release: bool,
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
    /// The Probe tool's last probe, whose numbers show beside its pixel until Escape, another
    /// probe, or another tool: its ticket, the next of `tickets`; its pixel, in the main window's
    /// pixels; and its answer, once it's come.
    probing: Option<Probing>,
    /// The answers to picks that came back since the last update, each with its ticket.
    answered: Vec<(u64, Option<Found>)>,
    /// The game's look, as the editor keeps it.
    looking: Looking,
    /// What the editor's wanted parts of the look went by as the last update left them, and the
    /// picks that update asked the Inspector for as the look's outline, if it asked for them: none
    /// unpicks.
    wants: Wants,
    took_pick: Option<Vec<EntityId>>,
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
    /// Whether the view sees through the game's camera only for the editor's camera to adopt where
    /// it sees from, as the editor starts and as a New or an Open is done, so that the view frames
    /// the scene the game has: in Edit, the first frame that drew through it turns the view to the
    /// editor's camera, which shows the handles and the grid. Any other turn of the view, and a
    /// Play or Simulate, end it.
    adopting: bool,
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
    /// Whether Play captured the view's input for the game, the cursor grabbed whether or not the
    /// game asks: from a click of Play, or of Resume in Play, until Escape, or the view's input
    /// stopping going to the game, but while a Play clicked waits for the game's answer.
    captured: bool,
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
    /// Whether the game's world has cells, as its last status said, which a `cells` asked with
    /// each status reads; and its cells, as the last `cells` answered, for the Scenes panel.
    streams: bool,
    cells: Option<gg_api::WorldCells>,
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
    /// The game's frame rate and the editor's, which the view shows while the settings say.
    rates: Rates,
}

impl Game {
    /// The layer of the project in directory `dir`, or of none, through the game's camera until
    /// the editor's adopts where it sees from, whose tools pick and edit through `inspector`, the
    /// Inspector's link.
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
            escaped: false,
            captured: false,
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
            rates: Rates::default(),
        }
    }
}

impl Layer for Game {
    /// Reads the projects the hub keeps, and where the toolbar was, and opens the project the
    /// editor was started in, noting it, or else the latest opened, if the hub's to skip to it; or,
    /// with neither, opens none, the hub showing.
    fn attach(&mut self, cx: &mut Context) {
        self.hub.read(cx);
        self.view.toolbar.read(cx);
        self.rates.read(cx);
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
        self.take_probes();
        if let Some(view) = cx.view() {
            self.fit(view.size);
        }
        // The game's grab, as it asks or Play captured the view's input for it, in Play alone, a
        // Stop or Simulate ending what Escape and Play did too, but while a Play clicked waits for
        // its answer; and the editor's camera's, while it looks about, orbits or dollies.
        let through = self.through();
        self.escaped &= through;
        self.captured &= through || self.playing > 0;
        let asked = self.reader.as_ref().is_some_and(Reader::cursor_grabbed);
        let games = through && !self.escaped && (asked || self.captured);
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
        // The images the game's frame makes, for the list of what the view shows, as it opens.
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
        // one, and draws it with what came before. Sent after the take, a camera moving steadily
        // showed the viewpoint of the frame before in a third of the game's frames, and twice the
        // step in the next.
        self.take_frame(cx);
        let published = self.reader.as_ref().map(Reader::published);
        self.rates.see(Instant::now(), published);
        let driving = !self.view.toolbar.world_camera;
        self.view.shown = self.seen.filter(|_| driving);
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
            // With the status, a world's cells, for the Scenes panel.
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
        self.rates.settings(ui, cx);
        // The hub, over the panels, which go on showing the project open, if one is, behind it.
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
        let (paused, serves) = (self.paused, self.talker.is_some());
        // None over the hub, where the bar shows the editor's title.
        let bar = !self.hub_shows();
        let title_bar = |ui: &mut egui::Ui| {
            if let Some((scene, tip)) = scene {
                ui.label(scene).on_hover_text(tip);
            }
            ui.label(label);
            let clicked = ui.add_enabled_ui(serves, |ui| overlay::show_run(ui, mode, paused));
            clicked.inner
        };
        let clicked = bar.then(|| ui.title_bar(title_bar)).flatten();
        if let Some(clicked) = clicked.flatten() {
            self.click(clicked);
            // A capture gives the view the keys, which the click's press, off the view, took.
            if self.captured {
                ui.focus_view();
            }
        }
        // A click on a scene, once the menus' choices are done, as a New of the File menu.
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
            // As a click there would pick.
            self.hovering = self.ask_pick(at, origin, self.view.link.picked());
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
        if let Some(view) = ui.view() {
            self.rates.paint(ui.egui(), &view);
            if let Some(probing) = &self.probing {
                let points = ui.egui().pixels_per_point();
                let [x, y] = probing.at.map(|axis| axis as f32 / points);
                let at = egui::Pos2::new(x, y);
                let text = overlay::probe_text(probing.answer.as_ref());
                overlay::probe_readout(ui.egui(), &view, at, text);
            }
            // Where the keys go in Play, which are the game's.
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

#[cfg(test)]
mod tests;
