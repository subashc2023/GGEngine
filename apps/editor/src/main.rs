//! The editor: the game's view in a panel among the editor's own, in a dock that fills its window.
//! Until the tool API brings it a game, the view shows a world of the editor's own, three squares
//! and two cameras, through the editor's camera, orthographic at first, as 2D games see theirs, over a
//! grid, with a toolbar and a gizmo over it; or through the world's camera, as the game would, and
//! then W, A, S and D drive the world's cameras, by a script of theirs, as a game's keys would. The
//! Inspector edits the world, and undoes and redoes its edits. A click in the view picks what it
//! shows there for the Inspector, or unpicks where it shows none, but with Pan or Orbit, whose
//! left button moves the camera; and the view's bottom left names what it shows under the cursor,
//! while the cursor is over it. Through the editor's camera, the toolbar's Move, Rotate and
//! Scale, which W, E and R pick, as Q picks Select, show handles over what the Inspector has
//! picked, whose drags move, turn and scale it, each an edit of the Inspector's. The File menu
//! starts a new scene, emptying the world; opens one, whose entities replace the world's; and
//! saves the world's entities as one, to the file it was opened from or saved to last, or as
//! another: through the OS's file dialogs, and with Ctrl+N, Ctrl+O, Ctrl+S and Ctrl+Shift+S, Cmd
//! for Ctrl on macOS. The Assets panel shows the files in the editor's assets folder, a folder at a
//! time, and a scene dragged from it onto the view opens as Open opens one. When a file can't be
//! read, opened or written, it says why, until that's dismissed.

mod assets;
mod camera;
mod grid;
mod handles;
mod overlay;

use std::collections::VecDeque;
use std::f32::consts::FRAC_PI_3;
use std::ffi::OsStr;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use gg_engine::egui::{
    self, Align2, Button, CursorIcon, Id, KeyboardShortcut, Modal, Modifiers, Response,
    ViewportCommand, ViewportId,
};
use gg_engine::math::{Vec2, Vec3, Vec4};
use gg_engine::reflect::Error;
use gg_engine::{
    Actions, App, Binding, Camera, Change, Commands, Component, Context, Docked, Entity, EntityId,
    Event, FileDialog, Inspector, InspectorLink, Key, Layer, Name, PerfPanel, Pick, Pose,
    Projection, Reflect, Script, Source, Sprite, Tick, Transform, Ui, Viewpoint, World,
    run_scripts,
};
use tracing::warn;

use assets::{Assets, DraggedScene};
use camera::{Move, OrbitCamera, Screen, Side, Tool};
use grid::Grid;
use handles::{Aim, Handles, Picked, Reading, Reply, Sight};
use overlay::Toolbar;

fn main() -> ExitCode {
    gg_engine::run(app())
}

fn app() -> App {
    let app = App::new("Editor").custom_title_bar().view_panel("Viewport");
    let app = app
        .dock("Inspector", Docked::RightOf("Viewport"))
        .dock("Entities", Docked::LeftOf("Viewport"))
        .dock("Performance", Docked::Below("Inspector"))
        .dock("Assets", Docked::Below("Viewport"));
    let inspector = Inspector::default();
    let link = inspector.link();
    let app = app.layer(Editor::new(link));
    app.overlay(PerfPanel).overlay(inspector)
}

/// The editor's layer: its camera, and the toolbar and gizmo over the view, and the name of what
/// it shows under the cursor; the handles over what the Inspector has picked, which pick and edit
/// through its link; the grid; the keys that drive the world's cameras; the File menu, which
/// empties the world for a new scene, replaces it with a scene's and saves it as one, keeping the
/// file it was opened from or saved to last for Save to write to; and the Assets panel, whose
/// scenes open as they're dropped on the view, as Open opens one. It says why when a file can't be
/// read, opened or written.
struct Editor {
    camera: OrbitCamera,
    toolbar: Toolbar,
    handles: Handles,
    /// The Inspector's, which says what's picked, and takes the handles' edits.
    link: InspectorLink,
    /// What the view on screen sees from: what the last update had it see from, while that's the
    /// editor's camera, which the handles are hit against.
    shown: Option<Viewpoint>,
    /// Where the last press over the view off the handles asked to pick, in the main window's
    /// pixels, until `event` asks the engine, keeping the ticket in `asked`.
    picking: Option<[f64; 2]>,
    /// The ticket of the latest pick asked for, until its answer comes back, which `update` has
    /// the Inspector pick, as [`answer`] says: a later press's takes its place, and a grab of a
    /// handle drops it, so that it picks nothing else under the drag.
    asked: Option<Pick>,
    /// What the view shows under the cursor, from the picks asked at the cursor while it's over the
    /// view, which the view's bottom left names.
    hover: Hover<Pick>,
    /// The grid, once attached.
    grid: Option<Grid>,
    /// What drives the world's cameras.
    actions: Actions<Act>,
    /// What the actions, the toolbar's switch and the File menu changed since the last tick,
    /// which it applies.
    commands: Vec<Command>,
    /// Whether the world's cameras take the keys: whether the view saw through the world's camera
    /// as the commands last heard; false once an Open has stopped them.
    gated: bool,
    /// The Assets panel, whose scenes open as they're dropped on the view.
    assets: Assets,
    /// The file the scene was opened from or saved to last, which Save writes to: none at first,
    /// and after New.
    scene: Option<PathBuf>,
    /// The files of the Opens queued, in their order: each is the scene's once its Open's tick
    /// replaces the world.
    opening: VecDeque<PathBuf>,
    /// Why a file couldn't be read, opened or written, shown until it's dismissed.
    message: Option<Message>,
}

/// What the editor's keys do to its world, beside what its camera's do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// W, A, S and D: which way the world's cameras go, while the view sees through the world's
    /// camera.
    Move,
}

/// What the editor's keys, its toolbar's switch and its File menu change of its world, queued as
/// they change it and applied at its next tick: its commands, which a recording keeps.
#[derive(Reflect)]
enum Command {
    /// Has the world's cameras go this way, across and up, until the next.
    Move(Vec2),
    /// Empties the world, for a new scene.
    New,
    /// Replaces the world's entities with those of the scene `text`, read from its file as it was
    /// opened, so that a replay needs no file; or, if it's refused, leaves the world as it was.
    Open { text: String },
}

impl Editor {
    /// The editor, whose handles pick and edit through `link`, the Inspector's.
    fn new(link: InspectorLink) -> Self {
        let mut actions = Actions::default();
        let wasd = Binding::Axis2 {
            x: [Key::KeyA, Key::KeyD].map(Source::Key),
            y: [Key::KeyS, Key::KeyW].map(Source::Key),
        };
        let bound = actions.bind(Act::Move, wasd);
        bound.expect("W, A, S and D are of one kind");
        Self {
            camera: OrbitCamera::new(),
            toolbar: Toolbar::default(),
            handles: Handles::default(),
            link,
            shown: None,
            picking: None,
            asked: None,
            hover: Hover::default(),
            grid: None,
            actions,
            commands: Vec::new(),
            gated: false,
            assets: Assets::new(assets_folder()),
            scene: None,
            opening: VecDeque::new(),
            message: None,
        }
    }

    /// Queues what the world's cameras do as the view starts or stops seeing through the world's
    /// camera, however the toolbar's switch went: go the way the keys held say, or stop, so that
    /// they move only while the view sees through it, from the next tick.
    fn gate(&mut self) {
        let through = self.toolbar.world_camera;
        if through != self.gated {
            self.gated = through;
            let heading = if through {
                self.actions.axis2(Act::Move)
            } else {
                Vec2::ZERO
            };
            self.commands.push(Command::Move(heading));
        }
    }

    /// Does what the File menu's `item` does, from [`Layer::ui`], as the menu or a shortcut
    /// chooses it: Open and Save As show the OS's dialog, which blocks the frame until it closes.
    fn choose(&mut self, cx: &mut Context, item: FileItem) {
        match (item, self.scene.clone()) {
            (FileItem::New, _) => self.commands.push(Command::New),
            (FileItem::Open, _) => {
                let picked = cx.pick_file(&self.dialog(false));
                if let Some(path) = picked {
                    self.open(path);
                }
            }
            (FileItem::Save, Some(path)) => self.save(cx.world(), path),
            // Save saves as, until there's a file to save to.
            (FileItem::Save | FileItem::SaveAs, _) => {
                let picked = cx.pick_file(&self.dialog(true));
                if let Some(path) = picked {
                    self.save_as(cx.world(), path);
                }
            }
        }
    }

