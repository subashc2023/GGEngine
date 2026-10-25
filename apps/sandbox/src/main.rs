//! Development playground: test scenes and debug tooling for engine work.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;
use std::{fs, io, mem};

use bytemuck::NoUninit;
use gg_engine::egui::{self, ViewportCommand, ViewportId};
use gg_engine::math::{self, EulerRot, Quat, Vec2, Vec3, Vec4};
use gg_engine::reflect::Error;
use gg_engine::{
    Actions, App, Binding, Blend, Camera, Change, Commands, Component, Context, Entity, Event, Key,
    Layer, MeshDraw, OrthographicCameraController, PadButton, Projection, Reflect, Sampler, Source,
    Sprite, SpriteSheet, Stick, Tick, Transform, Ui, World,
};
use tracing::{error, info};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The sandbox, with the engine's Performance, Entities and Inspector panels: what `main` runs,
/// and what its recordings replay.
fn app() -> App {
    let app = App::new("Sandbox").custom_title_bar();
    let app = app.layer(Sandbox::new()).overlay(gg_engine::PerfPanel);
    app.overlay(gg_engine::Inspector::default())
}

/// A corner of a mesh in the colors of its corners, which `shaders/vertex_color.slang` reads: its
/// position in the mesh's own space, and its color in linear RGBA.
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

/// How fast the arrows move the triangle, in units a second.
const STEER_SPEED: f32 = 1.0;

// How far forward each part of the scene is, which the engine draws back to front by its z in the
// world: the map's ground, what stands on it, the swarm, the grid, at 0.2 in its scene, the
// checkerboard, the triangle, its moons, each 0.01 in front of its parent in theirs, so that
// they're between the triangle and the logo, and the logo. The camera sees from 1 behind it to 1
// in front.
const GROUND_Z: f32 = 0.0;
const STANDING_Z: f32 = 0.01;
const SWARM_Z: f32 = 0.1;
const CHECKERBOARD_Z: f32 = 0.3;
const TRIANGLE_Z: f32 = 0.4;
const LOGO_Z: f32 = 0.5;

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

/// The map, a letter a tile, its rows top first: W water, O deep water, S sand, G grass, F flowers,
/// D dirt, P paving, H planks; and on grass, R a rock, B a bush, T a tree, a tile across and two
/// tall, which stands over the tile above its own.
const MAP: [&str; 14] = [
    "OOOOOOOOOOOOOOOOOOOOOOOO",
    "OOWWWWWWWWWWWWWWWWWWWWOO",
    "OWWSSSSSSSSSSWWWWSSSSWWO",
    "OWSSGGTGGGGSSSWWSSGTGSWO",
    "OWSGGGGGFGGGSSWWSGGGGSWO",
    "OWSGTGGDDDDDDDHHDDDGFSWO",
    "OWSGGGGDPPPPGSWWSGGBGSWO",
    "OWSGFGGDPPPPGSWWSGTGGSWO",
    "OWSGGBGDGGGRGSWWSGGGGSWO",
    "OWSSGGGDGGTGSSWWSSGGSSWO",
    "OWWSSSGDGGGSSWWWWSSSSWWO",
    "OOWWSSSDSSSSWWWOOWWWWWOO",
    "OOOWWWWWWWWWWWOOOOOOOOOO",
    "OOOOOOOOOOOOOOOOOOOOOOOO",
];
const _: () = {
    let mut row = 1;
    while row < MAP.len() {
        assert!(MAP[row].len() == MAP[0].len(), "the map's rows are as long");
        row += 1;
    }
};

/// How far across and down a tile of the map is, in units: a power of two, with the map's center a
/// multiple of half of it, so that every corner of every tile is exact in `f32`, and tiles side by
/// side share their edges exactly, leaving no crack between them.
const TILE: f32 = 0.25;

/// Where the map's center is: below the swarm's field, whose sprites would hide it.
const MAP_CENTER: Vec2 = Vec2::new(0.0, -5.0);

/// The cells of `assets/tiles.png` that the map's `letter` draws: its ground's, and what stands on
/// it, with how many cells tall that is.
fn tile(letter: u8) -> ([u32; 2], Option<([u32; 2], u32)>) {
    let on_grass = |standing| ([2, 0], Some(standing));
    match letter {
        b'W' => ([0, 0], None),
        b'O' => ([2, 2], None),
        b'S' => ([1, 0], None),
        b'G' => ([2, 0], None),
        b'F' => ([1, 1], None),
        b'D' => ([2, 1], None),
        b'P' => ([0, 1], None),
        b'H' => ([3, 2], None),
        b'R' => on_grass(([0, 2], 1)),
        b'B' => on_grass(([1, 2], 1)),
        b'T' => on_grass(([3, 0], 2)),
        _ => panic!("the map has no tile {:?}", char::from(letter)),
    }
}

/// A camera, unturned, over `center`, seeing `height` units from the view's bottom to its top.
fn overlooking(center: Vec2, height: f32) -> Camera {
    Camera {
        position: center.extend(0.0),
        projection: Projection::Orthographic {
            height,
            near: -1.0,
            far: 1.0,
        },
        ..Camera::default()
    }
}

