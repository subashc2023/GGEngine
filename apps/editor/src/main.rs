//! The editor: the game's view in a panel among the editor's own, in a dock that fills its window.
//! Until the tool API brings it a game, the view shows a world of the editor's own, three squares
//! and two cameras, through the editor's camera, orthographic at first, as 2D games see theirs, over a
//! grid, with a toolbar and a gizmo over it; or through the world's camera, as the game would, and
//! then W, A, S and D drive the world's cameras, by a script of theirs, as a game's keys would. The
//! Inspector edits the world, and undoes and redoes its edits.

mod camera;
mod grid;
mod overlay;

use std::f32::consts::FRAC_PI_3;
use std::ops::ControlFlow;
use std::process::ExitCode;
use std::time::Duration;

use gg_engine::egui::{Align2, CursorIcon, ViewportCommand, ViewportId};
use gg_engine::math::{Vec2, Vec3, Vec4};
use gg_engine::{
    Actions, App, Binding, Camera, Change, Commands, Component, Context, Docked, Entity, Event,
    Inspector, Key, Layer, Name, PerfPanel, Projection, Reflect, Script, Source, Sprite, Tick,
    Transform, Ui, World, run_scripts,
};

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

struct Editor {
    camera: OrbitCamera,
    toolbar: Toolbar,
    /// The grid, once attached.
    grid: Option<Grid>,
    /// What drives the world's cameras.
    actions: Actions<Act>,
    /// What the actions, and the toolbar's switch, changed since the last tick, which it applies.
    commands: Vec<Command>,
    /// Whether the world's cameras take the keys: whether the view saw through the world's camera
    /// as the commands last heard.
    gated: bool,
}

/// What the editor's keys do to its world, beside what its camera's do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// W, A, S and D: which way the world's cameras go, while the view sees through the world's
    /// camera.
    Move,
}

/// What the editor's keys, and its toolbar's switch, change of its world, queued as they change it
/// and applied at its next tick: its commands, which a recording keeps.
#[derive(Reflect)]
enum Command {
    /// Has the world's cameras go this way, across and up, until the next.
    Move(Vec2),
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
}

/// The script that drives the world's cameras, as a game's would: it moves its entity across and up
/// at its speed, the way its heading says, which the editor's W, A, S and D set while the view sees
/// through the world's camera.
#[derive(Clone, Copy, Component)]
struct CameraController {
    /// How fast it goes, in units a second.
    #[reflect(min = 0.0)]
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

impl Layer for Editor {
    fn attach(&mut self, cx: &mut Context) {
        cx.set_clear_color([0.02, 0.02, 0.025, 1.0]);
        self.grid = Some(Grid::new(cx));
        spawn_world(cx.world_mut());
    }

    fn tick(&mut self, world: &mut World, tick: Tick) {
        for command in self.commands.drain(..) {
            match command {
                Command::Move(heading) => {
                    for controller in world.query_mut::<&mut CameraController>() {
                        controller.heading = heading;
                    }
                }
            }
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
        let handled = self.camera.event(event, screen, self.toolbar.tool);
        // What the editor's camera takes moves it, from where it was: the view goes back to it.
        if handled.is_break() {
            self.toolbar.world_camera = false;
        }
        // Queued now, rather than at the next update, so that the next tick applies them, while
        // the world's cameras take the keys; the actions see every event, so that none misses
        // the release of a key it saw pressed.
        let Self {
            actions,
            commands,
            gated,
            ..
        } = self;
        let gated = *gated;
        actions.event(event, |change| {
            if gated && let Change::Axis2(Act::Move, heading) = change {
                commands.push(Command::Move(heading));
            }
        });
        self.gate();
        handled
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        ui.menu("File", |ui| {
            if ui.button("Quit").clicked() {
                ui.ctx()
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
        });
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
    use gg_engine::MouseButton;

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

    /// The way the editor's last command has the world's cameras go, if it queued one.
    fn last_move(editor: &Editor) -> Option<Vec2> {
        editor
            .commands
            .last()
            .map(|&Command::Move(heading)| heading)
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
}
