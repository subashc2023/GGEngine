//! Development playground: test scenes and debug tooling for engine work.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use bytemuck::NoUninit;
use gg_engine::egui::{self, ViewportCommand, ViewportId};
use gg_engine::math::{EulerRot, Mat4, Quat, Vec2, Vec3, Vec4};
use gg_engine::{
    Blend, Camera, Context, Event, Key, Layer, Mesh, OrthographicCameraController, Projection,
    Sampler, Shader, Sprite, Texture, Ui,
};
use tracing::info;

fn main() -> ExitCode {
    let app = gg_engine::App::new("Sandbox").custom_title_bar();
    gg_engine::run(app.layer(Sandbox::new()).overlay(gg_engine::PerfPanel))
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

/// The most sprites the swarm has.
const SWARM_MAX: u32 = 100_000;

/// How many sprites the swarm starts with: all of them in a release build, and a tenth in a debug
/// build, whose unoptimized code would draw them all slowly.
const SWARM_START: u32 = if cfg!(debug_assertions) {
    SWARM_MAX / 10
} else {
    SWARM_MAX
};

/// The swarm's field, centered on the origin: a little wider and taller than the view at first,
/// so that its sprites wrap around out of sight.
const FIELD: Vec2 = Vec2::new(8.0, 4.5);

/// The textures from `assets/`: the checkerboard, the logo drawn over it, and the sprite sheet the
/// swarm draws from, 4 by 4 cells of shapes with transparent margins.
struct Assets {
    checkerboard: Texture,
    logo: Texture,
    sheet: Texture,
}

/// One of the swarm's sprites: what it shows and how it moves, picked once from its index.
struct Drifter {
    /// Where it is when the swarm starts, from the field's bottom left.
    start: Vec2,
    /// In units a second.
    velocity: Vec2,
    /// How far it's turned when the swarm starts, in radians.
    turn: f32,
    /// How fast it turns, in radians a second.
    spin: f32,
    /// How far across it is, in units.
    size: f32,
    /// Its cell of the sheet, in texture coordinates.
    rect: [Vec2; 2],
}

impl Drifter {
    fn new(index: u32) -> Self {
        let pick = |salt| pick(index, salt);
        let direction = Vec2::from_angle(pick(0) * TAU);
        let cell = (pick(1) * 16.0) as u32 % 16;
        let top_left = Vec2::new((cell % 4) as f32, (cell / 4) as f32) / 4.0;
        Self {
            start: Vec2::new(pick(2), pick(3)) * FIELD,
            velocity: direction * (0.05 + 0.25 * pick(4)),
            turn: pick(5) * TAU,
            spin: (pick(6) - 0.5) * 4.0,
            size: 0.05 + 0.07 * pick(7),
            rect: [top_left, top_left + Vec2::splat(0.25)],
        }
    }

    /// Where it is `t` seconds after the swarm starts. Its place and its turn are worked out in
    /// `f64` and wrapped before they're narrowed, so that the swarm moves as smoothly however long
    /// it has run.
    fn transform(&self, t: f64) -> Mat4 {
        let wrap = |start: f32, speed: f32, every: f32| {
            (f64::from(start) + f64::from(speed) * t).rem_euclid(f64::from(every)) as f32
        };
        let at = Vec2::new(
            wrap(self.start.x, self.velocity.x, FIELD.x),
            wrap(self.start.y, self.velocity.y, FIELD.y),
        ) - FIELD / 2.0;
        Mat4::from_scale_rotation_translation(
            Vec3::splat(self.size),
            Quat::from_rotation_z(wrap(self.turn, self.spin, TAU)),
            at.extend(0.0),
        )
    }
}

/// A number from 0 up to 1 that `index` and `salt` pick, the same every time: Chris Wellons's
/// lowbias32 hash, its top 24 bits as a fraction.
fn pick(index: u32, salt: u32) -> f32 {
    let mut x = index ^ salt.wrapping_mul(0x9e37_79b9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 / (1 << 24) as f32
}

/// Draws in the game's view a swarm of sprites from a sheet's cells, drifting and turning, which
/// tests how many sprites the engine draws; over it, a grid of sprites in two colors that
/// alternate; over the grid's middle, a checkerboard sprite, tiled and tinted, then a triangle in
/// red, green and blue turning, then a logo sprite whose alpha shows them through it. The triangle
/// is a mesh with a vertex type and shader of the sandbox's own, and the rest the engine's sprites,
/// so the swarm, the grid and the checkerboard are one instanced draw, and the triangle and the
/// logo one each. They're drawn over a color that drifts slowly, so that a running sandbox visibly
/// animates. The engine's camera controller steers the camera: W, A, S and D move it along its own
/// axes, Q and E turn it left and right, at the same speed whatever the frame rate, and the mouse
/// wheel zooms it. Shows five panels to dock, beside the engine's Performance panel, which counts
/// the draws: the camera; how many sprites the swarm has; the grid's colors, which can be picked;
/// the checkerboard's filtering, smooth or sharp, how many times it repeats, and its tint; and the
/// clear color, which can be held or picked. Logs the key and mouse presses that reach it, which
/// those on the panels don't. It polls input too: holding Space pauses the drift, and a panel
/// shows where the cursor is. The title bar's File menu quits.
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
    camera: OrthographicCameraController,
    /// How far the triangle has turned counterclockwise, in radians from 0 to a whole turn.
    triangle_turn: f32,
    /// Every sprite the swarm can have, of which it draws the first `swarm`.
    drifters: Vec<Drifter>,
    swarm: u32,
    /// How long the swarm has moved: its own clock, which the drift's pauses don't stop.
    swarm_time: Duration,
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
            camera,
            triangle_turn: 0.0,
            drifters: (0..SWARM_MAX).map(Drifter::new).collect(),
            swarm: SWARM_START,
            swarm_time: Duration::ZERO,
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
        let sheet = cx.add_texture("sheet", assets.join("sheet.png"));
        self.triangle = Some((
            triangle.expect("the triangle is a whole triangle, of its own vertices"),
            colored.expect("the vertex color shader compiles"),
        ));
        self.assets = Some(Assets {
            checkerboard: checkerboard.expect("the checkerboard loads"),
            logo: logo.expect("the logo loads"),
            sheet: sheet.expect("the sprite sheet loads"),
        });
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        let _zone = gg_engine::zone!();
        let seconds = dt.as_secs_f32();
        self.camera.update(cx, dt);

        // First, so that the swarm, the grid and the checkerboard are one run of sprites.
        self.swarm_time += dt;
        if let Some(assets) = &self.assets {
            let t = self.swarm_time.as_secs_f64();
            for drifter in &self.drifters[..self.swarm as usize] {
                let sprite = Sprite {
                    texture: assets.sheet,
                    rect: drifter.rect,
                    ..Sprite::default()
                };
                cx.draw_sprite(drifter.transform(t), sprite);
            }
        }

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
        ui.panel("Sprites", |ui| {
            let swarm = egui::Slider::new(&mut self.swarm, 0..=SWARM_MAX).logarithmic(true);
            ui.add(swarm.text("In the swarm"));
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