/// What the sandbox set up as it attached, which its ticks go by: the swarm's sprites, one for each
/// cell of a sheet of 4 by 4 shapes with transparent margins, row by row.
struct Scene {
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

/// One of the map's sprites, which the map's filtering applies to.
#[derive(Clone, Copy, Component)]
struct MapTile;

/// The checkerboard's sprite, which its panel's filtering, tiling and tint apply to.
#[derive(Clone, Copy, Component)]
struct Checkerboard;

/// One of the grid's squares, which its sprite's color follows.
#[derive(Clone, Copy, Component)]
struct GridCell {
    /// The grid's first color or its second, alternating as a checkerboard's.
    #[reflect(max = 1)]
    color: u32,
}

/// Turns an entity about z.
#[derive(Clone, Copy, Component)]
struct Spin {
    /// How far it turns a second, counterclockwise.
    #[reflect(degrees)]
    speed: f32,
}

/// Moves an entity, the triangle, as the arrows, the left stick and the D-pad steer it.
#[derive(Clone, Copy, Component)]
struct Steer {
    /// Where it's going, no longer than 1: a unit vector for one arrow or two, shorter for a stick
    /// pushed part of the way, and zero for none.
    heading: Vec2,
}

/// Registers the sandbox's component types, as a game does before it loads a scene of them.
fn register(world: &mut World) {
    for registered in [
        world.register::<Drifter>(),
        world.register::<MapTile>(),
        world.register::<Checkerboard>(),
        world.register::<GridCell>(),
        world.register::<Spin>(),
        world.register::<Steer>(),
    ] {
        registered.expect("the sandbox's types describe themselves");
    }
}

/// The sandbox's scenes, in `scenes/` beside its shaders and assets: `grid.ron`, the grid, 10 by
/// 10 squares 2 units across; and `moons.ron`, a prefab of a pivot with two moons, which the
/// triangle carries around it as it turns, each turning too, and a moonlet, which one of them
/// carries around in turn.
fn scene(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join(name)
}

/// The text of scene `name`, and its path; the sandbox stops without it, saying why.
fn read_scene(name: &str) -> (String, PathBuf) {
    let path = scene(name);
    let text = fs::read_to_string(&path);
    let text = text.unwrap_or_else(|e| panic!("can't read {}: {e}", path.display()));
    (text, path)
}

/// The grid's squares.
fn grid(world: &World) -> Vec<Entity> {
    let cells = world.query::<(Entity, &GridCell)>();
    cells.map(|(cell, _)| cell).collect()
}

/// Saves the grid to `scenes/grid.ron`, through a file beside it that then replaces it, so that a
/// save cut short leaves the last grid whole.
fn save_grid(world: &World) -> Result<(), Error> {
    let path = scene("grid.ron");
    let text = world.save_scene(&grid(world))?;
    let error = |e: io::Error| {
        let message = format!("can't save the grid to {}: {e}", path.display());
        Error::new("sandbox.grid_file", message).with_source(e)
    };
    let temporary = path.with_extension("ron.new");
    fs::write(&temporary, text).map_err(error)?;
    fs::rename(&temporary, &path).map_err(error)
}

/// The grid scene's text, from `scenes/grid.ron`; or, if it can't be read, logs why.
fn read_grid() -> Option<String> {
    let path = scene("grid.ron");
    match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) => {
            let path = path.display();
            error!(code = "sandbox.grid_file", %path, "Can't read the grid from {path}: {e}");
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
/// threads.
fn drift(world: &mut World, t: f64) {
    let _zone = gg_engine::zone!();
    world
        .query_mut::<(&Drifter, &mut Transform)>()
        .par_for_each(|(drifter, transform)| *transform = drifter.transform(t));
}

/// Colors the grid's squares in `colors`, in linear RGB.
fn color_grid(world: &mut World, colors: [[f32; 3]; 2]) {
    for (cell, sprite) in world.query_mut::<(&GridCell, &mut Sprite)>() {
        sprite.color = Vec3::from(colors[cell.color as usize]).extend(1.0);
    }
}

/// Turns each entity that spins as far as it does in `dt`.
fn spin(world: &mut World, dt: Duration) {
    for (spin, transform) in world.query_mut::<(&Spin, &mut Transform)>() {
        let turn = Quat::from_rotation_z(spin.speed * dt.as_secs_f32());
        transform.rotation = (turn * transform.rotation).normalize();
    }
}

/// Moves each entity that steers along its heading, as far as it goes in `dt`.
fn steer(world: &mut World, dt: Duration) {
    for (steer, transform) in world.query_mut::<(&Steer, &mut Transform)>() {
        // One that isn't going anywhere stays where it is, to a zero's sign.
        if steer.heading != Vec2::ZERO {
            let by = steer.heading * STEER_SPEED * dt.as_secs_f32();
            transform.translation += by.extend(0.0);
        }
    }
}

/// Spawns the map from the cells of `tiles`, 4 by 3 of 16 by 16 pixels, packed edge to edge, of
/// ground, and of what stands on it with transparent backgrounds: every tile's ground, then what
/// stands on the tiles, each from the top row.
fn spawn_map(world: &mut World, tiles: &SpriteSheet) {
    // The top left tile's center.
    let rows = MAP.len() as f32;
    let columns = MAP[0].len() as f32;
    let corner = MAP_CENTER + Vec2::new(1.0 - columns, rows - 1.0) * TILE / 2.0;
    let mut spawn = |row: usize, column: usize, cell, tall: u32, z: f32| {
        let at = corner + Vec2::new(column as f32, -(row as f32)) * TILE;
        // From the tile up.
        let height = TILE * tall as f32;
        let center = at + Vec2::new(0.0, (height - TILE) / 2.0);
        let transform = Transform {
            translation: center.extend(z),
            scale: Vec3::new(TILE, height, 1.0),
            ..Transform::IDENTITY
        };
        world.spawn((transform, tiles.sprite(cell, [1, tall]), MapTile));
    };
    let tiles = || {
        let rows = MAP.iter().enumerate();
        rows.flat_map(|(row, letters)| letters.bytes().enumerate().map(move |(c, l)| (row, c, l)))
    };
    for (row, column, letter) in tiles() {
        spawn(row, column, tile(letter).0, 1, GROUND_Z);
    }
    for (row, column, letter) in tiles() {
        if let Some((cell, tall)) = tile(letter).1 {
            spawn(row, column, cell, tall, STANDING_Z);
        }
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
/// red, green and blue turning, with two moons, which it carries around as it turns,
/// one of them carrying a moonlet of its own, then a logo sprite whose alpha shows them through it.
/// Below the swarm, out of its way, a map of tiles from a tile set's cells packed edge to edge,
/// which tests that none shows any of the cells beside it. They're entities, spawned as the sandbox
/// attaches but for the swarm, at its first tick, the grid loaded from a scene and the moons placed
/// from a prefab, under a pivot that the triangle carries, which the engine draws back to front by
/// their z's in the world; the world holds all of the simulation, so that a snapshot of it restores
/// it, and a recording of the commands replays it. At each tick, 64 a second whatever the frame
/// rate, the sandbox's systems move the swarm by its drifters, turn the triangle, its moons and the
/// moonlet by their spins, and move the triangle along its heading, after the commands its panels,
/// File menu and actions queued since the last: they steer the triangle and reverse its spin,
/// resize the swarm, color the grid by its cells, filter the map and set the checkerboard's
/// sprite, so that what the Inspector sets stays until a panel changes it, and load the grid
/// again. The triangle is a mesh with a vertex type and shader of the sandbox's own, and the rest
/// the engine's sprites, so the map, the swarm, the grid and the checkerboard are one instanced
/// draw, the triangle another, and the moons, the moonlet and the logo a third. They're drawn over
/// a color that drifts slowly, so that a running sandbox visibly animates. The engine's camera
/// controller steers the camera: W, A, S and D, or a gamepad's right stick, move it along its own
/// axes, Q and E, or the triggers, turn it left and right, at the same speed whatever the frame
/// rate, and the mouse wheel zooms it. The arrows, the left stick and the D-pad steer the triangle,
/// a unit a second at most, and R or the pad's South button, A on an Xbox pad, reverses its spin:
/// the sandbox's actions, whose changes it queues as commands as they come, so that its ticks move
/// the triangle for as long as an arrow is held, and reverse it once for each press, however short.
/// Shows six panels to dock, beside the engine's Performance panel, which counts the draws, and its
/// Entities and Inspector panels, which list the entities and edit the picked one's components: the
/// camera; how many sprites the swarm has; the map's filtering, smooth or sharp, and buttons that
/// look at the map or back at the swarm; the grid's colors, which can be picked; the checkerboard's
/// filtering, how many times it repeats, and its tint; and the clear color, which can be held or
/// picked. Logs the key, mouse and gamepad presses that reach it, which those on the panels don't.
/// It polls input too: holding Space pauses the drift, and a panel shows where the cursor is. The
/// title bar's File menu saves the grid to its scene, loads it again from the scene, which a
/// refusal of leaves it as it was, and quits.
struct Sandbox {
    time: Duration,
    drift: bool,
    /// The clear color, in linear RGB.
    color: [f32; 3],
    /// The grid's two colors, in linear RGB.
    grid_colors: [[f32; 3]; 2],
    settings: Settings,
    camera: OrthographicCameraController,
    actions: Actions<Act>,
    /// How many sprites the swarm has, as its panel says.
    swarm: u32,
    /// Spawned once the sandbox attaches.
    scene: Option<Scene>,
    /// What the panels, the File menu and the actions changed since the last tick.
    commands: Vec<Command>,
}

/// What the sandbox's keys and gamepads do, beside the camera controller's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// The arrows, the left stick and the D-pad: which way the triangle goes.
    Steer,
    /// R and South: turns the triangle the other way.
    Reverse,
}

/// How the map and the checkerboard are drawn, as their panels say.
#[derive(Clone, Copy, Reflect)]
struct Settings {
    /// Whether the map is filtered linearly rather than sampled at the nearest texel, as pixel art
    /// is.
    smooth_map: bool,
    /// Whether the checkerboard is filtered linearly, which blurs its texels together, rather than
    /// sampled at the nearest, which keeps them sharp.
    smooth: bool,
    /// How many times the checkerboard repeats across and down.
    tiles: f32,
    /// What the checkerboard's colors are multiplied by, in linear RGBA, alpha not multiplied in.
    tint: [f32; 4],
}

/// What the sandbox's panels, File menu and actions change of its world, queued as they change it
/// and applied at its next tick, before its systems: its commands, which a recording keeps.
#[derive(Reflect)]
enum Command {
    /// Has the triangle go this way, until the next.
    Steer(Vec2),
    /// Turns the triangle, but not its moons, the other way.
    Reverse,
    /// Spawns or despawns drifters until the swarm has this many.
    Swarm(u32),
    /// Colors the grid's squares in these, in linear RGB.
    GridColors([[f32; 3]; 2]),
    /// Draws the map and the checkerboard so.
    Settings(Settings),
    /// Loads the grid in place of the one there is from `text`, read from its scene as the menu
    /// was clicked, and colors it in `colors`, as the panel had them then.
    ReloadGrid { text: String, colors: [[f32; 3]; 2] },
}

impl Sandbox {
    fn new() -> Self {
        let mut camera = OrthographicCameraController::new(Self::start());
        camera.rotation = true;
        let mut actions = Actions::default();
        let arrows = Binding::Axis2 {
            x: [Key::ArrowLeft, Key::ArrowRight].map(Source::Key),
            y: [Key::ArrowDown, Key::ArrowUp].map(Source::Key),
        };
        let d_pad = Binding::Axis2 {
            x: [PadButton::DPadLeft, PadButton::DPadRight].map(Source::PadButton),
            y: [PadButton::DPadDown, PadButton::DPadUp].map(Source::PadButton),
        };
        let bound = [
            (Act::Steer, arrows),
            (Act::Steer, Binding::stick(Stick::Left)),
            (Act::Steer, d_pad),
            (Act::Reverse, Binding::Button(Source::Key(Key::KeyR))),
            (
                Act::Reverse,
                Binding::Button(Source::PadButton(PadButton::South)),
            ),
        ]
        .into_iter()
        .try_for_each(|(action, binding)| actions.bind(action, binding));
        bound.expect("the sandbox's bindings are each of one kind, of other sources");
        Self {
            time: Duration::ZERO,
            drift: true,
            color: [0.0; 3],
            // Blue and red.
            grid_colors: [[0.2, 0.3, 0.8], [0.8, 0.2, 0.3]],
            settings: Settings {
                smooth_map: false,
                smooth: false,
                tiles: 2.0,
                // Pink.
                tint: [1.0, 0.8, 0.8, 1.0],
            },
            camera,
            actions,
            swarm: SWARM_START,
            scene: None,
            commands: Vec::new(),
        }
    }

    /// Where the camera starts: over the origin, 3 units from the view's bottom to its top, around
    /// the grid's 2.
    fn start() -> Camera {
        overlooking(Vec2::ZERO, 3.0)
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

    /// Has the map's sprites and the checkerboard's as `settings` say, as the sandbox attaches and
    /// at the tick after the panels change them: between, the Inspector may set them otherwise.
    fn apply_settings(&self, world: &mut World, settings: Settings) {
        let sampler = if settings.smooth_map {
            Sampler::LinearClamp
        } else {
            Sampler::NearestClamp
        };
        for (_, sprite) in world.query_mut::<(&MapTile, &mut Sprite)>() {
            sprite.sampler = sampler;
        }
        for (_, checkerboard) in world.query_mut::<(&Checkerboard, &mut Sprite)>() {
            checkerboard.color = Vec4::from(settings.tint);
            checkerboard.rect = [Vec2::ZERO, Vec2::splat(settings.tiles)];
            checkerboard.sampler = if settings.smooth {
                Sampler::LinearRepeat
            } else {
                Sampler::NearestRepeat
            };
        }
    }
}

impl Layer for Sandbox {
    fn attach(&mut self, cx: &mut Context) {
        // A shader that doesn't compile, a texture that doesn't load, or a scene that's refused
        // stops the sandbox, with what went wrong.
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
        let shapes = cx.add_sprite_sheet("sheet", assets.join("sheet.png"), [32, 32]);
        let tiles = cx.add_sprite_sheet("tiles", assets.join("tiles.png"), [16, 16]);
        let shapes = shapes.expect("the shapes load");
        let mesh = triangle.expect("the triangle is a whole triangle, of its own vertices");
        let shader = colored.expect("the vertex color shader compiles");

        let world = cx.world_mut();
        register(world);
        spawn_map(world, &tiles.expect("the tiles load"));
        let (grid, path) = read_scene("grid.ron");
        let loaded = world.load_scene(&grid);
        loaded.unwrap_or_else(|error| panic!("{}:{error}", path.display()));
        // 1.5 units across, over the grid's middle.
        let middle = Vec3::splat(1.5);
        let checkerboard = Sprite {
            texture: checkerboard.expect("the checkerboard loads"),
            ..Sprite::default()
        };
        let at = Transform {
            translation: Vec3::Z * CHECKERBOARD_Z,
            scale: middle,
            ..Transform::IDENTITY
        };
        world.spawn((at, checkerboard, Checkerboard));
        let at = Transform {
            translation: Vec3::Z * TRIANGLE_Z,
            ..Transform::IDENTITY
        };
        let spin = Spin { speed: TURN_SPEED };
        let steer = Steer {
            heading: Vec2::ZERO,
        };
        let triangle = world.spawn((at, MeshDraw::new(mesh, shader, ()), spin, steer));
        let (moons, path) = read_scene("moons.ron");
        let placed = world.place_prefab(&moons, Some(triangle));
        placed.unwrap_or_else(|error| panic!("{}:{error}", path.display()));
        let logo = Sprite {
            texture: logo.expect("the logo loads"),
            ..Sprite::default()
        };
        let at = Transform {
            translation: Vec3::Z * LOGO_Z,
            scale: middle,
            ..Transform::IDENTITY
        };
        world.spawn((at, logo));
        self.scene = Some(Scene {
            shapes: std::array::from_fn(|i| shapes.sprite([i as u32 % 4, i as u32 / 4], [1, 1])),
        });
        self.apply_settings(world, self.settings);
        color_grid(world, self.grid_colors);
        // At the first tick, so that a recording holds the swarm's size, which a build's own
        // differs from another's.
        self.commands.push(Command::Swarm(self.swarm));
    }

    fn tick(&mut self, world: &mut World, tick: Tick) {
        let _zone = gg_engine::zone!();
        let t = tick.time().as_secs_f64();
        let mut commands = mem::take(&mut self.commands);
        // The swarm's last size, once: its slider dragged between two ticks queues several.
        let mut swarm = None;
        for command in commands.drain(..) {
            match command {
                Command::Steer(heading) => {
                    for steer in world.query_mut::<&mut Steer>() {
                        steer.heading = heading;
                    }
                }
                Command::Reverse => {
                    for (_, spin) in world.query_mut::<(&Steer, &mut Spin)>() {
                        spin.speed = -spin.speed;
                    }
                }
                Command::Swarm(count) => swarm = Some(count),
                Command::GridColors(colors) => color_grid(world, colors),
                Command::Settings(settings) => self.apply_settings(world, settings),
                Command::ReloadGrid { text, colors } => reload_grid(world, &text, colors),
            }
        }
        // Kept for its memory.
        self.commands = commands;
        if let Some(swarm) = swarm {
            self.resize_swarm(world, swarm, t);
        }
        drift(world, t);
        spin(world, Tick::DT);
        steer(world, Tick::DT);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        self.camera.update(cx, dt);
        if self.drift && !cx.input().key_down(Key::Space) {
            self.time += dt;
            let t = self.time.as_secs_f32() * 0.4;
            let wave = |phase: f32| 0.02 + 0.06 * (0.5 + 0.5 * math::libm::sinf(t + phase));
            self.color = [wave(0.0), wave(2.1), wave(4.2)];
        }
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        self.camera.event(cx, event)?;
        // Queued now, rather than at the next update, so that the next tick applies them.
        let Self {
            actions, commands, ..
        } = self;
        actions.event(event, |change| match change {
            Change::Axis2(Act::Steer, heading) => commands.push(Command::Steer(heading)),
            Change::Pressed(Act::Reverse) => commands.push(Command::Reverse),
            _ => {}
        });
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
            let camera = self.camera.camera;
            let [x, y, _] = camera.position.to_array();
            let (turn, _, _) = camera.rotation.to_euler(EulerRot::ZYX);
            let Projection::Orthographic { height, .. } = camera.projection;
            ui.label(format!(
                "Camera at {x:.2}, {y:.2}, turned {:.0}°, {height:.2} units high",
                math::to_degrees(turn).rem_euclid(360.0)
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
        let mut changed = false;
        ui.panel("Map", |ui| {
            changed |= ui
                .checkbox(&mut self.settings.smooth_map, "Smooth the map")
                .changed();
            ui.horizontal(|ui| {
                // 5 units high, so that the map, 6 across and 3.5 down, fits a view of most shapes.
                if ui.button("Look at the map").clicked() {
                    self.camera.camera = overlooking(MAP_CENTER, 5.0);
                }
                if ui.button("Look at the swarm").clicked() {
                    self.camera.camera = Self::start();
                }
            });
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
        ui.panel("Textures", |ui| {
            let settings = &mut self.settings;
            changed |= ui
                .checkbox(&mut settings.smooth, "Smooth the checkerboard")
                .changed();
            let tiles = egui::Slider::new(&mut settings.tiles, 1.0..=10.0).text("Tiles");
            changed |= ui.add(tiles).changed();
            ui.horizontal(|ui| {
                ui.label("Tint");
                changed |= ui
                    .color_edit_button_rgba_unmultiplied(&mut settings.tint)
                    .changed();
            });
        });
        if changed {
            self.commands.push(Command::Settings(self.settings));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A world that knows the sandbox's types, and the engine's, as the app's does.
    fn world() -> World {
        let mut world = World::default();
        world.register::<Transform>().expect("registered");
        world.register::<Sprite>().expect("registered");
        register(&mut world);
        world
    }

    /// Each scene loads and saves to its own bytes, so that a type of the sandbox's that changes
    /// under its files fails here; and the moons are placed as a prefab.
    #[test]
    fn the_scenes_save_as_they_are() {
        for name in ["grid.ron", "moons.ron"] {
            let (text, _) = read_scene(name);
            let mut world = world();
            let roots = world.load_scene(&text);
            let roots = roots.unwrap_or_else(|error| panic!("{name}:{error}"));
            assert_eq!(world.save_scene(&roots).expect("saved"), text, "{name}");
        }
        let mut world = world();
        world.load_scene(&read_scene("grid.ron").0).expect("loaded");
        assert_eq!(grid(&world).len(), 100);
        let parent = world.spawn(Transform::IDENTITY);
        let pivot = world.place_prefab(&read_scene("moons.ron").0, Some(parent));
        let pivot = pivot.expect("placed");
        let moons = world.children(pivot).expect("alive").count();
        assert_eq!(moons, 2);
        assert_eq!(world.query::<&Spin>().count(), 3);
    }

    /// A reload replaces the grid whole, a square that's another's descendant too, or, if it's
    /// refused, leaves the grid there was.
    #[test]
    fn a_reload_replaces_the_grid_whole_or_not_at_all() {
        let mut world = world();
        world.load_scene(&read_scene("grid.ron").0).expect("loaded");
        let cells = grid(&world);
        world
            .set_parent(cells[1], Some(cells[0]))
            .expect("parented");
        let nested = world.save_scene(&cells).expect("saved");
        replace_grid(&mut world, &nested).expect("replaced");
        assert_eq!(world.save_scene(&grid(&world)).expect("saved"), nested);
        let refused = replace_grid(&mut world, "(entities: {").expect_err("refused");
        assert_eq!(refused.code(), "scene.syntax");
        assert_eq!(world.save_scene(&grid(&world)).expect("saved"), nested);
    }

    /// The panels' commands change nothing until the next tick, which applies them in the order
    /// they came; and what the Inspector sets stays until a panel changes it again.
    #[test]
    fn commands_change_the_world_at_the_next_tick_in_order() {
        let mut world = world();
        world.load_scene(&read_scene("grid.ron").0).expect("loaded");
        world.spawn((Transform::IDENTITY, Sprite::default(), Checkerboard));
        let mut sandbox = Sandbox::new();
        sandbox.scene = Some(Scene {
            shapes: std::array::from_fn(|_| Sprite::default()),
        });
        let colors = [[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]];
        let commands = [
            Command::Swarm(5),
            Command::Swarm(3),
            Command::GridColors(colors),
        ];
        sandbox.commands.extend(commands);
        let drifters = |world: &World| world.query::<&Drifter>().count();
        let colored = |world: &World| {
            let cells = world.query::<(&GridCell, &Sprite)>();
            let color = |cell: &GridCell| Vec3::from(colors[cell.color as usize]).extend(1.0);
            cells
                .filter(|(cell, sprite)| sprite.color == color(cell))
                .count()
        };
        assert_eq!((drifters(&world), colored(&world)), (0, 0));
        sandbox.tick(&mut world, Tick { number: 0 });
        assert_eq!((drifters(&world), colored(&world)), (3, 100));
        // A square colored otherwise, as the Inspector colors it, stays so at the next tick.
        let square = world
            .query::<(Entity, &GridCell)>()
            .next()
            .map(|(square, _)| square);
        let sprite = world.get_mut::<Sprite>(square.expect("a square"));
        sprite.expect("a sprite").color = Vec4::ONE;
        sandbox.tick(&mut world, Tick { number: 1 });
        assert_eq!(colored(&world), 99);
        // And stays so through a reload that's refused, which leaves the grid as it was.
        let text = "(entities: {".to_owned();
        sandbox.commands.push(Command::ReloadGrid { text, colors });
        sandbox.tick(&mut world, Tick { number: 2 });
        assert_eq!(colored(&world), 99);
        // The swarm's size dragged away and back between two ticks leaves its drifters be.
        let swarm = |world: &World| {
            let drifters = world.query::<(Entity, &Drifter)>();
            let mut drifters: Vec<(u32, Entity)> = drifters.map(|(e, d)| (d.index, e)).collect();
            drifters.sort_unstable();
            drifters
        };
        let before = swarm(&world);
        assert_eq!(
            before.iter().map(|&(i, _)| i).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        sandbox
            .commands
            .extend([Command::Swarm(1), Command::Swarm(3)]);
        sandbox.tick(&mut world, Tick { number: 3 });
        assert_eq!(swarm(&world), before);
        // Shrunk, it keeps its first.
        sandbox.commands.push(Command::Swarm(2));
        sandbox.tick(&mut world, Tick { number: 4 });
        assert_eq!(swarm(&world), before[..2]);
    }

    /// The arrows, the left stick, the D-pad, R and South queue their changes as commands as they
    /// come, rather than at the next update, so that the frame's ticks apply them: the arrows, the
    /// stick and the D-pad each heading they make, and R and South a reverse for each press, not
    /// for its repeats or its release. The right stick is the camera's alone.
    #[test]
    fn its_keys_and_gamepads_queue_commands_as_they_come() {
        let mut cx = Context::seeded(0x5eed);
        let mut sandbox = Sandbox::new();
        let key = |key, pressed, repeat| Event::Key {
            key,
            pressed,
            repeat,
        };
        let pad = gg_engine::Pad(0);
        let stick = |stick, value| Event::PadStick { pad, stick, value };
        let button = |button, pressed| Event::PadButton {
            pad,
            button,
            pressed,
        };
        for event in [
            key(Key::ArrowRight, true, false),
            key(Key::ArrowUp, true, false),
            key(Key::KeyR, true, false),
            key(Key::KeyR, true, true),
            key(Key::KeyR, false, false),
            key(Key::ArrowRight, false, false),
            key(Key::ArrowUp, false, false),
            stick(Stick::Left, Vec2::new(-0.5, 0.0)),
            stick(Stick::Right, Vec2::Y),
            stick(Stick::Left, Vec2::ZERO),
            button(PadButton::DPadDown, true),
            button(PadButton::South, true),
            button(PadButton::South, false),
            button(PadButton::DPadDown, false),
        ] {
            assert_eq!(sandbox.event(&mut cx, &event), ControlFlow::Continue(()));
        }
        // Each steer's heading, and `None` for a reverse.
        let queued = sandbox.commands.iter().map(|command| match *command {
            Command::Steer(heading) => Some(heading),
            Command::Reverse => None,
            _ => panic!("the keys and pads queue steers and reverses alone"),
        });
        let diagonal = Vec2::ONE.clamp_length_max(1.0);
        let expected = [
            Some(Vec2::X),
            Some(diagonal),
            None,
            Some(Vec2::Y),
            Some(Vec2::ZERO),
            Some(Vec2::new(-0.5, 0.0)),
            Some(Vec2::ZERO),
            Some(Vec2::NEG_Y),
            None,
            Some(Vec2::ZERO),
        ];
        assert_eq!(queued.collect::<Vec<_>>(), expected);
        let camera = sandbox.camera.actions.axis2(gg_engine::CameraAction::Move);
        assert_eq!(camera, Vec2::Y);
    }

    /// The live recording, `tests/references/live.recording`, of a windowed run whose panels and
    /// Inspector were used, replays without a GPU to the hashes it recorded, as the deterministic
    /// mode needs, in a debug build as in a release one and on every platform.
    /// `GG_UPDATE_REFERENCES=1` writes its hashes again, after a change meant to change the
    /// simulation or what the hash reads; a change to a command's type that its lines no longer
    /// read as needs a new recording.
    #[test]
    fn the_live_recording_replays_to_its_hashes() {
        let path = live_recording();
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("can't read {}: {error}", path.display()));
        if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
            let rewritten = gg_engine::Replay::rewrite(app(), &text);
            fs::write(&path, rewritten.unwrap_or_else(|error| panic!("{error:#}")))
                .expect("written");
            println!("wrote {}", path.display());
            return;
        }
        assert!(text.ends_with("\nend\n"), "the recording has its end");
        let mut replay = gg_engine::Replay::new(app(), &text);
        let replay = replay.as_mut().unwrap_or_else(|error| panic!("{error:#}"));
        let mut ticks = 0;
        while let Some(tick) = replay.step().unwrap_or_else(|error| panic!("{error:#}")) {
            ticks = tick.number + 1;
        }
        let hashes = text
            .lines()
            .filter(|line| line.starts_with(|c: char| c.is_ascii_digit()));
        assert_eq!(ticks, hashes.count() as u64);
    }

    fn live_recording() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/references/live.recording")
    }

    /// A copy of the live recording whose first command that colors the grid colors one of its
    /// squares' colors otherwise goes another way at that command's tick, and the first difference
    /// between the two worlds there is the red of the first square of that color, by its ID.
    #[test]
    fn a_forced_divergence_names_its_tick_entity_and_field() {
        // Its hashes are being written again, by the test above.
        if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
            return;
        }
        let text = fs::read_to_string(live_recording()).expect("the live recording");
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let at = lines
            .iter()
            .position(|line| line.starts_with("> 0 Variant(\"GridColors\""));
        let at = at.expect("the recording colors the grid");
        // The first color's red.
        let red = lines[at].find("F32(").expect("a color") + "F32(".len();
        let end = red + lines[at][red..].find(')').expect("a float");
        let old: f32 = lines[at][red..end].parse().expect("a float");
        let new = if old > 0.5 { old - 0.25 } else { old + 0.25 };
        lines[at].replace_range(red..end, &format!("{new:?}"));
        let tick = lines[at..]
            .iter()
            .find_map(|line| line.split_once(' ')?.0.parse::<u64>().ok());
        let tick = tick.expect("the command's tick");
        let changed: String = lines.iter().map(|line| format!("{line}\n")).collect();
        let mut original = gg_engine::Replay::new(app(), &text).expect("attached");
        let mut copy = gg_engine::Replay::new(app(), &changed).expect("attached");
        let diverged = loop {
            let ran = original.step().expect("the recording replays");
            let ran = ran.expect("the copy goes another way before the end");
            if let Err(error) = copy.step() {
                break (ran, error);
            }
        };
        let (ran, error) = diverged;
        assert_eq!(error.code(), "replay.diverged", "{error}");
        assert_eq!(ran.number, tick, "{error}");
        assert!(
            error.to_string().contains(&format!("after tick {tick}")),
            "{error}"
        );
        let world = original.world();
        let squares = world
            .query::<(Entity, &GridCell)>()
            .filter(|(_, cell)| cell.color == 0);
        let first = squares.map(|(square, _)| world.id(square)).min().flatten();
        let difference = world.difference(copy.world()).expect("a difference");
        assert_eq!(Some(difference.entity), first);
        assert_eq!(difference.component.as_deref(), Some("Sprite"));
        assert_eq!(difference.path, "color.x");
        assert_eq!(difference.theirs, Some(gg_engine::reflect::Value::F32(new)));
    }

    /// A world restored from a snapshot simulates as it did from there, through a resize of the
    /// swarm, a steer of the triangle and a reverse of its spin: the sandbox keeps none of the
    /// swarm, or of where the triangle is going, which the world holds.
    #[test]
    fn a_restored_world_simulates_as_before() {
        let mut cx = Context::seeded(0x5eed);
        let mut sandbox = Sandbox {
            swarm: 1_000,
            ..Sandbox::new()
        };
        sandbox.attach(&mut cx);
        let world = cx.world_mut();
        for number in 0..64 {
            // Going up and right as the snapshot is taken.
            if number == 32 {
                let heading = Vec2::ONE.normalize();
                sandbox.commands.push(Command::Steer(heading));
            }
            sandbox.tick(world, Tick { number });
        }
        let snapshot = world.clone();
        let run = |sandbox: &mut Sandbox, world: &mut World| {
            let hashes = (64..128).map(|number| {
                match number {
                    72 => sandbox.commands.push(Command::Reverse),
                    80 => sandbox.commands.push(Command::Steer(-Vec2::X)),
                    96 => sandbox.commands.push(Command::Swarm(250)),
                    _ => {}
                }
                sandbox.tick(world, Tick { number });
                world.hash()
            });
            hashes.collect::<Vec<_>>()
        };
        let first = run(&mut sandbox, world);
        assert_eq!(world.query::<&Drifter>().count(), 250);
        world.clone_from(&snapshot);
        let again = run(&mut sandbox, world);
        let parted = first.iter().zip(&again).position(|(a, b)| a != b);
        assert_eq!(parted.map(|tick| 64 + tick), None, "the first tick to part");
    }

    /// The sandbox's simulation, run without a GPU from a seed through the commands of a script,
    /// hashes at its attach and at each tick as its reference says: the same in one run as in the
    /// next, on four threads as on one, in a debug build as in a release one, and on every
    /// platform, as the deterministic mode needs. `GG_UPDATE_REFERENCES=1` writes the reference
    /// again, after a change meant to change the simulation or what the hash reads.
    #[test]
    fn the_simulation_gives_its_reference_hashes() {
        let run = simulate();
        let again = "; the simulation goes another way each run, or on more threads than one, so \
                     no reference holds";
        let on_four = gg_engine::on_threads(4, simulate);
        first_difference(
            &run,
            &on_four,
            "a second run in this process, on 4 threads",
            again,
        );
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/references/hashes.txt");
        if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
            fs::create_dir_all(path.parent().expect("a folder")).expect("made");
            fs::write(&path, run.join("\n") + "\n").expect("written");
            println!("wrote {}", path.display());
            return;
        }
        let reference = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "can't read {}: {error}; GG_UPDATE_REFERENCES=1 writes it",
                path.display()
            )
        });
        let reference: Vec<String> = reference.lines().map(str::to_owned).collect();
        let after = "; after a change meant to change the simulation or what the hash reads, \
                     `GG_UPDATE_REFERENCES=1 cargo test -p sandbox` writes it again, but a \
                     difference on some platforms alone is their simulations going apart";
        first_difference(&run, &reference, &path.display().to_string(), after);
    }

    /// The sandbox's simulation from a seed, as lines of the world's hash, its content then its
    /// layout: one after it attaches, and one at each of 256 ticks, between which the script queues
    /// what the panels would, and sets what the Inspector would.
    fn simulate() -> Vec<String> {
        let mut cx = Context::seeded(0x5eed);
        // One swarm for debug and release builds, whose own differ.
        let mut sandbox = Sandbox {
            swarm: 1_000,
            ..Sandbox::new()
        };
        sandbox.attach(&mut cx);
        let hashed = |at: &dyn std::fmt::Display, world: &World| {
            let hash = world.hash();
            format!("{at} {:016x} {:016x}", hash.content, hash.layout)
        };
        let mut lines = vec![hashed(&"attach", cx.world())];
        for number in 0..256 {
            let world = cx.world_mut();
            match number {
                32 => sandbox.commands.push(Command::Swarm(1_500)),
                48 => sandbox.commands.push(Command::Steer(Vec2::X)),
                64 => {
                    let colors = [[0.9, 0.6, 0.1], [0.1, 0.6, 0.9]];
                    sandbox.commands.push(Command::GridColors(colors));
                }
                80 => sandbox.commands.push(Command::Steer(Vec2::ZERO)),
                96 => sandbox.commands.push(Command::Swarm(250)),
                // An infinite spin, which turns the drifter by a NaN that an invalid operation
                // makes, whose sign each CPU gives it, and the hash takes as one.
                112 => {
                    let mut drifters = world.query_mut::<&mut Drifter>();
                    let first = drifters.find(|drifter| drifter.index == 0);
                    first.expect("a drifter").spin = f32::INFINITY;
                }
                128 => sandbox.commands.push(Command::Settings(Settings {
                    smooth_map: true,
                    smooth: true,
                    tiles: 3.0,
                    tint: [0.5, 1.0, 0.5, 1.0],
                })),
                // A zero whose sign the hash keeps.
                144 => {
                    let mut triangle = world.query_mut::<(&MeshDraw, &mut Transform)>();
                    let (_, at) = triangle.next().expect("the triangle");
                    at.translation.x = -0.0;
                }
                160 => {
                    let (text, _) = read_scene("grid.ron");
                    let colors = sandbox.grid_colors;
                    sandbox.commands.push(Command::ReloadGrid { text, colors });
                }
                176 => sandbox.commands.push(Command::Reverse),
                192 => sandbox.commands.push(Command::Swarm(1_000)),
                _ => {}
            }
            sandbox.tick(world, Tick { number });
            lines.push(hashed(&number, world));
        }
        lines
    }

    /// Panics at the first of `run`'s lines that isn't `expected`'s, which `what` holds, saying
    /// which part of the hash differs there and how many lines differ, then `after`; or, if one
    /// of them ends first, at the first line the other has past its end.
    fn first_difference(run: &[String], expected: &[String], what: &str, after: &str) {
        let differ = |(line, other): &(&String, &String)| line != other;
        let lines = || run.iter().zip(expected);
        if let Some((line, other)) = lines().find(differ) {
            let parts = |line: &str| {
                line.split(' ')
                    .skip(1)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            };
            let (ours, theirs) = (parts(line), parts(other));
            let which: Vec<_> = ["content", "layout"]
                .into_iter()
                .zip(ours.iter().zip(&theirs))
                .filter(|(_, (a, b))| a != b)
                .map(|(part, _)| part)
                .collect();
            let verb = if which.len() == 1 {
                "differs"
            } else {
                "differ"
            };
            panic!(
                "the world's {} {verb} from {what}'s, first at its line `{other}`, which this run \
                 has as `{line}`; {} of {} lines differ{after}",
                which.join(" and "),
                lines().filter(differ).count(),
                run.len()
            );
        }
        if let Some(past) = expected.get(run.len()).or(run.get(expected.len())) {
            panic!(
                "{what} has {} lines and this run {}, the first past the other's end being \
                 `{past}`{after}",
                expected.len(),
                run.len()
            );
        }
    }
}