    /// The dialog that picks a scene to open, or, if `save`, a file to save the world as: one that
    /// shows scenes, in the scene's directory, if there's a scene, and suggests its name to save
    /// as.
    fn dialog(&self, save: bool) -> FileDialog<'_> {
        let scene = self.scene.as_deref();
        let file_name = scene.filter(|_| save).and_then(Path::file_name);
        FileDialog {
            title: if save {
                "Save the scene as"
            } else {
                "Open a scene"
            },
            save,
            filter: Some(("Scenes", &["ron"])),
            directory: scene.and_then(Path::parent),
            file_name: file_name.and_then(OsStr::to_str),
        }
    }

    /// Reads the scene at `path`, whose entities replace the world's at the next tick, and whose
    /// file is then the scene's; or, if it can't be read, says why.
    fn open(&mut self, path: PathBuf) {
        match gg_engine::read_scene(&path) {
            Ok(text) => {
                self.opening.push_back(path);
                self.commands.push(Command::Open { text });
            }
            Err(error) => self.message = Some(Message::refused("open", Some(&path), &error)),
        }
    }

    /// Saves `world` to `picked`, the file Save As's dialog picked, with `.ron` after its name
    /// unless it ends in it, as the XDG portal's dialog may leave it. A file there at the name
    /// with `.ron` after is one the dialog didn't ask about replacing, so it isn't replaced: the
    /// editor says so, and saves nothing.
    fn save_as(&mut self, world: &World, mut picked: PathBuf) {
        if !is_scene(&picked) {
            picked.as_mut_os_string().push(".ron");
            if picked.exists() {
                let message = format!(
                    "{} is there already: pick it in the dialog to replace it",
                    picked.display()
                );
                let error = Error::new("editor.scene_exists", message);
                self.message = Some(Message::refused("save", Some(&picked), &error));
                return;
            }
        }
        self.save(world, picked);
    }

    /// Has the actions see `event`, and while the world's cameras take the keys, queues the way
    /// they go as the keys held change: now, rather than at the next update, so that the next tick
    /// applies it, while they take the keys. A key pressed with `command` held, Ctrl, or Cmd on
    /// macOS, is a shortcut's, such as Ctrl+S's S, which drives nothing; the actions see every
    /// release, so that none misses the release of a key it saw pressed.
    fn drive(&mut self, event: &Event<'_>, command: bool) {
        if command && matches!(event, Event::Key { pressed: true, .. }) {
            return;
        }
        let (gated, commands) = (self.gated, &mut self.commands);
        self.actions.event(event, |change| {
            if gated && let Change::Axis2(Act::Move, heading) = change {
                commands.push(Command::Move(heading));
            }
        });
    }

    /// Has Q, W, E or R pick its tool, Select, Move, Rotate or Scale, for `event`, a press of it,
    /// not a repeat; but not one with `command` held, Ctrl, or Cmd on macOS, which is a
    /// shortcut's; nor while the view sees through the world's camera, whose W, A, S and D drive
    /// the world's cameras; nor while the editor's camera flies, as its keys steer it; nor during a
    /// drag of the handles.
    fn choose_tool(&mut self, event: &Event<'_>, command: bool) {
        let Event::Key {
            key,
            pressed: true,
            repeat: false,
        } = *event
        else {
            return;
        };
        let busy = self.toolbar.world_camera || self.camera.flies() || self.handles.dragging();
        if command || busy {
            return;
        }
        self.toolbar.tool = match key {
            Key::KeyQ => Tool::Select,
            Key::KeyW => Tool::Move,
            Key::KeyE => Tool::Rotate,
            Key::KeyR => Tool::Scale,
            _ => return,
        };
    }

    /// What the Inspector has picked, as `world` has it, while it's alive with a transform, as the
    /// handles see it.
    fn picked(&self, world: &World) -> Option<Picked> {
        let id = self.link.picked()?;
        let entity = world.entity(id)?;
        let local = *world.get::<Transform>(entity)?;
        let parent = match world.parent(entity).ok()? {
            Some(parent) => world.pose(parent).ok()?,
            None => Pose::IDENTITY,
        };
        Some(Picked {
            entity: id,
            serial: world.serial(),
            local,
            parent,
            stepped: self.link.stepped(),
            waiting: self.link.waiting(),
        })
    }

    /// Does what `event` does, as the editor reads the window, `reading`, with what the Inspector
    /// has picked, `picked`: first the handles, which take a press that grabs one from the camera,
    /// and while a drag lasts, every other press, the wheel and Home, setting what the drag gives
    /// through the link and sealing its edit as it ends, or as the pick, the world or the
    /// Inspector's steps change; and which have a press off them ask for a pick. Then Q, W, E and
    /// R, which pick a tool; then the camera, which the view goes back to as it takes a press, the
    /// wheel or Home; then the keys that drive the world's cameras.
    fn take(
        &mut self,
        event: &Event<'_>,
        reading: &Reading,
        picked: Option<Picked>,
    ) -> ControlFlow<()> {
        if self.handles.hold(picked.as_ref()) {
            self.link.seal();
        }
        let aim = Aim {
            tool: self.toolbar.tool,
            viewpoint: self.shown,
            picked,
            camera: self.camera.moving().is_some(),
            easing: self.camera.eases(),
        };
        match self.handles.event(event, reading, &aim) {
            // The drag drops a pick asked for before it, whose answer would pick something else
            // under it.
            Reply::Take => {
                self.asked = None;
                return ControlFlow::Break(());
            }
            Reply::Set(entity, field) => {
                self.link
                    .set(entity, "Transform", field.path(), field.value());
            }
            Reply::Seal => self.link.seal(),
            // For `event`, which has the context, to ask of the engine.
            Reply::Pick(at) => self.picking = Some(at),
            Reply::Pass => {}
        }
        self.choose_tool(event, reading.command);
        let handled = self.camera.event(event, reading.screen, self.toolbar.tool);
        // What the editor's camera takes moves it, from where it was: the view goes back to it.
        if handled.is_break() {
            self.toolbar.world_camera = false;
        }
        self.drive(event, reading.command);
        self.gate();
        handled
    }

    /// Saves `world`, as the last tick left it, to the file at `path`, which is then the scene's;
    /// or, if it can't, says why.
    fn save(&mut self, world: &World, path: PathBuf) {
        let saved = whole_scene(world).and_then(|text| gg_engine::write_scene(&path, &text));
        match saved {
            Ok(()) => self.scene = Some(path),
            Err(error) => self.message = Some(Message::refused("save", Some(&path), &error)),
        }
    }

    /// Opens the scene dragged from the Assets panel and let go over `view`, the game's view, if
    /// one was this frame, as Open opens one; and outlines the view while one's held over it.
    fn take_drop(&mut self, egui: &egui::Context, view: &Response) {
        if view.dnd_hover_payload::<DraggedScene>().is_some() {
            overlay::outline_drop(egui, view);
        }
        if let Some(dropped) = view.dnd_release_payload::<DraggedScene>() {
            self.open(dropped.0.clone());
        }
    }

    /// Shows the message, if there is one, in front of the UI, which it keeps clicks from, until
    /// it's dismissed: by its OK, Escape or a click outside it.
    fn show_message(&mut self, egui: &egui::Context) {
        let Some(Message { title, text }) = &self.message else {
            return;
        };
        let shown = Modal::new(Id::unique("editor message")).show(egui, |ui| {
            ui.set_max_width(MESSAGE_WIDTH);
            ui.heading(title.as_str());
            ui.label(text.as_str());
            ui.button("OK").clicked()
        });
        if shown.inner || shown.should_close() {
            self.message = None;
        }
    }
}

/// The script that drives the world's cameras, as a game's would: it moves its entity across and up
/// at its speed, the way its heading says, which the editor's W, A, S and D set while the view sees
/// through the world's camera.
#[derive(Clone, Copy, Component)]
struct CameraController {
    /// How fast it goes, in units a second.
    #[reflect(min = 0.0, default = 5.0)]
    speed: f32,
    /// Which way it goes, across and up; one longer than 1 goes no faster than 1 does.
    heading: Vec2,
}

impl Script for CameraController {
    fn tick(entity: Entity, world: &mut World, _: Tick) {
        let Some(&Self { speed, heading }) = world.get(entity) else {
            return;
        };
        // One that isn't going anywhere stays where it is, to a zero's sign.
        if heading == Vec2::ZERO {
            return;
        }
        if let Some(at) = world.get_mut::<Transform>(entity) {
            let step = heading.clamp_length_max(1.0) * speed * Tick::DT.as_secs_f32();
            at.translation += step.extend(0.0);
        }
    }
}

/// Spawns the world the editor starts with: three squares, "Red", "Green" and "Blue", side by side,
/// and two cameras in front of them, of which the view sees through the primary, "Camera",
/// orthographic, 8 units tall, until the Inspector makes "Second camera", with perspective,
/// primary in its place. Each square is as much bigger as it's farther from "Second camera", so
/// that through it they look the same size, a unit 3 units in front of it, a quarter of a unit
/// apart; orthographic, which shows nothing smaller for being farther, shows how big they are. Both
/// cameras have a [`CameraController`], which goes 5 units a second, as Hazel's does.
fn spawn_world(world: &mut World) {
    // How far in front of the origin "Second camera" is.
    const SEEN_FROM: f32 = 3.0;
    let named = |name: &str| Name(name.to_owned());
    // Of a unit across where it's seen from, `across` from its middle, as big as it looks
    // `distance` in front of it.
    let square = |across: f32, distance: f32, [r, g, b]: [f32; 3]| {
        let scale = distance / SEEN_FROM;
        let at = Transform {
            translation: Vec3::new(across * scale, 0.0, SEEN_FROM - distance),
            scale: Vec3::new(scale, scale, 1.0),
            ..Transform::IDENTITY
        };
        let sprite = Sprite {
            color: Vec4::new(r, g, b, 1.0),
            ..Sprite::default()
        };
        (at, sprite)
    };
    // The nearest in the middle, so that orthographic shows them apart too.
    for (name, (at, sprite)) in [
        ("Red", square(-1.25, 4.5, [0.8, 0.2, 0.2])),
        ("Green", square(0.0, 3.0, [0.2, 0.7, 0.3])),
        ("Blue", square(1.25, 6.0, [0.2, 0.35, 0.85])),
    ] {
        world.spawn((named(name), at, sprite));
    }
    let in_front = |z: f32| Transform {
        translation: Vec3::new(0.0, 0.0, z),
        ..Transform::IDENTITY
    };
    let controller = CameraController {
        speed: 5.0,
        heading: Vec2::ZERO,
    };
    let orthographic = Camera {
        projection: Projection::Orthographic {
            height: 8.0,
            near: 0.1,
            far: 100.0,
        },
        primary: true,
    };
    world.spawn((named("Camera"), in_front(10.0), orthographic, controller));
    let perspective = Camera {
        projection: Projection::Perspective {
            fov: FRAC_PI_3,
            near: 0.1,
        },
        primary: false,
    };
    world.spawn((
        named("Second camera"),
        in_front(SEEN_FROM),
        perspective,
        controller,
    ));
}

