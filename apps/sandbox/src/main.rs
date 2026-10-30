//! The engine's development app: a swarm of up to 100,000 sprites drifting on the job system's
//! threads, which measures how fast the engine simulates and draws, and a grid loaded from its scene
//! file, which development builds apply as it's saved, with the engine's Performance, Entities and
//! Inspector panels.

use std::f32::consts::TAU;
use std::mem;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Duration;

use gg_engine::Error;
use gg_engine::egui::{self, ViewportCommand, ViewportId};
use gg_engine::math::{self, EulerRot, Quat, Vec2, Vec3};
use gg_engine::{
    App, Commands, Component, Context, Entity, Event, Layer, OrthographicCameraController,
    Projection, Reflect, Sprite, Tick, Transform, Ui, Viewpoint, World,
};
use tracing::{error, info};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The sandbox, with the engine's Performance, Entities and Inspector panels: what `main` runs,
/// and what its recordings replay.
fn app() -> App {
    let app = App::new("Sandbox").root(env!("CARGO_MANIFEST_DIR"));
    let app = app.custom_title_bar();
    let app = app.layer(Sandbox::new()).overlay(gg_engine::PerfPanel);
    app.overlay(gg_engine::Inspector::default())
}

/// How far forward the swarm is, behind the grid, at 0.2 in its scene. The orthographic camera sees
/// from 1 behind it to 1 in front.
const SWARM_Z: f32 = 0.1;

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

