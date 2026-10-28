//! The editor: the game's view in a panel among the editor's own, in a dock that fills its window.
//! Until the tool API brings it a game, the view shows a world of the editor's own, three squares
//! and two cameras, through the editor's camera, orthographic at first, as 2D games see theirs, over a
//! grid, with a toolbar and a gizmo over it; or through the world's camera, as the game would. The
//! Inspector edits the world, and undoes and redoes its edits.

mod camera;
mod grid;
mod overlay;

use std::f32::consts::FRAC_PI_3;
use std::ops::ControlFlow;
use std::process::ExitCode;
use std::time::Duration;

use gg_engine::egui::{Align2, CursorIcon, ViewportCommand, ViewportId};
use gg_engine::math::{Vec3, Vec4};
use gg_engine::{
    App, Camera, Context, Docked, Event, Inspector, Key, Layer, Name, PerfPanel, Projection,
    Sprite, Transform, Ui, World,
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
}

impl Editor {
    fn new() -> Self {
        Self {
            camera: OrbitCamera::new(),
            toolbar: Toolbar::default(),
            grid: None,
        }
    }
}

/// Spawns the world the editor starts with: three squares, "Red", "Green" and "Blue", side by side,
/// and two cameras in front of them, of which the view sees through the primary, "Camera",
/// orthographic, 8 units tall, until the Inspector makes "Second camera", with perspective,
/// primary in its place. Each square is as much bigger as it's farther from "Second camera", so
/// that through it they look the same size, a unit 3 units in front of it, a quarter of a unit
/// apart; orthographic, which shows nothing smaller for being farther, shows how big they are.
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
    let orthographic = Camera {
        projection: Projection::Orthographic {
            height: 8.0,
            near: 0.1,
            far: 100.0,
        },
        primary: true,
    };
    world.spawn((named("Camera"), in_front(10.0), orthographic));
    let perspective = Camera {
        projection: Projection::Perspective {
            fov: FRAC_PI_3,
            near: 0.1,
        },
        primary: false,
    };
    world.spawn((named("Second camera"), in_front(SEEN_FROM), perspective));
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

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        self.camera.update(dt);
        let plane = grid::shown(&self.camera.viewpoint(), self.toolbar.plane);
        self.camera.settle(plane.normal());
        // Off once the world has no primary camera, as its ticks left it, so that no frame shows
        // the default viewpoint as the last one goes.
        self.toolbar.world_camera &= has_camera(cx.world());
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
}