/// Whether `world` has a camera for the view to see through: an entity with a [`Camera`] that's
/// primary and a [`Transform`].
fn has_camera(world: &World) -> bool {
    let mut cameras = world.query::<(&Camera, &Transform)>();
    cameras.any(|(camera, _)| camera.primary)
}

/// Has every [`CameraController`] in `world` go `heading`.
fn steer(world: &mut World, heading: Vec2) {
    for controller in world.query_mut::<&mut CameraController>() {
        controller.heading = heading;
    }
}

/// The editor's assets folder, `assets` beside its source, found from where it was built, as the
/// sandbox finds its files: what the Assets panel shows until the tool API brings a game.
fn assets_folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")
}

/// Whether `path` names a scene's file, as the File menu's dialogs show them: one whose name ends
/// in `.ron`, in any case.
fn is_scene(path: &Path) -> bool {
    let extension = path.extension();
    extension.is_some_and(|extension| extension.eq_ignore_ascii_case("ron"))
}

/// `world`'s roots and their descendants, every entity it has, as a scene's text.
fn whole_scene(world: &World) -> Result<String, Error> {
    let roots: Vec<Entity> = world.roots().collect();
    world.save_scene(&roots)
}

/// What the answers to picks that came back, `answers`, each a pick's ticket and the entity the
/// view showed there, if any, pick through the Inspector's link: once the latest pick asked for,
/// `asked`, has its answer, which forgets it, the entity it found, or none, which unpicks, as an
/// entity no longer alive does; `None` while it waits. The answers to picks that a later press
/// superseded pick nothing.
fn answer<T: Copy + PartialEq>(
    asked: &mut Option<T>,
    answers: &[(T, Option<EntityId>)],
) -> Option<Option<EntityId>> {
    let ticket = (*asked)?;
    let &(_, found) = answers.iter().find(|&&(answered, _)| answered == ticket)?;
    *asked = None;
    Some(found)
}

/// What the view shows under the cursor, as the answers to the picks asked at the cursor while it's
/// over the view come back, each pick's ticket a `T`, a [`Pick`] but in tests: every frame the
/// cursor is over the view asks one, and the view's bottom left names what the latest answer found,
/// four frames behind the cursor.
struct Hover<T> {
    /// The ticket of the first pick asked since the cursor last came over the view, while it's over
    /// it: the answers to picks asked before it found what was under the cursor before it left.
    since: Option<T>,
    /// The latest answer to a pick asked since then: its ticket, and the entity it found under the
    /// cursor, if any. Answers can come back out of the order their picks were asked in, as one
    /// outside the view, answered sooner, does before one inside it asked earlier.
    latest: Option<(T, Option<EntityId>)>,
}

impl<T> Default for Hover<T> {
    fn default() -> Self {
        Self {
            since: None,
            latest: None,
        }
    }
}

impl<T: Copy + Ord> Hover<T> {
    /// Takes `answers`, those that came back with the frame, with the cursor `over` the view or
    /// not. Over it, an answer to a pick asked since it came over the view, later than the latest,
    /// becomes the latest: a press's too, since a press off the handles asks at the cursor as well.
    /// Off it, the picks asked and their answers are forgotten.
    fn see(&mut self, over: bool, answers: &[(T, Option<EntityId>)]) {
        let Some(since) = self.since.filter(|_| over) else {
            *self = Self::default();
            return;
        };
        for &(pick, found) in answers {
            let later = self.latest.is_none_or(|(latest, _)| pick > latest);
            if pick >= since && later {
                self.latest = Some((pick, found));
            }
        }
    }

    /// Keeps `asked`, the ticket of a pick at the cursor over the view, if the frame had a pick left
    /// to ask, for [`see`](Self::see) to take its answer and those after it.
    fn ask(&mut self, asked: Option<T>) {
        self.since = self.since.or(asked);
    }

    /// The entity the view shows under the cursor, as the latest answer found it.
    fn under(&self) -> Option<EntityId> {
        self.latest.and_then(|(_, found)| found)
    }
}

/// What the view's bottom left calls `id`, the entity the view shows under the cursor, while it's
/// alive in `world`: its name, or, without a [`Name`], its ID.
fn hovered_name(world: &World, id: EntityId) -> Option<String> {
    let entity = world.entity(id)?;
    let name = world.get::<Name>(entity);
    Some(name.map_or_else(|| id.to_string(), |name| name.0.clone()))
}

/// An item of the File menu, which its shortcut chooses too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileItem {
    /// Empties the world at the next tick, for a new scene, which has no file until it's saved.
    New,
    /// Picks a scene, whose entities replace the world's at the next tick.
    Open,
    /// Saves the world to the scene's file, or, without one, as Save As does.
    Save,
    /// Picks a file to save the world to, which is then the scene's.
    SaveAs,
}

impl FileItem {
    /// The items, in the menu's order.
    const MENU: [Self; 4] = [Self::New, Self::Open, Self::Save, Self::SaveAs];

    /// What the menu calls it.
    fn label(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Open => "Open...",
            Self::Save => "Save",
            Self::SaveAs => "Save As...",
        }
    }

    /// The keys that choose it: N, O or S with Ctrl, or Cmd on macOS, and for Save As, S with
    /// Shift too.
    fn shortcut(self) -> KeyboardShortcut {
        let (modifiers, key) = match self {
            Self::New => (Modifiers::COMMAND, egui::Key::N),
            Self::Open => (Modifiers::COMMAND, egui::Key::O),
            Self::Save => (Modifiers::COMMAND, egui::Key::S),
            Self::SaveAs => (Modifiers::COMMAND.plus(Modifiers::SHIFT), egui::Key::S),
        };
        KeyboardShortcut::new(modifiers, key)
    }
}

/// Which item of the File menu a shortcut pressed among `events`, a frame's input to egui,
/// chooses, if one does: the first's. Takes out the shortcuts' presses and their repeats, which
/// choose nothing, so that one held down saves, or shows a dialog, once.
fn file_shortcut(events: &mut Vec<egui::Event>) -> Option<FileItem> {
    // Save As's first, since a shortcut matches with Shift held as well.
    let shortcuts = [
        FileItem::SaveAs,
        FileItem::Save,
        FileItem::New,
        FileItem::Open,
    ];
    let mut chosen = None;
    events.retain(|event| {
        let &egui::Event::Key {
            key,
            pressed: true,
            repeat,
            modifiers,
            ..
        } = event
        else {
            return true;
        };
        let matched = shortcuts.into_iter().find(|item| {
            let shortcut = item.shortcut();
            key == shortcut.logical_key && modifiers.matches_logically(shortcut.modifiers)
        });
        let Some(item) = matched else {
            return true;
        };
        if !repeat {
            chosen = chosen.or(Some(item));
        }
        false
    });
    chosen
}

/// The widest a message gets, in points, so that a long error wraps.
const MESSAGE_WIDTH: f32 = 400.0;

/// Why a file couldn't be read, opened or written, which the editor shows until it's dismissed.
struct Message {
    /// What couldn't be done, to which file, such as "Can't open grid.ron".
    title: String,
    /// Why: the error's text, which begins with the line and column where a scene's text was
    /// refused.
    text: String,
}

impl Message {
    /// Logs `error`, which refused to `verb` the scene at `path`, and says so: naming its file, or
    /// the scene without one, as a replay's Opens have none.
    fn refused(verb: &str, path: Option<&Path>, error: &Error) -> Self {
        let shown = path.map(|path| path.display().to_string());
        warn!(code = error.code(), path = shown, "{error:#}");
        let name = path.and_then(Path::file_name);
        let name = name.map_or("the scene".into(), OsStr::to_string_lossy);
        Self {
            title: format!("Can't {verb} {name}"),
            text: format!("{error:#}"),
        }
    }
}

impl Layer for Editor {
    fn attach(&mut self, cx: &mut Context) {
        cx.set_clear_color([0.02, 0.02, 0.025, 1.0]);
        self.grid = Some(Grid::new(cx));
        spawn_world(cx.world_mut());
    }

