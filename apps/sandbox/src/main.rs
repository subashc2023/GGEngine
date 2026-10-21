//! Development playground: test scenes and debug tooling for engine work.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use bytemuck::NoUninit;
use gg_engine::egui::{self, ViewportCommand, ViewportId};
use gg_engine::math::{EulerRot, Mat4, Vec2, Vec3, Vec4};
use gg_engine::{
    Blend, Camera, Context, Event, Key, Layer, Mesh, OrthographicCameraController, Projection,
    Sampler, Shader, Sprite, Texture, Ui,
};
use tracing::info;

fn main() -> ExitCode {
    let app = gg_engine::App::new("Sandbox").custom_title_bar();
    gg_engine::run(app.layer(Sandbox::new()))
}

/// A corner of a mesh in the colors of its corners, which `shaders/vertex_color.slang` reads: its
/// position in the mesh's own space, and its color in linear RGBA. The color is an array, since a
/// `Vec4` is 16-byte aligned, which would leave padding after the position.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Colored {
    position: Vec3,
    color: [f32; 4],
}

/// A triangle in red, green and blue, a unit across, pointing up.
const TRIANGLE: [Colored; 3] = [
    Colored {
        position: Vec3::new(0.0, 0.5, 0.0),
        color: [1.0, 0.0, 0.0, 1.0],
    },
    Colored {
        position: Vec3::new(0.5, -0.5, 0.0),
        color: [0.0, 1.0, 0.0, 1.0],
    },
    Colored {
        position: Vec3::new(-0.5, -0.5, 0.0),
        color: [0.0, 0.0, 1.0, 1.0],
    },
];

/// How fast the triangle turns, in radians a second: a quarter turn.
const TURN_SPEED: f32 = FRAC_PI_2;

/// The textures from `assets/`: the checkerboard, and the logo drawn over it.
struct Assets {
    checkerboard: Texture,
    logo: Texture,
}

/// Draws a grid of sprites in the game's view, in two colors that alternate; over its middle, a
/// checkerboard sprite, tiled and tinted, then a triangle in red, green and blue turning, then a
/// logo sprite whose alpha shows them through it. The triangle is a mesh with a vertex type and
/// shader of the sandbox's own, the rest the engine's sprites, and they're drawn over a color that
/// drifts slowly, so that a running sandbox visibly animates. The engine's camera controller steers
/// the camera: W, A, S and D move it along its own axes, Q and E turn it left and right, at the
/// same speed whatever the frame rate, and the mouse wheel zooms it. Shows four panels to dock:
/// frame times and the camera; the grid's colors, which can be picked; the checkerboard's
/// filtering, smooth or sharp, how many times it repeats, and its tint; and the clear color, which
/// can be held or picked. Logs the key and mouse presses that reach it, which those on the panels
/// don't. It polls input too: holding Space pauses the drift, and a panel shows where the cursor
/// is. The title bar's File menu quits.
struct Sandbox {
    time: Duration,
    drift: bool,
    /// The clear color, in linear RGB.
    color: [f32; 3],
    /// The grid's two colors, in linear RGB.
    grid_colors: [[f32; 3]; 2],
    /// Whether the checkerboard is filtered linearly, which blurs its texels together, rather than
    /// sampled at the nearest, which keeps them sharp.
    smooth: bool,
    /// How many times the checkerboard repeats across and down.
    tiles: f32,
    /// What the checkerboard's colors are multiplied by, in linear RGBA, alpha not multiplied in.
    tint: [f32; 4],
    /// The last frame's time, shown as it is: smoothing would hide uneven frames.
    frame_time: Duration,
    camera: OrthographicCameraController,
    /// How far the triangle has turned counterclockwise, in radians from 0 to a whole turn.
    triangle_turn: f32,
    /// Added once the sandbox attaches.
    triangle: Option<(Mesh<Colored>, Shader<Colored>)>,
    assets: Option<Assets>,
}

impl Sandbox {
    fn new() -> Self {
        // 3 units from the view's bottom to its top, around the grid's 2.
        let mut camera = OrthographicCameraController::new(Camera {
            projection: Projection::Orthographic {
                height: 3.0,
                near: -1.0,
                far: 1.0,
            },
            ..Camera::default()
        });
        camera.rotation = true;
        Self {
            time: Duration::ZERO,
            drift: true,
            color: [0.0; 3],
            // Blue and red.
            grid_colors: [[0.2, 0.3, 0.8], [0.8, 0.2, 0.3]],
            smooth: false,
            tiles: 2.0,
            // Pink.
            tint: [1.0, 0.8, 0.8, 1.0],
            frame_time: Duration::ZERO,
            camera,
            triangle_turn: 0.0,
            triangle: None,
            assets: None,
        }
    }
}

