//! Development playground: test scenes and debug tooling for engine work.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use bytemuck::NoUninit;
use gg_engine::egui::{ViewportCommand, ViewportId};
use gg_engine::math::{Mat4, Quat, Vec3};
use gg_engine::{Camera, Context, Event, Key, Layer, Mesh, Projection, Shader, Ui};
use tracing::info;

fn main() -> ExitCode {
    let app = gg_engine::App::new("Sandbox").custom_title_bar();
    gg_engine::run(app.layer(Sandbox::new()))
}

/// A corner of a mesh in one color, which `shaders/flat_color.slang` reads: its position in the
/// mesh's own space.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Position {
    position: [f32; 3],
}

/// A corner of a mesh in the colors of its corners, which `shaders/vertex_color.slang` reads: its
/// position in the mesh's own space, and its color in linear RGBA.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Colored {
    position: [f32; 3],
    color: [f32; 4],
}

/// A square a unit across, as two triangles.
const SQUARE: [Position; 4] = [
    Position {
        position: [-0.5, -0.5, 0.0],
    },
    Position {
        position: [0.5, -0.5, 0.0],
    },
    Position {
        position: [0.5, 0.5, 0.0],
    },
    Position {
        position: [-0.5, 0.5, 0.0],
    },
];
const SQUARE_INDICES: [u32; 6] = [0, 1, 2, 2, 3, 0];

/// A triangle in red, green and blue, a unit across, pointing up.
const TRIANGLE: [Colored; 3] = [
    Colored {
        position: [0.0, 0.5, 0.0],
        color: [1.0, 0.0, 0.0, 1.0],
    },
    Colored {
        position: [0.5, -0.5, 0.0],
        color: [0.0, 1.0, 0.0, 1.0],
    },
    Colored {
        position: [-0.5, -0.5, 0.0],
        color: [0.0, 0.0, 1.0, 1.0],
    },
];

/// How fast the camera moves, in world units a second.
const CAMERA_SPEED: f32 = 1.5;
/// How fast the camera and the triangle turn, in radians a second: a quarter turn.
const TURN_SPEED: f32 = FRAC_PI_2;

/// Draws a grid of blue squares in the game's view, and a triangle in red, green and blue turning
/// over them, each mesh with a vertex type and shader of its own, over a color that drifts slowly,
/// so that a running sandbox visibly animates. W, A, S and D move the camera along its own axes,
/// and Q and E turn it left and right, at the same speed whatever the frame rate. Shows two panels
/// to dock: frame times and the camera, and the color, which can be held or picked. Logs the key
/// and mouse presses that reach it, which those on the panels don't. It polls input too: holding
/// Space pauses the drift, and a panel shows where the cursor is. The title bar's File menu quits.
struct Sandbox {
    time: Duration,
    drift: bool,
    /// The clear color, in linear RGB.
    color: [f32; 3],
    /// The last frame's time, shown as it is: smoothing would hide uneven frames.
    frame_time: Duration,
    camera: Camera,
    /// How far the camera and the triangle have turned counterclockwise, in radians from 0 to a
    /// whole turn.
    camera_turn: f32,
    triangle_turn: f32,
    /// Added once the sandbox attaches.
    square: Option<(Mesh<Position>, Shader<Position>)>,
    triangle: Option<(Mesh<Colored>, Shader<Colored>)>,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            time: Duration::ZERO,
            drift: true,
            color: [0.0; 3],
            frame_time: Duration::ZERO,
            // 3 units from the view's bottom to its top, around the grid's 2.
            camera: Camera {
                projection: Projection::Orthographic {
                    height: 3.0,
                    near: -1.0,
                    far: 1.0,
                },
                ..Camera::default()
            },
            camera_turn: 0.0,
            triangle_turn: 0.0,
            square: None,
            triangle: None,
        }
    }
}

