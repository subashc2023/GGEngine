//! The editor: the game's view in a panel among the editor's own, in a dock that fills its window.
//! Until the tool API brings it a game, the view shows a world of the editor's own, three squares
//! and two cameras, through the editor's camera, orthographic at first, as 2D games see theirs, over a
//! grid, with a toolbar and a gizmo over it; or through the world's camera, as the game would, and
//! then W, A, S and D drive the world's cameras, by a script of theirs, as a game's keys would. The
//! Inspector edits the world, and undoes and redoes its edits. The File menu starts a new scene,
//! emptying the world; opens one, whose entities replace the world's; and saves the world's
//! entities as one, to the file it was opened from or saved to last, or as another: through the
//! OS's file dialogs, and with Ctrl+N, Ctrl+O, Ctrl+S and Ctrl+Shift+S, Cmd for Ctrl on macOS.
//! When a file can't be read, opened or written, it says why, until that's dismissed.

mod camera;
mod grid;
mod overlay;

use std::collections::VecDeque;
use std::f32::consts::FRAC_PI_3;
use std::ffi::OsStr;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use gg_engine::egui::{
    self, Align2, Button, CursorIcon, Id, KeyboardShortcut, Modal, Modifiers, ViewportCommand,
    ViewportId,
};
use gg_engine::math::{Vec2, Vec3, Vec4};
use gg_engine::reflect::Error;
use gg_engine::{
    Actions, App, Binding, Camera, Change, Commands, Component, Context, Docked, Entity, Event,
    FileDialog, Inspector, Key, Layer, Name, PerfPanel, Projection, Reflect, Script, Source,
    Sprite, Tick, Transform, Ui, World, run_scripts,
};
use tracing::warn;

use camera::{Move, OrbitCamera, Screen, Side, Tool};
use grid::Grid;
use overlay::Toolbar;

fn main() -> ExitCode {
    gg_engine::run(app())
}

fn app() -> App {
    let app = App::new("Editor").custom_title_bar().view_panel("Viewport");
    let app = app
        .dock("Inspector", Docked::RightOf("Viewport"))
        .dock("Entities", Docked::LeftOf("Viewport"))
        .dock("Performance", Docked::Below("Inspector"));
    let app = app.layer(Editor::new());
    app.overlay(PerfPanel).overlay(Inspector::default())
}

/// The editor's layer: its camera, and the toolbar and gizmo over the view; the grid; the keys
/// that drive the world's cameras; and the File menu, which empties the world for a new scene,
/// replaces it with a scene's and saves it as one, keeping the file it was opened from or saved
/// to last for Save to write to, and says why when a file can't be read, opened or written.
struct Editor {
    camera: OrbitCamera,
    toolbar: Toolbar,
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
    fn new() -> Self {
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
            grid: None,
            actions,
            commands: Vec::new(),
            gated: false,
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
        let ron = picked
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("ron"));
        if !ron {
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

    /// Saves `world`, as the last tick left it, to the file at `path`, which is then the scene's;
    /// or, if it can't, says why.
    fn save(&mut self, world: &World, path: PathBuf) {
        let saved = whole_scene(world).and_then(|text| gg_engine::write_scene(&path, &text));
        match saved {
            Ok(()) => self.scene = Some(path),
            Err(error) => self.message = Some(Message::refused("save", Some(&path), &error)),
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

/// `world`'s roots and their descendants, every entity it has, as a scene's text.
fn whole_scene(world: &World) -> Result<String, Error> {
    let roots: Vec<Entity> = world.roots().collect();
    world.save_scene(&roots)
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
        if self.toolbar.world_camera {
            return;
        }
        let camera = self.camera.viewpoint();
        cx.set_viewpoint(camera);
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
        let handled = self.camera.event(event, screen, self.toolbar.tool);
        // What the editor's camera takes moves it, from where it was: the view goes back to it.
        if handled.is_break() {
            self.toolbar.world_camera = false;
        }
        self.drive(event, command);
        self.gate();
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
        self.show_message(ui.egui());
        let Self {
            camera, toolbar, ..
        } = self;
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
                Tool::Select => None,
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
    use gg_engine::{EntityId, MouseButton};

    use super::*;

    /// The view sees through the world's camera while the toolbar says to and the world has a
    /// primary one, without the editor's viewpoint; and the editor's camera's moves, a press and
    /// Home, look through it again, while a press it doesn't take leaves the view as it is.
    #[test]
    fn the_view_sees_through_the_worlds_camera_until_the_editors_moves() {
        let mut editor = Editor::new();
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
        let mut editor = Editor::new();
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
        let mut editor = Editor::new();
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
        let mut editor = Editor::new();
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
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
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
        let mut editor = Editor::new();
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
        let mut editor = Editor::new();
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
        let mut editor = Editor::new();
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
        let mut editor = Editor::new();
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
        let mut editor = Editor::new();
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
        let mut editor = Editor::new();
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
}