    fn tick(&mut self, world: &mut World, tick: Tick) {
        let Self {
            commands,
            gated,
            scene,
            opening,
            message,
            ..
        } = self;
        let mut opened = false;
        for command in commands.drain(..) {
            match command {
                Command::Move(heading) => steer(world, heading),
                Command::New => {
                    world.clear();
                    *scene = None;
                }
                Command::Open { text } => {
                    let path = opening.pop_front();
                    match world.replace_scene(&text) {
                        Ok(_) => {
                            *scene = path;
                            opened = true;
                        }
                        Err(error) => {
                            *message = Some(Message::refused("open", path.as_deref(), &error));
                        }
                    }
                }
            }
        }
        // A scene gives the cameras the headings it was saved with, such as those of keys held as
        // it was saved, but only keys held while the view sees through the world's camera drive
        // them. So they stop, after every command, so that a move queued since the Open stops
        // too; and the next update's gate has them go the way the keys held say, if the view sees
        // through the world's camera then, as it does when the view starts to.
        if opened {
            steer(world, Vec2::ZERO);
            *gated = false;
        }
        // Not once the world's last primary camera has gone, as the frame's later ticks would, until
        // its update turns the view back to the editor's camera and stops them.
        if has_camera(world) {
            run_scripts::<CameraController>(world, tick);
        }
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        self.camera.update(dt);
        let plane = grid::shown(&self.camera.viewpoint(), self.toolbar.plane);
        self.camera.settle(plane.normal());
        // Off once the world has no primary camera, as its ticks left it, so that no frame shows
        // the default viewpoint as the last one goes.
        self.toolbar.world_camera &= has_camera(cx.world());
        self.gate();
        // What the view showed under the latest press off the handles, once that comes back.
        if let Some(found) = answer(&mut self.asked, cx.picked()) {
            self.link.pick(found);
        }
        // A drag of the handles ends as the pick changes or dies, the world is another, the
        // Inspector undoes or redoes, or the view goes to the world's camera.
        let ended = self.handles.hold(self.picked(cx.world()).as_ref());
        let left = self.toolbar.world_camera && self.handles.end();
        if ended || left {
            self.link.seal();
        }
        if self.toolbar.world_camera {
            self.shown = None;
            return;
        }
        let camera = self.camera.viewpoint();
        cx.set_viewpoint(camera);
        self.shown = Some(camera);
        // Fitted to the view as the last frame showed it, which has nowhere to fit it at first.
        if self.toolbar.grid
            && let (Some(grid), Some(view)) = (&self.grid, cx.view())
        {
            grid.draw(cx, &camera, view, self.toolbar.plane);
        }
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        let input = cx.input();
        let screen = Screen {
            view: cx.view(),
            scale: cx.window().map_or(1.0, |window| window.scale() as f32),
            alt: input.key_down(Key::AltLeft) || input.key_down(Key::AltRight),
        };
        // The key egui's shortcuts take as the command key.
        let command = if cfg!(target_os = "macos") {
            [Key::SuperLeft, Key::SuperRight]
        } else {
            [Key::ControlLeft, Key::ControlRight]
        };
        let command = command.into_iter().any(|key| input.key_down(key));
        let reading = Reading {
            screen,
            command,
            cursor: input.cursor(),
        };
        let picked = self.picked(cx.world());
        let handled = self.take(event, &reading, picked);
        if let Some(at) = self.picking.take() {
            self.asked = cx.pick(at);
        }
        handled
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        let mut clicked = None;
        ui.menu("File", |ui| {
            for item in FileItem::MENU {
                let shortcut = ui.ctx().format_shortcut(&item.shortcut());
                let button = Button::new(item.label()).shortcut_text(shortcut);
                if ui.add(button).clicked() {
                    clicked = Some(item);
                }
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                ui.ctx()
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
        });
        // The shortcuts, whichever panel of the main window has the keys. What's chosen is done
        // once the menu is drawn, since Open and Save As show a dialog, which takes the context
        // and blocks the frame.
        let pressed = ui
            .egui()
            .input_mut(|input| file_shortcut(&mut input.events));
        if let Some(item) = clicked.or(pressed) {
            self.choose(cx, item);
        }
        ui.panel("Assets", |ui| self.assets.show(ui));
        if let Some(view) = ui.view() {
            self.take_drop(ui.egui(), &view);
        }
        self.show_message(ui.egui());
        // What the view shows under the cursor, while the cursor's over it: a pick at the cursor
        // every frame, and the name of what the latest answer found, painted over the handles.
        let over = ui.view().filter(|view| view.hovered());
        let at = over.as_ref().and(cx.input().cursor());
        self.hover.see(at.is_some(), cx.picked());
        if let Some(at) = at {
            self.hover.ask(cx.pick(at));
        }
        let name = (self.hover.under()).and_then(|id| hovered_name(cx.world(), id));
        let picked = self.picked(cx.world());
        let Self {
            camera,
            toolbar,
            handles,
            ..
        } = self;
        // The handles, on the view's own layer: over the world it shows, and under what's over it,
        // as the frame renders the view, through the editor's camera.
        if let (Some(view), Some(pixels), Some(viewpoint)) =
            (ui.view(), ui.viewport(), cx.viewpoint())
        {
            let scale = cx.window().map_or(1.0, |window| window.scale() as f32);
            if let Some(sight) = Sight::new(viewpoint, pixels, scale) {
                let egui = ui.egui();
                let ppp = egui.pixels_per_point();
                let painter = egui.layer_painter(view.layer_id).with_clip_rect(view.rect);
                // The one a press would grab: none while the camera drags or eases, nor with Alt
                // held, which orbits.
                let input = cx.input();
                let alt = input.key_down(Key::AltLeft) || input.key_down(Key::AltRight);
                let hover = view.hover_pos().filter(|_| view.hovered());
                let hover = hover.filter(|_| camera.moving().is_none() && !camera.eases() && !alt);
                let hover = hover.map(|at| Vec2::new(at.x, at.y) * ppp);
                handles.paint(&painter, ppp, &sight, toolbar.tool, picked.as_ref(), hover);
            }
        }
        if let (Some(view), Some(name)) = (over, name) {
            overlay::name_hovered(ui.egui(), &view, name);
        }
        let has_camera = has_camera(cx.world());
        ui.over_view("toolbar", Align2::LEFT_TOP, |ui| {
            toolbar.show(ui, camera, has_camera);
        });
        if !toolbar.world_camera {
            // The side the camera looks squarely from, which the gizmo names.
            let seen = camera.viewpoint();
            let square = cx.view().is_some_and(|view| grid::square(&seen, view));
            let aligned = square
                .then(|| Side::facing(seen.rotation * Vec3::NEG_Z))
                .flatten();
            ui.over_view("gizmo", Align2::RIGHT_TOP, |ui| {
                overlay::gizmo(ui, camera, aligned);
            });
        }
        // The cursor over the view says what the left button does, and while a drag goes on,
        // what it does, wherever the cursor is.
        let hovered = ui.view().is_some_and(|view| view.hovered());
        let icon = match camera.moving() {
            Some(Move::Pan) => Some(CursorIcon::Move),
            Some(Move::Orbit) => Some(CursorIcon::Grabbing),
            Some(Move::Look) => Some(CursorIcon::Crosshair),
            Some(Move::Dolly) => Some(CursorIcon::ZoomIn),
            None if hovered => match toolbar.tool {
                Tool::Select | Tool::Move | Tool::Rotate | Tool::Scale => None,
                Tool::Pan => Some(CursorIcon::Move),
                Tool::Orbit => Some(CursorIcon::Grab),
            },
            None => None,
        };
        if let Some(icon) = icon {
            ui.egui().set_cursor_icon(icon);
        }
        // As the toolbar's switch went.
        self.gate();
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use gg_engine::reflect::Value;
    use gg_engine::{MouseButton, Scroll, Viewport};

    use super::*;

    /// The editor, linked to an Inspector of its own.
    fn fresh() -> Editor {
        Editor::new(Inspector::default().link())
    }

    /// A view whose top left isn't the window's, at 1.5 pixels a point, as the camera's tests have:
    /// through the editor's camera as it starts, 10 units tall, the origin at its middle, (500,
    /// 350), and a unit 60 pixels across.
    const VIEW: Viewport = Viewport {
        origin: [100, 50],
        size: [800, 600],
    };

    /// On Move's X arrow, as the editor's camera starts, for an entity at the origin: 105 of its
    /// 150 pixels, 70 points, right of the middle.
    const ON_X: [f64; 2] = [605.0, 350.0];

    /// What the editor reads of the window with the cursor at `at`, Alt held if `alt`.
    fn reading(at: [f64; 2], alt: bool) -> Reading {
        Reading {
            screen: Screen {
                view: Some(VIEW),
                scale: 1.5,
                alt,
            },
            command: false,
            cursor: Some(at),
        }
    }

    /// An entity at the origin, unturned, at its own size, as the Inspector has picked it.
    fn at_the_origin() -> Option<Picked> {
        Some(Picked {
            entity: "0123456789abcdef".parse().expect("an ID"),
            serial: 1,
            local: Transform::IDENTITY,
            parent: Pose::IDENTITY,
            stepped: 0,
            waiting: false,
        })
    }

    /// What `editor` does with the cursor moved to `at`, and then with `button` pressed or let go
    /// there, as it reads the window there, with `picked` picked.
    fn press_at(
        editor: &mut Editor,
        picked: Option<Picked>,
        at: [f64; 2],
        button: MouseButton,
        pressed: bool,
    ) -> ControlFlow<()> {
        let moved = Event::CursorMoved { x: at[0], y: at[1] };
        let _ = editor.take(&moved, &reading(at, false), picked);
        let event = Event::MouseButton { button, pressed };
        editor.take(&event, &reading(at, false), picked)
    }

    /// An editor dragging Move's X arrow of `at_the_origin`, through its camera as it starts.
    fn dragging() -> Editor {
        let mut editor = fresh();
        editor.toolbar.tool = Tool::Move;
        editor.shown = Some(editor.camera.viewpoint());
        let pressed = press_at(&mut editor, at_the_origin(), ON_X, MouseButton::Left, true);
        assert_eq!(pressed, ControlFlow::Break(()));
        assert!(editor.handles.dragging());
        editor
    }

    /// A drag of a handle sets one field of the picked entity through the Inspector's link, the
    /// latest value of it alone between the Inspector's passes, and seals the edit as it ends; the
    /// camera sees the moves: here Move's X arrow, dragged right along it, and off it a little.
    #[test]
    fn a_drag_of_a_handle_sets_one_field_of_the_pick_and_seals() {
        let mut editor = dragging();
        let picked = at_the_origin();
        for x in [620.0, 650.0, 680.0] {
            let moved = Event::CursorMoved { x, y: 360.0 };
            let handled = editor.take(&moved, &reading([x, 360.0], false), picked);
            assert_eq!(handled, ControlFlow::Continue(()));
        }
        let at = [680.0, 360.0];
        let _ = press_at(&mut editor, picked, at, MouseButton::Left, false);
        assert!(!editor.handles.dragging());
        let pending = editor.link.pending();
        let [Some((id, component, path, Value::Vec3(moved))), None] = &pending[..] else {
            panic!("a set and a seal: {pending:?}");
        };
        let entity = picked.expect("picked").entity;
        let field = (*id, component.as_str(), path.as_str());
        assert_eq!(field, (entity, "Transform", "translation"));
        // 75 pixels right, at 60 a unit.
        assert!(
            moved.abs_diff_eq(Vec3::new(1.25, 0.0, 0.0), 1e-4),
            "{moved}"
        );
        assert_eq!(editor.picking, None, "it picked nothing");
    }

    /// While a drag of the handles lasts, the editor takes the other buttons' presses, the wheel
    /// and Home from its camera, which neither drags, dollies nor goes home; and while the camera
    /// drags, a press on a handle grabs nothing, and asks for no pick.
    #[test]
    fn the_handles_and_the_camera_keep_out_of_each_others_drags() {
        let mut editor = dragging();
        let picked = at_the_origin();
        // Away from where it starts, which Home would ease it back to.
        editor.camera.orbit(Vec2::new(40.0, 20.0));
        let seen = editor.camera.viewpoint();
        let press = |button| Event::MouseButton {
            button,
            pressed: true,
        };
        let home = Event::Key {
            key: Key::Home,
            pressed: true,
            repeat: false,
        };
        let wheel = Event::Scroll(Scroll::Lines { x: 0.0, y: 2.0 });
        for event in [
            press(MouseButton::Middle),
            press(MouseButton::Right),
            wheel,
            home,
        ] {
            let handled = editor.take(&event, &reading(ON_X, false), picked);
            assert_eq!(handled, ControlFlow::Break(()), "{event:?}");
        }
        editor.camera.update(Duration::from_secs(1));
        assert_eq!(editor.camera.moving(), None);
        assert_eq!(editor.camera.viewpoint(), seen);
        let _ = press_at(&mut editor, picked, ON_X, MouseButton::Left, false);
        let panning = press_at(&mut editor, picked, ON_X, MouseButton::Middle, true);
        assert_eq!(panning, ControlFlow::Break(()));
        assert_eq!(editor.camera.moving(), Some(Move::Pan));
        let left = press_at(&mut editor, picked, ON_X, MouseButton::Left, true);
        assert_eq!(left, ControlFlow::Continue(()));
        assert!(!editor.handles.dragging());
        assert_eq!(editor.picking, None);
    }

    /// Q, W, E and R pick Select, Move, Rotate and Scale as they're pressed: not as they repeat,
    /// with Ctrl held, through the world's camera, while the editor's camera flies, nor during a
    /// drag of the handles.
    #[test]
    fn q_w_e_and_r_pick_the_tools() {
        let mut editor = fresh();
        let key = |key, repeat| Event::Key {
            key,
            pressed: true,
            repeat,
        };
        for (pressed, tool) in [
            (Key::KeyW, Tool::Move),
            (Key::KeyE, Tool::Rotate),
            (Key::KeyR, Tool::Scale),
            (Key::KeyQ, Tool::Select),
        ] {
            editor.choose_tool(&key(pressed, false), false);
            assert_eq!(editor.toolbar.tool, tool);
        }
        editor.choose_tool(&key(Key::KeyW, true), false);
        editor.choose_tool(&key(Key::KeyW, false), true);
        assert_eq!(editor.toolbar.tool, Tool::Select, "repeating, or with Ctrl");
        editor.toolbar.world_camera = true;
        editor.choose_tool(&key(Key::KeyW, false), false);
        assert_eq!(
            editor.toolbar.tool,
            Tool::Select,
            "through the world's camera"
        );
        editor.toolbar.world_camera = false;
        let right = |pressed| Event::MouseButton {
            button: MouseButton::Right,
            pressed,
        };
        let screen = reading(ON_X, false).screen;
        let _ = editor.camera.event(&right(true), screen, Tool::Select);
        assert!(editor.camera.flies());
        editor.choose_tool(&key(Key::KeyE, false), false);
        assert_eq!(editor.toolbar.tool, Tool::Select, "flying");
        let mut editor = dragging();
        let _ = editor.take(
            &key(Key::KeyR, false),
            &reading(ON_X, false),
            at_the_origin(),
        );
        assert_eq!(editor.toolbar.tool, Tool::Move, "dragging");
        let _ = press_at(&mut editor, at_the_origin(), ON_X, MouseButton::Left, false);
        let _ = editor.take(
            &key(Key::KeyR, false),
            &reading(ON_X, false),
            at_the_origin(),
        );
        assert_eq!(editor.toolbar.tool, Tool::Scale, "let go");
    }

    /// A press of the left button over the view off the handles asks for a pick at the cursor,
    /// with Select, Move, Rotate and Scale, through the world's camera too, which it leaves the
    /// view seeing through; but not with Alt, which orbits, with Pan or Orbit, or with nothing
    /// under it to pick, as a press on a handle has. A grab of a handle drops the pick asked
    /// before it.
    #[test]
    fn a_press_off_the_handles_asks_for_a_pick() {
        let off = [200.0, 500.0];
        for tool in [Tool::Select, Tool::Move, Tool::Rotate, Tool::Scale] {
            let mut editor = fresh();
            editor.toolbar.tool = tool;
            editor.shown = Some(editor.camera.viewpoint());
            let pressed = press_at(&mut editor, at_the_origin(), off, MouseButton::Left, true);
            assert_eq!(pressed, ControlFlow::Continue(()), "{tool:?}");
            assert_eq!(editor.picking, Some(off), "{tool:?}");
        }
        let mut editor = fresh();
        editor.toolbar.world_camera = true;
        let pressed = press_at(&mut editor, at_the_origin(), ON_X, MouseButton::Left, true);
        assert_eq!(pressed, ControlFlow::Continue(()));
        assert_eq!(editor.picking, Some(ON_X));
        assert!(editor.toolbar.world_camera);
        for tool in [Tool::Pan, Tool::Orbit] {
            let mut editor = fresh();
            editor.toolbar.tool = tool;
            let _ = press_at(&mut editor, at_the_origin(), off, MouseButton::Left, true);
            assert_eq!(editor.picking, None, "{tool:?}");
        }
        let mut editor = fresh();
        let alt = Event::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        };
        let _ = editor.take(&alt, &reading(off, true), at_the_origin());
        assert_eq!(editor.picking, None, "with Alt");
        let editor = dragging();
        assert_eq!(editor.picking, None, "on a handle");
        let mut editor = fresh();
        editor.toolbar.tool = Tool::Move;
        editor.shown = Some(editor.camera.viewpoint());
        editor.asked = Context::seeded(0).pick([0.0, 0.0]);
        assert!(editor.asked.is_some());
        let grabbed = press_at(&mut editor, at_the_origin(), ON_X, MouseButton::Left, true);
        assert_eq!(grabbed, ControlFlow::Break(()));
        assert_eq!(editor.asked, None, "a grab drops the pick asked before it");
    }

    /// The answer to the latest pick asked for picks what it found, or unpicks for none; answers
    /// to older picks, which a later press superseded, pick nothing; and once answered, a pick is
    /// forgotten.
    #[test]
    fn the_latest_picks_answer_picks() {
        let [a, b]: [EntityId; 2] =
            ["0123456789abcdef", "fedcba9876543210"].map(|id| id.parse().expect("an ID"));
        let mut asked = Some(2_u32);
        assert_eq!(answer(&mut asked, &[]), None, "until it comes");
        assert_eq!(answer(&mut asked, &[(1, Some(a))]), None, "an older pick's");
        assert_eq!(asked, Some(2));
        let answers = [(1, Some(a)), (2, Some(b))];
        assert_eq!(answer(&mut asked, &answers), Some(Some(b)));
        assert_eq!(asked, None);
        assert_eq!(answer(&mut asked, &answers), None, "answered");
        let mut asked = Some(3);
        assert_eq!(answer(&mut asked, &[(3, None)]), Some(None), "unpicks");
    }

    /// What the view shows under the cursor is what the latest answer found, of those to picks
    /// asked since the cursor came over the view: nothing until one comes back, nor where it found
    /// nothing, and an answer that comes back after a later one's says nothing. Off the view,
    /// nothing is, and back over it, the answers to picks asked before say nothing either.
    #[test]
    fn the_latest_pick_since_the_cursor_came_over_the_view_finds_whats_under_it() {
        let [a, b]: [EntityId; 2] =
            ["0123456789abcdef", "fedcba9876543210"].map(|id| id.parse().expect("an ID"));
        let mut hover = Hover::default();
        hover.see(true, &[(1, Some(a))]);
        assert_eq!(hover.under(), None, "a pick from before it came over");
        hover.ask(Some(2_u32));
        hover.see(true, &[]);
        assert_eq!(hover.under(), None, "until one comes back");
        hover.ask(Some(3));
        hover.see(true, &[(1, Some(a)), (2, Some(b))]);
        assert_eq!(hover.under(), Some(b));
        hover.see(true, &[]);
        assert_eq!(hover.under(), Some(b), "until the next");
        // A pick outside the view, answered sooner, before one inside it asked earlier.
        hover.see(true, &[(5, None)]);
        assert_eq!(hover.under(), None, "nothing there");
        hover.see(true, &[(3, Some(a)), (4, Some(a))]);
        assert_eq!(hover.under(), None, "after a later one's");
        hover.see(true, &[(6, Some(a))]);
        assert_eq!(hover.under(), Some(a));
        hover.see(false, &[(7, Some(b))]);
        assert_eq!(hover.under(), None, "off the view");
        hover.see(true, &[(7, Some(b))]);
        hover.ask(Some(9));
        hover.see(true, &[(8, Some(b))]);
        assert_eq!(hover.under(), None, "asked before it came back over");
        hover.see(true, &[(9, Some(b))]);
        assert_eq!(hover.under(), Some(b));
    }

    /// The view's bottom left names what's under the cursor by its name, or without one by its ID,
    /// and nothing once it's gone.
    #[test]
    fn whats_under_the_cursor_shows_by_its_name_or_else_its_id() {
        let mut cx = Context::seeded(1);
        let world = cx.world_mut();
        let named = world.spawn((Name("Red".to_owned()), Transform::IDENTITY));
        let unnamed = world.spawn((Transform::IDENTITY,));
        let [named, unnamed] = [named, unnamed].map(|entity| world.id(entity).expect("alive"));
        assert_eq!(hovered_name(world, named).as_deref(), Some("Red"));
        assert_eq!(hovered_name(world, unnamed), Some(unnamed.to_string()));
        let entity = world.entity(named).expect("alive");
        world.despawn(entity).expect("alive");
        assert_eq!(hovered_name(world, named), None, "gone");
    }

    /// The view sees through the world's camera while the toolbar says to and the world has a
    /// primary one, without the editor's viewpoint; and the editor's camera's moves, a press and
    /// Home, look through it again, while a press it doesn't take leaves the view as it is.
    #[test]
    fn the_view_sees_through_the_worlds_camera_until_the_editors_moves() {
        let mut editor = fresh();
        // What a frame's update has the view see from, in a context of its own, so that no frame
        // before it set it, of the world `spawn` makes.
        let frame = |editor: &mut Editor, spawn: fn(&mut World)| {
            let mut cx = Context::seeded(1);
            spawn(cx.world_mut());
            editor.update(&mut cx, Duration::ZERO);
            cx.viewpoint()
        };
        // Without a primary camera, it can't: in a world without cameras, or in the editor's with
        // neither primary.
        let none_primary: fn(&mut World) = |world| {
            spawn_world(world);
            for (camera, _) in world.query_mut::<(&mut Camera, &Transform)>() {
                camera.primary = false;
            }
        };
        for spawn in [|_: &mut World| {}, none_primary] {
            editor.toolbar.world_camera = true;
            assert_eq!(frame(&mut editor, spawn), Some(editor.camera.viewpoint()));
            assert!(!editor.toolbar.world_camera);
        }
        let mut cx = Context::seeded(1);
        let press = |button, pressed| Event::MouseButton { button, pressed };
        let home = Event::Key {
            key: Key::Home,
            pressed: true,
            repeat: false,
        };
        for (moves, then) in [
            (
                press(MouseButton::Middle, true),
                Some(press(MouseButton::Middle, false)),
            ),
            (home, None),
        ] {
            editor.toolbar.world_camera = true;
            assert_eq!(frame(&mut editor, spawn_world), None);
            // A left press with the Select tool, which the editor's camera doesn't take.
            let select = press(MouseButton::Left, true);
            assert_eq!(editor.event(&mut cx, &select), ControlFlow::Continue(()));
            let _ = editor.event(&mut cx, &press(MouseButton::Left, false));
            assert!(editor.toolbar.world_camera);
            assert_eq!(editor.event(&mut cx, &moves), ControlFlow::Break(()));
            assert!(!editor.toolbar.world_camera);
            if let Some(then) = then {
                let _ = editor.event(&mut cx, &then);
            }
            let seen = frame(&mut editor, spawn_world);
            assert_eq!(seen, Some(editor.camera.viewpoint()));
        }
    }

    /// The editor's ticks, from tick `number` on, and where they leave the world's cameras.
    struct Ticks {
        number: u64,
    }

    impl Ticks {
        /// Where the world's cameras are after `count` more of `editor`'s ticks in `cx`'s world.
        fn run(&mut self, editor: &mut Editor, cx: &mut Context, count: u64) -> Vec<Vec3> {
            for number in self.number..self.number + count {
                editor.tick(cx.world_mut(), Tick { number });
            }
            self.number += count;
            let cameras = cx.world().query::<(&CameraController, &Transform)>();
            cameras.map(|(_, at)| at.translation).collect()
        }
    }

    /// Where the cameras `at` are after moving `by` at each of `ticks` ticks, as their script adds
    /// it.
    fn moved(at: &[Vec3], by: Vec3, ticks: u32) -> Vec<Vec3> {
        let after = |at: Vec3| (0..ticks).fold(at, |at, _| at + by);
        at.iter().map(|&at| after(at)).collect()
    }

    /// The way the editor's last command has the world's cameras go, if it's a move.
    fn last_move(editor: &Editor) -> Option<Vec2> {
        match *editor.commands.last()? {
            Command::Move(heading) => Some(heading),
            _ => None,
        }
    }

    /// W, A, S and D drive the world's cameras, by their script at the editor's ticks, while the
    /// view sees through the world's camera: at their speed, 5 units a second, a 64th of it a tick;
    /// a release stops them, and so does the view going back to the editor's camera, whichever way
    /// it does; while it's on the editor's camera, they queue nothing, and once it sees through the
    /// world's again, the cameras go the way the keys held say.
    #[test]
    fn w_a_s_and_d_drive_the_worlds_cameras_while_the_view_sees_through_them() {
        let mut editor = fresh();
        let mut cx = Context::seeded(1);
        spawn_world(cx.world_mut());
        let mut ticks = Ticks { number: 0 };
        let start = ticks.run(&mut editor, &mut cx, 0);
        assert_eq!(start.len(), 2, "both cameras have one");
        let key = |key, pressed| Event::Key {
            key,
            pressed,
            repeat: false,
        };
        // A tick's step, 5 units a second for a 64th of one.
        let step = 5.0 / 64.0;
        let [up_a_step, right_a_step] = [Vec3::Y, Vec3::X].map(|way| way * step);
        // On the editor's camera, W queues nothing.
        let _ = editor.event(&mut cx, &key(Key::KeyW, true));
        assert!(editor.commands.is_empty());
        assert_eq!(ticks.run(&mut editor, &mut cx, 4), start);
        // Seeing through the world's camera, as the toolbar's switch has it, they go up with W
        // held, a step a tick.
        editor.toolbar.world_camera = true;
        editor.update(&mut cx, Duration::ZERO);
        assert_eq!(last_move(&editor), Some(Vec2::Y));
        let up = moved(&start, up_a_step, 8);
        assert_eq!(ticks.run(&mut editor, &mut cx, 8), up);
        assert_eq!(up[0].y, 0.625, "exactly 8 of its 64ths of 5 units");
        let _ = editor.event(&mut cx, &key(Key::KeyW, false));
        assert_eq!(last_move(&editor), Some(Vec2::ZERO));
        assert_eq!(ticks.run(&mut editor, &mut cx, 4), up);
        let _ = editor.event(&mut cx, &key(Key::KeyD, true));
        let right = moved(&up, right_a_step, 4);
        assert_eq!(ticks.run(&mut editor, &mut cx, 4), right);
        // Diagonally, no faster.
        let _ = editor.event(&mut cx, &key(Key::KeyS, true));
        let across = ticks.run(&mut editor, &mut cx, 4);
        let diagonal = Vec3::new(1.0, -1.0, 0.0).normalize() * 4.0 * step;
        for (at, from) in across.iter().zip(&right) {
            assert!(
                (*at - (*from + diagonal)).length() < 1e-5,
                "{at} from {from}"
            );
        }
        let _ = editor.event(&mut cx, &key(Key::KeyS, false));
        // Each way the view goes back to the editor's camera stops them: a move of the editor's
        // camera, the world losing its primary camera, and the toolbar's switch, for which a
        // test sets the field and runs an update, since it can't run `ui`, which catches a click.
        let middle = |pressed| Event::MouseButton {
            button: MouseButton::Middle,
            pressed,
        };
        let held = ticks.run(&mut editor, &mut cx, 0);
        assert_eq!(editor.event(&mut cx, &middle(true)), ControlFlow::Break(()));
        let _ = editor.event(&mut cx, &middle(false));
        assert_eq!(last_move(&editor), Some(Vec2::ZERO));
        assert_eq!(ticks.run(&mut editor, &mut cx, 4), held);
        // Back to the world's camera with D still held: they go right again.
        editor.toolbar.world_camera = true;
        editor.update(&mut cx, Duration::ZERO);
        assert_eq!(last_move(&editor), Some(Vec2::X));
        let held = moved(&held, right_a_step, 2);
        assert_eq!(ticks.run(&mut editor, &mut cx, 2), held);
        for camera in cx.world_mut().query_mut::<&mut Camera>() {
            camera.primary = false;
        }
        // The frame's ticks after the last one goes, before its update.
        assert_eq!(ticks.run(&mut editor, &mut cx, 2), held);
        editor.update(&mut cx, Duration::ZERO);
        assert!(!editor.toolbar.world_camera);
        assert_eq!(last_move(&editor), Some(Vec2::ZERO));
        assert_eq!(ticks.run(&mut editor, &mut cx, 4), held);
        let primary = cx.world_mut().query_mut::<&mut Camera>().next();
        primary.expect("a camera").primary = true;
        editor.toolbar.world_camera = true;
        editor.update(&mut cx, Duration::ZERO);
        let held = moved(&held, right_a_step, 2);
        assert_eq!(ticks.run(&mut editor, &mut cx, 2), held);
        editor.toolbar.world_camera = false;
        editor.update(&mut cx, Duration::ZERO);
        assert_eq!(last_move(&editor), Some(Vec2::ZERO));
        assert_eq!(ticks.run(&mut editor, &mut cx, 4), held);
        let _ = editor.event(&mut cx, &key(Key::KeyW, true));
        assert!(editor.commands.is_empty());
    }

    /// Every living entity's ID in `world`, in order.
    fn ids(world: &World) -> Vec<EntityId> {
        let entities = world.entities(None).expect("every entity");
        let mut ids: Vec<EntityId> = entities.filter_map(|entity| world.id(entity)).collect();
        ids.sort();
        ids
    }

    /// New, at its tick, empties the world and forgets the scene's file, so that Save saves as.
    #[test]
    fn new_empties_the_world_and_forgets_the_scenes_file() {
        let mut editor = fresh();
        let mut cx = Context::seeded(1);
        spawn_world(cx.world_mut());
        editor.scene = Some(PathBuf::from("editor.ron"));
        editor.choose(&mut cx, FileItem::New);
        assert_eq!(ids(cx.world()).len(), 5, "until its tick");
        editor.tick(cx.world_mut(), Tick { number: 0 });
        assert_eq!(ids(cx.world()), []);
        assert_eq!(editor.scene, None);
    }

    /// An Open, at its tick, replaces the world with the scene's: the editor's own, saved and
    /// opened, has the IDs it had and saves as it did, and its file is then the scene's. One
    /// refused, for its syntax or for a type the world doesn't know, leaves the world, its serial
    /// too, and the scene's file as they were, and says why, from the line and column where its
    /// text was refused.
    #[test]
    fn an_open_replaces_the_world_unless_its_refused() {
        let mut editor = fresh();
        let mut cx = Context::seeded(1);
        spawn_world(cx.world_mut());
        let saved = whole_scene(cx.world()).expect("saved");
        let before = ids(cx.world());
        let open = |editor: &mut Editor, file: &str, text: &str| {
            editor.opening.push_back(PathBuf::from(file));
            editor.commands.push(Command::Open {
                text: text.to_owned(),
            });
        };
        let serial = cx.world().serial();
        open(&mut editor, "editor.ron", &saved);
        editor.tick(cx.world_mut(), Tick { number: 0 });
        assert_ne!(cx.world().serial(), serial, "another world");
        assert_eq!(ids(cx.world()), before);
        assert_eq!(whole_scene(cx.world()).expect("saved"), saved);
        assert_eq!(editor.scene, Some(PathBuf::from("editor.ron")));
        assert!(editor.message.is_none());
        // Without its last parenthesis; and with a type the world doesn't know in place of the
        // first camera's controller, on the line where it was.
        let unclosed = &saved[..saved.rfind(')').expect("a scene ends with one")];
        let unknown = saved.replacen("CameraController", "Unknown", 1);
        let mut lines = saved.lines();
        let line = lines.position(|line| line.contains("CameraController"));
        let line = line.expect("the cameras have controllers") + 1;
        let serial = cx.world().serial();
        for (text, at) in [(unclosed, None), (unknown.as_str(), Some(line))] {
            open(&mut editor, "broken.ron", text);
            editor.tick(cx.world_mut(), Tick { number: 1 });
            assert_eq!(cx.world().serial(), serial);
            assert_eq!(whole_scene(cx.world()).expect("saved"), saved);
            assert_eq!(editor.scene, Some(PathBuf::from("editor.ron")));
            let message = editor.message.take().expect("refused");
            assert_eq!(message.title, "Can't open broken.ron");
            let location = message.text.split(':').take(2);
            let location: Vec<Option<usize>> = location.map(|part| part.parse().ok()).collect();
            assert!(
                matches!(location[..], [Some(_), Some(_)]),
                "{}",
                message.text
            );
            if let Some(line) = at {
                assert_eq!(location[0], Some(line), "{}", message.text);
            }
        }
    }

    /// A directory of a test's own in the OS's temporary directory, made empty, and removed as the
    /// test ends, failed or not.
    pub(crate) struct Scratch(pub(crate) PathBuf);

    impl Scratch {
        pub(crate) fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("gg-editor-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir(&dir).expect("made");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The world saved and opened again saves the same bytes: through Save As, which gives a name
    /// without an extension `.ron`, Open, after New has emptied the world, and Save, which writes
    /// to the scene's file. A file that can't be read, or written, as in a directory that isn't
    /// there, says why, and leaves the scene's file as it was.
    #[test]
    fn the_world_saved_and_opened_again_saves_the_same_bytes() {
        let scratch = Scratch::new("saved-and-opened");
        let directory = &scratch.0;
        let mut editor = fresh();
        let mut cx = Context::seeded(1);
        spawn_world(cx.world_mut());
        editor.save_as(cx.world(), directory.join("scene"));
        let path = directory.join("scene.ron");
        assert_eq!(editor.scene.as_ref(), Some(&path));
        let saved = fs::read(&path).expect("saved");
        editor.choose(&mut cx, FileItem::New);
        editor.open(path.clone());
        editor.tick(cx.world_mut(), Tick { number: 0 });
        assert!(editor.message.is_none());
        assert_eq!(editor.scene.as_ref(), Some(&path));
        fs::remove_file(&path).expect("removed");
        editor.choose(&mut cx, FileItem::Save);
        assert_eq!(fs::read(&path).expect("saved again"), saved);
        editor.open(directory.join("absent.ron"));
        let message = editor.message.take().expect("not read");
        assert_eq!(message.title, "Can't open absent.ron");
        assert!(editor.commands.is_empty() && editor.opening.is_empty());
        editor.save_as(cx.world(), directory.join("absent").join("scene.ron"));
        let message = editor.message.take().expect("not written");
        assert_eq!(message.title, "Can't save scene.ron");
        assert_eq!(editor.scene, Some(path));
    }

    /// Save As puts `.ron` after a name that doesn't end in it, one with another extension too, as
    /// the XDG portal's dialog may leave them; and replaces no file at the name it gives, which the
    /// dialog didn't ask about, saying so instead.
    #[test]
    fn save_as_ends_the_name_in_ron_and_replaces_no_file_unasked() {
        let scratch = Scratch::new("save-as");
        let mut editor = fresh();
        let mut cx = Context::seeded(1);
        spawn_world(cx.world_mut());
        for (picked, saved) in [
            ("level", "level.ron"),
            ("level.1", "level.1.ron"),
            ("Boss.RON", "Boss.RON"),
        ] {
            editor.save_as(cx.world(), scratch.0.join(picked));
            assert!(editor.message.is_none(), "{picked}");
            assert_eq!(editor.scene, Some(scratch.0.join(saved)));
        }
        let kept = scratch.0.join("level.ron");
        fs::write(&kept, "kept").expect("written");
        editor.save_as(cx.world(), scratch.0.join("level"));
        let message = editor.message.take().expect("refused");
        assert_eq!(message.title, "Can't save level.ron");
        assert_eq!(fs::read_to_string(&kept).expect("read"), "kept");
        assert_eq!(editor.scene, Some(scratch.0.join("Boss.RON")));
    }

    /// An Open stops the world's cameras, whichever way the scene was saved with them going, since
    /// only keys held while the view sees through the world's camera drive them: the next update
    /// has them go the way the keys held say if it does, and leaves them stopped if it doesn't.
    #[test]
    fn an_open_stops_the_worlds_cameras_until_the_keys_drive_them() {
        let mut editor = fresh();
        let mut cx = Context::seeded(1);
        spawn_world(cx.world_mut());
        let mut ticks = Ticks { number: 0 };
        let headings = |world: &World| {
            let controllers = world.query::<&CameraController>();
            controllers
                .map(|controller| controller.heading)
                .collect::<Vec<_>>()
        };
        // Seeing through the world's camera with W held, they go up, and are saved going up.
        editor.toolbar.world_camera = true;
        editor.update(&mut cx, Duration::ZERO);
        let w = Event::Key {
            key: Key::KeyW,
            pressed: true,
            repeat: false,
        };
        let _ = editor.event(&mut cx, &w);
        ticks.run(&mut editor, &mut cx, 1);
        assert_eq!(headings(cx.world()), [Vec2::Y; 2]);
        let going_up = whole_scene(cx.world()).expect("saved");
        editor.commands.push(Command::Open {
            text: going_up.clone(),
        });
        ticks.run(&mut editor, &mut cx, 1);
        assert_eq!(headings(cx.world()), [Vec2::ZERO; 2]);
        assert!(!editor.gated);
        editor.update(&mut cx, Duration::ZERO);
        assert_eq!(last_move(&editor), Some(Vec2::Y));
        ticks.run(&mut editor, &mut cx, 1);
        assert_eq!(headings(cx.world()), [Vec2::Y; 2]);
        // Back on the editor's camera, W still held.
        editor.toolbar.world_camera = false;
        editor.update(&mut cx, Duration::ZERO);
        editor.commands.push(Command::Open { text: going_up });
        ticks.run(&mut editor, &mut cx, 1);
        assert_eq!(headings(cx.world()), [Vec2::ZERO; 2]);
        assert!(!editor.gated);
        editor.update(&mut cx, Duration::ZERO);
        assert!(editor.commands.is_empty());
    }

    /// `key` pressed with `modifiers`, as egui gets it, or repeating as it's held, if `repeat`.
    fn pressed(key: egui::Key, modifiers: Modifiers, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        }
    }

    /// Each of the File menu's shortcuts chooses its item, Ctrl+Shift+S Save As rather than Save,
    /// and is taken from egui's input; a repeat chooses nothing, and is taken too; and keys without
    /// Ctrl, or Cmd on macOS, or another's shortcut, are left as they were.
    #[test]
    fn the_shortcuts_choose_the_file_menus_items() {
        // The command key as egui-winit sends it: with Ctrl on Windows and Linux, Cmd on macOS.
        for key in [Modifiers::CTRL, Modifiers::MAC_CMD] {
            let command = key.plus(Modifiers::COMMAND);
            let command_shift = command.plus(Modifiers::SHIFT);
            for (key, modifiers, item) in [
                (egui::Key::N, command, FileItem::New),
                (egui::Key::O, command, FileItem::Open),
                (egui::Key::S, command, FileItem::Save),
                (egui::Key::S, command_shift, FileItem::SaveAs),
            ] {
                let mut events = vec![pressed(key, modifiers, false)];
                assert_eq!(file_shortcut(&mut events), Some(item));
                assert_eq!(events, []);
                let mut events = vec![pressed(key, modifiers, true)];
                assert_eq!(file_shortcut(&mut events), None, "{item:?} repeating");
                assert_eq!(events, []);
            }
            // S alone and with Shift, and the Inspector's Undo.
            let others = vec![
                pressed(egui::Key::S, Modifiers::NONE, false),
                pressed(egui::Key::S, Modifiers::SHIFT, false),
                pressed(egui::Key::Z, command, false),
            ];
            let mut events = others.clone();
            assert_eq!(file_shortcut(&mut events), None);
            assert_eq!(events, others);
        }
    }

    /// A key pressed with Ctrl held, or Cmd on macOS, is a shortcut's, such as Ctrl+S's S, and
    /// drives none of the world's cameras while the view sees through them; its release still lets
    /// go of a key held before, so that S held, then Ctrl, then S let go, stops them.
    #[test]
    fn a_shortcuts_key_drives_no_camera() {
        let mut editor = fresh();
        let mut cx = Context::seeded(1);
        spawn_world(cx.world_mut());
        editor.toolbar.world_camera = true;
        editor.update(&mut cx, Duration::ZERO);
        editor.commands.clear();
        let s = |pressed| Event::Key {
            key: Key::KeyS,
            pressed,
            repeat: false,
        };
        editor.drive(&s(true), true);
        editor.drive(&s(false), true);
        assert!(editor.commands.is_empty());
        editor.drive(&s(true), false);
        assert_eq!(last_move(&editor), Some(Vec2::NEG_Y));
        editor.drive(&s(false), true);
        assert_eq!(last_move(&editor), Some(Vec2::ZERO));
    }

    /// A message shows until it's dismissed: by Escape, or a click outside it.
    #[test]
    fn a_message_shows_until_its_dismissed() {
        let mut editor = fresh();
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let frame = |editor: &mut Editor, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..egui::RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| editor.show_message(ui.ctx()));
            // As the renderer would have.
            output.textures_delta.clear();
        };
        // A corner, outside the message, which shows in the middle.
        let corner = egui::pos2(4.0, 4.0);
        let click = |pressed| egui::Event::PointerButton {
            pos: corner,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let escape = vec![pressed(egui::Key::Escape, Modifiers::NONE, false)];
        let outside = vec![egui::Event::PointerMoved(corner), click(true), click(false)];
        for dismissal in [escape, outside] {
            editor.message = Some(Message {
                title: "Can't open scene.ron".into(),
                text: "why".into(),
            });
            frame(&mut editor, Vec::new());
            assert!(editor.message.is_some());
            // A frame each, as egui gets them.
            for event in dismissal {
                frame(&mut editor, vec![event]);
            }
            assert!(editor.message.is_none());
        }
    }

    /// New and Open are values that a recording keeps and a replay queues again, through the
    /// editor's queue, Open with the scene's text; and a move, as recordings from before them hold
    /// it, still reads as one.
    #[test]
    fn the_file_menus_commands_are_values_a_recording_keeps() {
        let mut editor = fresh();
        let text = "(\n    entities: {},\n)\n";
        editor.commands.push(Command::New);
        editor.commands.push(Command::Open {
            text: text.to_owned(),
        });
        let queue = editor.commands().expect("the editor queues commands");
        let values: Vec<Value> = (0..queue.len()).map(|i| queue.value(i)).collect();
        let open = vec![("text".into(), Value::String(text.to_owned()))];
        let expected = [
            Value::Variant("New".into(), Vec::new()),
            Value::Variant("Open".into(), open),
        ];
        assert_eq!(values, expected);
        queue.clear();
        let heading = Vec2::new(-1.0, 0.5);
        let moved = Value::Variant("Move".into(), vec![("0".into(), Value::Vec2(heading))]);
        for value in values.iter().chain([&moved]) {
            queue.push(value).expect("a command");
        }
        assert!(matches!(
            &editor.commands[..],
            [Command::New, Command::Open { text: read }, Command::Move(to)]
                if read == text && *to == heading
        ));
    }

    /// Every scene in the editor's assets folder, and in the folders under it, opens in the
    /// editor's world, as one dropped from the Assets panel does, and saves as it was saved: so a
    /// change to the editor's types, or to how scenes are written, that leaves one behind fails.
    #[test]
    fn the_assets_scenes_open_and_save_as_they_were_saved() {
        let mut folders = vec![assets_folder()];
        let mut scenes = Vec::new();
        while let Some(folder) = folders.pop() {
            for entry in fs::read_dir(&folder).expect("listed") {
                let path = entry.expect("listed").path();
                if path.is_dir() {
                    folders.push(path);
                } else if is_scene(&path) {
                    scenes.push(path);
                }
            }
        }
        assert!(!scenes.is_empty(), "the assets folder holds a scene");
        for path in scenes {
            let mut cx = Context::seeded(1);
            // As the editor starts, which registers its types with the world.
            spawn_world(cx.world_mut());
            let text = gg_engine::read_scene(&path).expect("read");
            let opened = cx.world_mut().replace_scene(&text);
            opened.unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
            let saved = whole_scene(cx.world()).expect("saved");
            assert_eq!(saved, text, "{}", path.display());
        }
    }

    /// The text drawn in `shapes`, a frame's output, each with where it was drawn.
    pub(crate) fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect)> {
        fn find(shape: &egui::Shape, texts: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        find(shape, texts);
                    }
                }
                egui::Shape::Text(text) => {
                    texts.push((text.galley.text().to_owned(), text.visual_bounding_rect()));
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in shapes {
            find(&clipped.shape, &mut texts);
        }
        texts
    }