impl Layer for Sandbox {
    fn attach(&mut self, cx: &mut Context) {
        // A shader that doesn't compile stops the sandbox, with slangc's diagnostics.
        let shaders = Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
        let flat = cx.add_shader("flat color", shaders.join("flat_color.slang"));
        let colored = cx.add_shader("vertex color", shaders.join("vertex_color.slang"));
        let square = cx.add_mesh("square", &SQUARE, &SQUARE_INDICES);
        let triangle = cx.add_mesh("triangle", &TRIANGLE, &[0, 1, 2]);
        let whole = "the meshes are whole triangles, of their own vertices";
        self.square = Some((
            square.expect(whole),
            flat.expect("the flat color shader compiles"),
        ));
        self.triangle = Some((
            triangle.expect(whole),
            colored.expect("the vertex color shader compiles"),
        ));
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        let seconds = dt.as_secs_f32();
        let held = |key| if cx.input().key_down(key) { 1.0 } else { 0.0 };
        let turn = held(Key::KeyQ) - held(Key::KeyE);
        // Along the camera's own axes, so that W moves up the screen however it's turned, and as
        // fast diagonally as straight.
        let along = Vec3::new(
            held(Key::KeyD) - held(Key::KeyA),
            held(Key::KeyW) - held(Key::KeyS),
            0.0,
        );
        self.camera_turn = (self.camera_turn + turn * TURN_SPEED * seconds).rem_euclid(TAU);
        self.camera.rotation = Quat::from_rotation_z(self.camera_turn);
        self.camera.position +=
            self.camera.rotation * along.normalize_or_zero() * CAMERA_SPEED * seconds;
        cx.set_camera(self.camera);

        if let Some((mesh, shader)) = self.square {
            // 10 by 10, 2 units across, each square a little smaller than the spacing.
            for row in 0..10 {
                for column in 0..10 {
                    let at = Vec3::new(column as f32 - 4.5, row as f32 - 4.5, 0.0) * 0.2;
                    let transform =
                        Mat4::from_translation(at) * Mat4::from_scale(Vec3::splat(0.18));
                    cx.draw(mesh, shader, transform);
                }
            }
        }
        self.triangle_turn = (self.triangle_turn + TURN_SPEED * seconds).rem_euclid(TAU);
        if let Some((mesh, shader)) = self.triangle {
            cx.draw(mesh, shader, Mat4::from_rotation_z(self.triangle_turn));
        }

        self.frame_time = dt;
        if self.drift && !cx.input().key_down(Key::Space) {
            self.time += dt;
            let t = self.time.as_secs_f32() * 0.4;
            let wave = |phase: f32| 0.02 + 0.06 * (0.5 + 0.5 * (t + phase).sin());
            self.color = [wave(0.0), wave(2.1), wave(4.2)];
        }
    }

    fn event(&mut self, _: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        match *event {
            Event::Key {
                key,
                pressed: true,
                repeat: false,
            } => info!(?key, "Key pressed"),
            Event::MouseButton {
                button,
                pressed: true,
            } => info!(?button, "Mouse button pressed"),
            _ => {}
        }
        ControlFlow::Continue(())
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        ui.menu("File", |ui| {
            if ui.button("Quit").clicked() {
                ui.ctx()
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
        });
        ui.panel("Stats", |ui| {
            let ms = self.frame_time.as_secs_f32() * 1000.0;
            let fps = if ms > 0.0 { 1000.0 / ms } else { 0.0 };
            ui.label(format!("{ms:.2} ms a frame, {fps:.0} fps"));
            let [x, y, _] = self.camera.position.to_array();
            ui.label(format!(
                "Camera at {x:.2}, {y:.2}, turned {:.0}°",
                self.camera_turn.to_degrees()
            ));
            ui.label(match cx.input().cursor() {
                Some([x, y]) => format!("Cursor at {x:.0}, {y:.0}"),
                None => "Cursor outside the window".to_owned(),
            });
        });
        ui.panel("Clear color", |ui| {
            ui.checkbox(&mut self.drift, "Drift");
            ui.horizontal(|ui| {
                ui.label("Color");
                // Picking a color holds it.
                if ui.color_edit_button_rgb(&mut self.color).changed() {
                    self.drift = false;
                }
            });
        });
        // After the panel, which may have changed it.
        let [r, g, b] = self.color;
        cx.set_clear_color([r, g, b, 1.0]);
    }
}