/// A camera, unturned, over `center`, seeing `height` units from the view's bottom to its top.
fn overlooking(center: Vec2, height: f32) -> Viewpoint {
    Viewpoint {
        position: center.extend(0.0),
        projection: Projection::Orthographic {
            height,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    }
}

/// What the sandbox set up as it loaded, which its ticks go by: the swarm's sprites, one for each
/// cell of a sheet of 4 by 4 shapes with transparent margins, row by row.
struct SwarmShapes {
    shapes: [Sprite; 16],
}

/// One of the swarm's sprites: how it moves, picked once from its index.
#[derive(Clone, Copy, Component)]
struct Drifter {
    /// Which of the swarm it is, from 0: the swarm shrinks from its last.
    index: u32,
    /// Where it is when the swarm starts, from the field's bottom left.
    start: Vec2,
    /// In units a second.
    velocity: Vec2,
    /// How far it's turned when the swarm starts.
    #[reflect(degrees)]
    turn: f32,
    /// How far it turns a second.
    #[reflect(degrees)]
    spin: f32,
    /// How far across it is, in units.
    size: f32,
}

impl Drifter {
    /// It, and which of the sheet's shapes it shows, row by row.
    fn new(index: u32) -> (Self, usize) {
        let pick = |salt| pick(index, salt);
        let direction = Vec2::from_angle(pick(0) * TAU);
        let drifter = Self {
            index,
            start: Vec2::new(pick(2), pick(3)) * FIELD,
            velocity: direction * (0.05 + 0.25 * pick(4)),
            turn: pick(5) * TAU,
            spin: (pick(6) - 0.5) * 4.0,
            size: 0.05 + 0.07 * pick(7),
        };
        (drifter, (pick(1) * 16.0) as usize % 16)
    }

    /// Where it is `t` seconds after the swarm starts. Its place and its turn are worked out in
    /// `f64` and wrapped before they're narrowed, so that the swarm moves as smoothly however long
    /// it has run.
    fn transform(&self, t: f64) -> Transform {
        let wrap = |start: f32, speed: f32, every: f32| {
            (f64::from(start) + f64::from(speed) * t).rem_euclid(f64::from(every)) as f32
        };
        let at = Vec2::new(
            wrap(self.start.x, self.velocity.x, FIELD.x),
            wrap(self.start.y, self.velocity.y, FIELD.y),
        ) - FIELD / 2.0;
        Transform {
            translation: at.extend(SWARM_Z),
            rotation: Quat::from_rotation_z(wrap(self.turn, self.spin, TAU)),
            scale: Vec3::splat(self.size),
        }
    }
}

/// One of the grid's squares, which its sprite's color follows.
#[derive(Clone, Copy, Component)]
struct GridCell {
    /// The grid's first color or its second, alternating as a checkerboard's.
    #[reflect(max = 1)]
    color: u32,
}

/// Registers the sandbox's component types, as a game does before it loads a scene of them.
fn register(world: &mut World) {
    for registered in [world.register::<Drifter>(), world.register::<GridCell>()] {
        registered.expect("the sandbox's types describe themselves");
    }
}

/// The sandbox's scene `name`, in `scenes/` beside its assets: `grid.ron`, the grid, 10 by 10
/// squares 2 units across.
fn scene(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join(name)
}

/// The grid's squares.
fn grid(world: &World) -> Vec<Entity> {
    let cells = world.query::<(Entity, &GridCell)>();
    cells.map(|(cell, _)| cell).collect()
}

/// Saves the grid to `scenes/grid.ron`, as [`gg_engine::write_scene`] writes a scene, so that a
/// save cut short leaves the last grid whole.
fn save_grid(world: &World) -> Result<(), Error> {
    let text = world.save_scene(&grid(world))?;
    gg_engine::write_scene(&scene("grid.ron"), &text)
}

/// The grid scene's text, from `scenes/grid.ron`; or, if it can't be read, logs why.
fn read_grid() -> Option<String> {
    let path = scene("grid.ron");
    match gg_engine::read_scene(&path) {
        Ok(text) => Some(text),
        Err(error) => {
            error!(code = error.code(), path = %path.display(), "{error:#}");
            None
        }
    }
}

/// Loads the grid from `text`, read from `scenes/grid.ron`, in place of the one there is, and
/// colors it in `colors`. If it's refused, logs where and why, and the grid stays as it was.
fn reload_grid(world: &mut World, text: &str, colors: [[f32; 3]; 2]) {
    match replace_grid(world, text) {
        Ok(()) => color_grid(world, colors),
        Err(refused) => {
            let path = scene("grid.ron");
            let path = path.display();
            error!(code = refused.code(), %path, "{path}:{refused}; the grid stays as it was");
        }
    }
}

/// Loads the grid scene `text` in place of the grid there is; or, if it's refused, loads the grid
/// there was back, so that it stays as it was, its IDs and the Inspector's pick of one of its
/// squares too.
fn replace_grid(world: &mut World, text: &str) -> Result<(), Error> {
    let cells = grid(world);
    let kept = world
        .save_scene(&cells)
        .expect("the grid saves, since its squares refer to nothing outside it");
    for cell in cells {
        // A square that's another's descendant went with it.
        if world.contains(cell) {
            world.despawn(cell).expect("the square is alive");
        }
    }
    let loaded = world.load_scene(text);
    if loaded.is_err() {
        world
            .load_scene(&kept)
            .expect("the grid saved a moment ago loads again");
    }
    loaded.map(drop)
}

/// Moves each drifter to where it is `t` seconds after the swarm starts, on the job system's
/// threads; and resets the interpolation of each that wraps from one side of the field to the
/// other, which it puts in `wrapped`, so that it doesn't draw on its way across.
fn drift(world: &mut World, t: f64, wrapped: &mut Mutex<Vec<Entity>>) {
    let _zone = gg_engine::zone!();
    // Kept for its memory, and the lock with it, which macOS's std makes as it's first taken. The
    // jobs find them in any order, which drawing alone reads.
    let found = &*wrapped;
    world
        .query_mut::<(Entity, &Drifter, &mut Transform)>()
        .par_for_each(|(entity, drifter, transform)| {
            let to = drifter.transform(t);
            if wraps(transform, &to) {
                let found = found.lock();
                found.expect("no job panics holding it").push(entity);
            }
            *transform = to;
        });
    let wrapped = wrapped.get_mut().expect("no job panicked holding it");
    for &entity in wrapped.iter() {
        let reset = world.reset_interpolation(entity);
        reset.expect("a drifter just moved is alive");
    }
}

/// Whether a drifter moved from `from` to `to` at a tick has wrapped across the field, or been
/// moved across it otherwise: a tick moves it far less than half the field's width or height.
fn wraps(from: &Transform, to: &Transform) -> bool {
    let moved = (to.translation - from.translation).truncate().abs();
    moved.cmpgt(FIELD / 2.0).any()
}

/// Colors the grid's squares in `colors`, in linear RGB.
fn color_grid(world: &mut World, colors: [[f32; 3]; 2]) {
    for (cell, sprite) in world.query_mut::<(&GridCell, &mut Sprite)>() {
        sprite.color = Vec3::from(colors[cell.color as usize]).extend(1.0);
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

/// Draws in the game's view a swarm of sprites from a sheet's cells, drifting and turning on the job
/// system's threads, which tests how many sprites the engine simulates and draws, and over it a grid
/// of sprites in two colors that alternate, from its scene file, which a development build applies
/// as it's saved. Both are entities: the world holds all of the simulation, so that a snapshot of it
/// restores it, and a recording of the commands replays it. The engine's orthographic camera
/// controller steers the camera, turning it too, by the keys [`OrthographicCameraController`]
/// gives. Shows three panels to dock, beside the engine's Performance panel, which counts the draws,
/// and its Entities and Inspector panels, which list the entities and edit the picked one's
/// components: the camera and the cursor; how many sprites the swarm has; and the grid's colors,
/// which can be picked. Logs the key, mouse and gamepad presses that reach it, which those on the
/// panels don't. The title bar's File menu saves the grid to its scene, loads it again from the
/// scene, which a refusal of leaves it as it was, and quits.
struct Sandbox {
    /// The grid's two colors, in linear RGB.
    grid_colors: [[f32; 3]; 2],
    camera: OrthographicCameraController,
    /// How many sprites the swarm has, as its panel says.
    swarm: u32,
    /// The swarm's shapes, set up as the sandbox loads.
    scene: Option<SwarmShapes>,
    /// What the panels and the File menu changed since the last tick.
    commands: Vec<Command>,
    /// The drifters that wrapped at a tick, kept for its memory, with its lock: empty between
    /// ticks.
    wrapped: Mutex<Vec<Entity>>,
}

/// What the sandbox's panels and File menu change of its world, queued as they change it and
/// applied at its next tick, before its systems: its commands, which a recording keeps.
#[derive(Reflect)]
enum Command {
    /// Spawns or despawns drifters until the swarm has this many.
    Swarm(u32),
    /// Colors the grid's squares in these, in linear RGB.
    GridColors([[f32; 3]; 2]),
    /// Loads the grid in place of the one there is from `text`, read from its scene as the menu
    /// was clicked, and colors it in `colors`, as the panel had them then.
    ReloadGrid { text: String, colors: [[f32; 3]; 2] },
}

impl Sandbox {
    fn new() -> Self {
        let mut camera = OrthographicCameraController::new(overlooking(Vec2::ZERO, 3.0));
        camera.rotation = true;
        Self {
            // Blue and red.
            grid_colors: [[0.2, 0.3, 0.8], [0.8, 0.2, 0.3]],
            camera,
            swarm: SWARM_START,
            scene: None,
            commands: Vec::new(),
            wrapped: Mutex::new(Vec::new()),
        }
    }

    /// Spawns drifters until the swarm has `swarm`, or despawns them until it has, from its end,
    /// so that those left are the first `swarm`, each moving as its index has it, from where it is
    /// `t` seconds after the swarm starts. It finds them in the world, which holds the swarm
    /// whole, as a snapshot of it restores it.
    fn resize_swarm(&self, world: &mut World, swarm: u32, t: f64) {
        let _zone = gg_engine::zone!();
        let Some(scene) = &self.scene else {
            return;
        };
        let drifters = world.query::<(Entity, &Drifter)>();
        let mut gone: Vec<(u32, Entity)> = drifters
            .filter(|(_, drifter)| drifter.index >= swarm)
            .map(|(entity, drifter)| (drifter.index, entity))
            .collect();
        // The last first.
        gone.sort_unstable_by(|a, b| b.cmp(a));
        for (_, entity) in gone {
            world
                .despawn(entity)
                .expect("the swarm's drifters are alive");
        }
        let kept = world.query::<&Drifter>().count() as u32;
        for index in kept..swarm {
            let (drifter, shape) = Drifter::new(index);
            world.spawn((drifter.transform(t), scene.shapes[shape], drifter));
        }
    }
}

impl Layer for Sandbox {
    fn load(&mut self, cx: &mut Context) {
        // A texture that doesn't load stops the sandbox, with what went wrong.
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let shapes = cx.add_sprite_sheet("sheet", assets.join("sheet.png"), [32, 32]);
        let shapes = shapes.expect("the shapes load");
        self.scene = Some(SwarmShapes {
            shapes: std::array::from_fn(|i| shapes.sprite([i as u32 % 4, i as u32 / 4], [1, 1])),
        });
        register(cx.world_mut());
    }

    fn attach(&mut self, cx: &mut Context) {
        // Watched as the sandbox runs, in development builds, which apply its saves; one that's
        // refused stops the sandbox, with what went wrong.
        let loaded = cx.load_scene_file(scene("grid.ron"));
        loaded.unwrap_or_else(|error| panic!("{error:#}"));
        color_grid(cx.world_mut(), self.grid_colors);
        // At the first tick, so that a recording holds the swarm's size, which a build's own
        // differs from another's.
        self.commands.push(Command::Swarm(self.swarm));
    }

    /// Takes the panels' values from the world, which a snapshot brought back as they had left it.
    fn restored(&mut self, cx: &mut Context) {
        let world = cx.world();
        self.swarm = world.query::<&Drifter>().count() as u32;
        // Each the color most of its squares have, which a square set apart in the Inspector
        // doesn't change.
        let mut counts: [Vec<([f32; 3], u32)>; 2] = Default::default();
        for (cell, sprite) in world.query::<(&GridCell, &Sprite)>() {
            let seen = sprite.color.truncate().to_array();
            let counted = &mut counts[cell.color as usize];
            match counted.iter_mut().find(|(color, _)| *color == seen) {
                Some((_, count)) => *count += 1,
                None => counted.push((seen, 1)),
            }
        }
        for (color, counted) in self.grid_colors.iter_mut().zip(&counts) {
            if let Some(&(most, _)) = counted.iter().max_by_key(|&&(_, count)| count) {
                *color = most;
            }
        }
    }

    fn tick(&mut self, world: &mut World, tick: Tick) {
        let _zone = gg_engine::zone!();
        let t = tick.time().as_secs_f64();
        let mut commands = mem::take(&mut self.commands);
        // The swarm's last size, once: its slider dragged between two ticks queues several.
        let mut swarm = None;
        for command in commands.drain(..) {
            match command {
                Command::Swarm(count) => swarm = Some(count),
                Command::GridColors(colors) => color_grid(world, colors),
                Command::ReloadGrid { text, colors } => reload_grid(world, &text, colors),
            }
        }
        // Kept for its memory.
        self.commands = commands;
        if let Some(swarm) = swarm {
            self.resize_swarm(world, swarm, t);
        }
        drift(world, t, &mut self.wrapped);
        let wrapped = self.wrapped.get_mut();
        wrapped.expect("no job panicked holding it").clear();
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        self.camera.update(cx, dt);
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
            Event::PadButton {
                pad,
                button,
                pressed: true,
            } => info!(pad = pad.0, ?button, "Gamepad button pressed"),
            _ => {}
        }
        ControlFlow::Continue(())
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        let (mut save, mut reload) = (false, false);
        ui.menu("File", |ui| {
            save = ui.button("Save the grid").clicked();
            reload = ui.button("Reload the grid").clicked();
            if ui.button("Quit").clicked() {
                ui.ctx()
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
        });
        // Once the menu has closed.
        if save && let Err(error) = save_grid(cx.world()) {
            error!(code = error.code(), "{error:#}");
        }
        // Read now, so that the tick goes by its commands alone; and colored as the panel says,
        // rather than as it was saved.
        if reload && let Some(text) = read_grid() {
            let colors = self.grid_colors;
            self.commands.push(Command::ReloadGrid { text, colors });
        }
        ui.panel("Stats", |ui| {
            let camera = self.camera.viewpoint;
            let [x, y, z] = camera.position.to_array();
            let (turn, _, _) = camera.rotation.to_euler(EulerRot::ZYX);
            let turn = math::to_degrees(turn).rem_euclid(360.0);
            let height = match camera.projection {
                Projection::Orthographic { height, .. } => height,
                Projection::Perspective { .. } => f32::NAN,
            };
            ui.label(format!(
                "Camera at {x:.2}, {y:.2}, {z:.2}, turned {turn:.0}°, {height:.2} units high"
            ));
            ui.label(match cx.input().cursor() {
                Some([x, y]) => format!("Cursor at {x:.0}, {y:.0}"),
                None => "Cursor outside the window".to_owned(),
            });
        });
        ui.panel("Sprites", |ui| {
            let swarm = egui::Slider::new(&mut self.swarm, 0..=SWARM_MAX).logarithmic(true);
            if ui.add(swarm.text("In the swarm")).changed() {
                self.commands.push(Command::Swarm(self.swarm));
            }
        });
        // By the colors rather than the buttons' changes: egui gives back the colors its picker
        // makes of them, as it first draws them too, which the world takes, as the grid's scene
        // saves them.
        let colors = self.grid_colors;
        ui.panel("Grid", |ui| {
            ui.horizontal(|ui| {
                ui.label("Colors");
                for color in &mut self.grid_colors {
                    ui.color_edit_button_rgb(color);
                }
            });
        });
        if self.grid_colors != colors {
            self.commands.push(Command::GridColors(self.grid_colors));
        }
    }
}

#[cfg(test)]
mod tests;