impl Layer for Sandbox {
    fn attach(&mut self, cx: &mut Context) {
        // A shader that doesn't compile, or a texture that doesn't load, stops the sandbox, with
        // what went wrong.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let assets = root.join("assets");
        let colored = cx.add_shader(
            "vertex color",
            root.join("shaders/vertex_color.slang"),
            Blend::Opaque,
        );
        let triangle = cx.add_mesh("triangle", &TRIANGLE, &[0, 1, 2]);
        let checkerboard = cx.add_texture("checkerboard", assets.join("checkerboard.png"));
        let logo = cx.add_texture("logo", assets.join("logo.png"));
        self.triangle = Some((
            triangle.expect("the triangle is a whole triangle, of its own vertices"),
            colored.expect("the vertex color shader compiles"),
        ));
        self.assets = Some(Assets {
            checkerboard: checkerboard.expect("the checkerboard loads"),
            logo: logo.expect("the logo loads"),
        });
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        let seconds = dt.as_secs_f32();
        self.camera.update(cx, dt);

        // 10 by 10, 2 units across, each square a little smaller than the spacing, in the grid's
        // colors in turn, as a checkerboard's.
        for row in 0..10 {
            for column in 0..10 {
                let at = Vec3::new(column as f32 - 4.5, row as f32 - 4.5, 0.0) * 0.2;
                let transform = Mat4::from_translation(at) * Mat4::from_scale(Vec3::splat(0.18));
                let color = Vec3::from(self.grid_colors[(row + column) % 2]).extend(1.0);
                let sprite = Sprite {
                    color,
                    ..Sprite::default()
                };
                cx.draw_sprite(transform, sprite);
            }
        }
        // 1.5 units across, over the grid's middle.
        let middle = Mat4::from_scale(Vec3::splat(1.5));
        if let Some(assets) = &self.assets {
            let sampler = if self.smooth {
                Sampler::LinearRepeat
            } else {
                Sampler::NearestRepeat
            };
            let checkerboard = Sprite {
                color: Vec4::from(self.tint),
                texture: assets.checkerboard,
                rect: [Vec2::ZERO, Vec2::splat(self.tiles)],
                sampler,
            };
            cx.draw_sprite(middle, checkerboard);
        }
        self.triangle_turn = (self.triangle_turn + TURN_SPEED * seconds).rem_euclid(TAU);
        if let Some((mesh, shader)) = self.triangle {
            cx.draw(mesh, shader, Mat4::from_rotation_z(self.triangle_turn), ());
        }
        if let Some(assets) = &self.assets {
            let logo = Sprite {
                texture: assets.logo,
                ..Sprite::default()
            };
            cx.draw_sprite(middle, logo);
        }

        self.frame_time = dt;
        if self.drift && !cx.input().key_down(Key::Space) {
            self.time += dt;
            let t = self.time.as_secs_f32() * 0.4;
            let wave = |phase: f32| 0.02 + 0.06 * (0.5 + 0.5 * (t + phase).sin());
            self.color = [wave(0.0), wave(2.1), wave(4.2)];
        }
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        self.camera.event(cx, event)?;
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
            let camera = self.camera.camera;
            let [x, y, _] = camera.position.to_array();
            let (turn, _, _) = camera.rotation.to_euler(EulerRot::ZYX);
            let Projection::Orthographic { height, .. } = camera.projection;
            ui.label(format!(
                "Camera at {x:.2}, {y:.2}, turned {:.0}°, {height:.2} units high",
                turn.to_degrees().rem_euclid(360.0)
            ));
            ui.label(match cx.input().cursor() {
                Some([x, y]) => format!("Cursor at {x:.0}, {y:.0}"),
                None => "Cursor outside the window".to_owned(),
            });
        });
        ui.panel("Grid", |ui| {
            ui.horizontal(|ui| {
                ui.label("Colors");
                for color in &mut self.grid_colors {
                    ui.color_edit_button_rgb(color);
                }
            });
        });
        ui.panel("Textures", |ui| {
            ui.checkbox(&mut self.smooth, "Smooth the checkerboard");
            ui.add(egui::Slider::new(&mut self.tiles, 1.0..=10.0).text("Tiles"));
            ui.horizontal(|ui| {
                ui.label("Tint");
                ui.color_edit_button_rgba_unmultiplied(&mut self.tint);
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