    /// Where `text` was drawn among `texts`.
    pub(crate) fn find(texts: &[(String, egui::Rect)], text: &str) -> egui::Rect {
        let found = texts.iter().find(|(drawn, _)| drawn == text);
        found.unwrap_or_else(|| panic!("no {text}: {texts:?}")).1
    }

    /// The events of a drag with the primary button from `from` to `to`, a frame's each, as egui
    /// gets them: past how far a click goes in the frame after the press, then to `to`, then let go.
    pub(crate) fn drag(from: egui::Pos2, to: egui::Pos2) -> Vec<egui::Event> {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
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

    /// A scene dragged from the Assets panel and let go over the view opens, as Open opens one: its
    /// text is queued for the next tick, and its file is the scene's then. One let go elsewhere
    /// opens nothing, nor does another kind of file, which doesn't drag.
    #[test]
    fn a_scene_dragged_from_the_assets_panel_onto_the_view_opens() {
        let scratch = Scratch::new("dropped");
        let mut cx = Context::seeded(1);
        spawn_world(cx.world_mut());
        let scene = scratch.0.join("level.ron");
        fs::write(&scene, whole_scene(cx.world()).expect("saved")).expect("written");
        fs::write(scratch.0.join("logo.png"), "not a scene").expect("written");
        let mut editor = fresh();
        editor.assets = Assets::new(scratch.0.clone());
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 400.0));
        let (panel, view) = screen.split_left_right_at_x(400.0);
        // A frame of the panel beside a view that senses presses as the dock's does.
        let frame = |editor: &mut Editor, event: Option<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events: event.into_iter().collect(),
                ..egui::RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let sense = egui::Sense::CLICK | egui::Sense::DRAG;
                let response = ui.interact(view, Id::unique("test view"), sense);
                let builder = egui::UiBuilder::new().max_rect(panel);
                ui.scope_builder(builder, |ui| editor.assets.show(ui));
                editor.take_drop(ui.ctx(), &response);
            });
            output.textures_delta.clear();
            output.shapes
        };
        // The drag from `from` to `to`, and whether each of its frames outlined the view.
        let drop = |editor: &mut Editor, from: egui::Rect, to| {
            let frames = drag(from.center(), to).into_iter();
            let shapes = frames.map(|event| frame(editor, Some(event)));
            let outlined = |shapes: Vec<egui::epaint::ClippedShape>| {
                let outline = |shape: &_| matches!(shape, egui::Shape::Rect(r) if r.rect == view);
                shapes.iter().any(|clipped| outline(&clipped.shape))
            };
            shapes.map(outlined).collect::<Vec<bool>>()
        };
        frame(&mut editor, None);
        let shown = texts(&frame(&mut editor, None));
        let (level, logo) = (find(&shown, "level.ron"), find(&shown, "logo.png"));
        let elsewhere = egui::pos2(200.0, 350.0);
        for (from, to) in [(logo, view.center()), (level, elsewhere)] {
            assert_eq!(drop(&mut editor, from, to), [false; 6]);
            assert!(editor.commands.is_empty() && editor.opening.is_empty());
        }
        // Outlined once it's over the view, until it's let go.
        let outlined = drop(&mut editor, level, view.center());
        assert_eq!(outlined, [false, false, false, true, true, true]);
        let text = fs::read_to_string(&scene).expect("read");
        assert!(matches!(&editor.commands[..], [Command::Open { text: read }] if *read == text));
        assert_eq!(editor.opening, [scene]);
    }
}
