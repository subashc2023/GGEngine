//! rts, a 3D real-time strategy skeleton made with GGEngine.
//!
//! A camera looks down at the ground. WASD or the arrows pan it and the wheel zooms. A left click
//! selects the unit under the cursor, a drag selects those in its box, and a click on empty ground
//! clears the selection. A double click on a unit selects all the units on the screen. A right
//! click on the ground orders the selected units there; with Shift held it adds the place to the
//! end of the units' [`Route`], which they walk in order.
//!
//! Each unit has [`Health`], shown as a small bar above it: green while it's healthy, red as it
//! runs low. Red [`Enemy`] units stand at the far side. A selected unit with an [`Attack`] hits the
//! nearest enemy in its range each time its cooldown ends, and whatever has no health left is
//! removed.
//!
//! The mouse becomes [`Command`]s that name units by entity, which recordings keep by ID. Ticks
//! apply them, so a run replays and a restart from a snapshot loses nothing.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::f32::consts::FRAC_PI_2;
use std::ops::ControlFlow;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use gg_engine::math::{DVec3, libm};
use gg_engine::prelude::*;
use gg_engine::{CANVAS_HEIGHT, Cast, Circle};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's debug overlays: what `main` runs, and what its recordings replay.
fn app() -> App {
    let app = App::new("rts")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
    app.layer(Game::new()).debug_overlays()
}

/// The ground's half side, which units and the camera's focus stay within.
const BOUND: f32 = 28.0;
/// The camera's nearest and farthest distance from its focus.
const ZOOM: [f32; 2] = [8.0, 45.0];
/// How far the camera's focus pans a second at distance 24.
const PAN_SPEED: f32 = 14.0;
/// How far a wheel line moves the camera.
const ZOOM_STEP: f32 = 2.0;
/// Space between units ordered to one place.
const FORMATION: f32 = 2.4;
/// The most places a route holds; a further one is dropped.
const ROUTE_MAX: usize = 8;
/// A drag shorter than this many pixels is a click.
const CLICK_PIXELS: f64 = 5.0;
/// Two clicks on units within this long, and `CLICK_PIXELS` of each other, are a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// The ticks a unit may make little progress before it stops.
const STALL: u32 = 32;
/// The same, within its radius of its target.
const NEAR_STALL: u32 = 4;
/// A unit's box, which a click's ray tests: half its width, then its height.
const UNIT_BOX: [f32; 2] = [0.4, 1.0];
/// A health bar: its width in world units, how far above the ground it floats, and its thickness
/// in view pixels.
const BAR: [f32; 3] = [1.2, 1.5, 5.0];

/// The camera's rig: its focus on the ground and how far it hovers, which each tick turns into the
/// camera's transform.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct RtsCamera {
    /// The ground point it looks at, as x and z.
    focus: Vec2,
    #[reflect(min = 5.0, default = 24.0)]
    distance: f32,
    /// Radians below the horizon it looks.
    #[reflect(min = 0.2, max = 1.5, default = 0.95)]
    pitch: f32,
    /// As the last pan set it: right and up on the screen, of length at most 1.
    pan: Vec2,
}

/// A unit of the player's side: it walks to `target` while `moving`, in a straight line.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Unit {
    #[reflect(min = 0.0, default = 6.0)]
    speed: f32,
    /// How much room it takes on the ground.
    #[reflect(min = 0.0, default = 0.5)]
    radius: f32,
    selected: bool,
    moving: bool,
    /// As x and z.
    target: Vec2,
    /// The ticks in a row it has made little progress toward `target`.
    stalled: u32,
}

/// A unit's hit points, shown as a bar above it.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Health {
    #[reflect(min = 0.0, default = 100.0)]
    current: f32,
    #[reflect(min = 1.0, default = 100.0)]
    max: f32,
}

/// The places a unit walks to after `Unit.target`, as x and z, in order.
#[derive(Clone, Debug, Default, PartialEq, Component)]
struct Route {
    points: Vec<Vec2>,
}

/// How a unit of the player's side fights: it attacks only while selected.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Attack {
    /// How far away it can hit an enemy, on the ground.
    #[reflect(min = 0.0, default = 6.0)]
    range: f32,
    /// The health each hit takes.
    #[reflect(min = 0.0, default = 10.0)]
    damage: f32,
    /// The ticks between its hits.
    #[reflect(default = 32)]
    cooldown: u32,
    /// The ticks until it may hit again.
    wait: u32,
}

/// An enemy of the player's units: it stands where it is and is hit by them.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Enemy;

/// Something units walk round: a disc on the ground.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Obstacle {
    #[reflect(min = 0.0, default = 1.0)]
    radius: f32,
}

/// What the keys and mouse do, applied at the next tick. Recordings keep them.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Pans the camera along `dir` until the next.
    Pan { dir: Vec2 },
    /// Moves the camera in by `lines` of wheel, or out by negative ones.
    Zoom { lines: f32 },
    /// Selects `unit` alone, or nothing for `None`.
    Select { unit: Option<Entity> },
    /// Selects `units` alone.
    SelectMany { units: Vec<Entity> },
    /// Selects the units on the ground inside the quad, as x and z, its corners in order round.
    SelectBox { corners: [Vec2; 4] },
    /// Sends the selected units to `to`, as x and z, spread round it: at once, or with `queue`,
    /// after the places they already walk to.
    Order { to: Vec2, queue: bool },
}

/// What the keys are bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Pan,
}

/// The game's layer.
struct Game {
    actions: Actions<Act>,
    commands: Vec<Command>,
    /// Where the left button went down, in window pixels, while it's down.
    drag: Option<[f64; 2]>,
    /// When and where the last click on a unit let go, if no other click has since.
    last_click: Option<(Instant, [f64; 2])>,
    /// Memory ticks and frames reuse.
    scratch: Scratch,
}

/// Lists a tick or a frame fills afresh, kept to keep their room.
#[derive(Default)]
struct Scratch {
    units: Vec<(Entity, Vec2)>,
    obstacles: Vec<(Vec2, f32)>,
    rings: Vec<Vec3>,
    /// Where each health bar floats, and how full it is, from 0 to 1.
    bars: Vec<(Vec3, f32)>,
    foes: Vec<(Entity, Vec2)>,
    /// The enemies hit this tick and the health each loses.
    hits: Vec<(Entity, f32)>,
    dead: Vec<Entity>,
}

impl Game {
    fn new() -> Self {
        let mut actions = Actions::default();
        let pan = [Binding::WASD, Binding::ARROWS];
        let bound = actions.bind_all(pan.map(|binding| (Act::Pan, binding)));
        bound.expect("the bindings are pairs of axes");
        // What `gg actions` says each control does.
        let pan = format!(
            "Pans `RtsCamera.focus`, the point of the ground (y = 0) the camera looks at: [x, y], \
             right and up the screen, which are +x and -z. 1 is {PAN_SPEED} units a second times \
             `RtsCamera.distance` / 24. The focus stays within {BOUND} units of the origin"
        );
        actions.describe(Act::Pan, pan);
        let zoom = format!(
            "Moves the camera {ZOOM_STEP} units closer to its focus a line scrolled up, positive \
             `y`, and away a line down, keeping `RtsCamera.distance` within {} to {}",
            ZOOM[0], ZOOM[1]
        );
        actions.describe_control(Control::Scroll, zoom);
        let select = format!(
            "Pressed off the UI, it starts a selection at the cursor, and letting go ends it. \
             A drag under {CLICK_PIXELS} pixels is a click that selects the unit under the \
             cursor alone, or clears the selection over empty ground. Two clicks on a unit \
             within {} ms select every unit the view shows. A longer drag selects the units \
             on the ground inside its box. `Unit.selected` says which are",
            DOUBLE_CLICK.as_millis()
        );
        let left = Control::Source(Source::Button(MouseButton::Left));
        actions.describe_control(left, select);
        let order = format!(
            "Pressed off the UI, it orders the selected units to the ground point under the \
             cursor, in a grid {FORMATION} units apart around it and within {BOUND} units of \
             the origin, setting each one's `Unit.target` and `Unit.moving` and emptying its \
             `Route.points`. With Shift held, a unit still walking instead adds the place to \
             `Route.points`, up to {ROUTE_MAX}, and takes each as it arrives. A unit that \
             gains little for {STALL} ticks in a row, or {NEAR_STALL} within its radius of its \
             target, stops short"
        );
        let right = Control::Source(Source::Button(MouseButton::Right));
        actions.describe_control(right, order);
        let cursor = "Where the cursor is picks what a click selects or the ground an order goes \
                      to, and a drag's box follows it";
        actions.describe_control(Control::Cursor, cursor);
        Self {
            actions,
            commands: Vec::new(),
            drag: None,
            last_click: None,
            scratch: Scratch::default(),
        }
    }

    /// Turns a left drag from `start` to `end`, in window pixels, into a selection.
    fn select(&mut self, cx: &Context, start: [f64; 2], end: [f64; 2]) {
        let world = cx.world();
        let moved = libm::hypot(end[0] - start[0], end[1] - start[1]);
        if moved < CLICK_PIXELS {
            let unit = cx.ray(end).and_then(|ray| unit_under(world, ray));
            let now = Instant::now();
            let double = unit.is_some()
                && (self.last_click.take()).is_some_and(|(then, at)| {
                    now.duration_since(then) < DOUBLE_CLICK
                        && libm::hypot(end[0] - at[0], end[1] - at[1]) < CLICK_PIXELS
                });
            if double {
                let units = units_on_screen(cx);
                self.commands.push(Command::SelectMany { units });
            } else {
                self.last_click = unit.map(|_| (now, end));
                self.commands.push(Command::Select { unit });
            }
            return;
        }
        let (x, y) = ([start[0], end[0]], [start[1], end[1]]);
        let ordered = [
            [x[0].min(x[1]), y[0].min(y[1])],
            [x[0].max(x[1]), y[0].max(y[1])],
        ];
        let at = [
            [ordered[0][0], ordered[0][1]],
            [ordered[1][0], ordered[0][1]],
            [ordered[1][0], ordered[1][1]],
            [ordered[0][0], ordered[1][1]],
        ];
        let mut corners = [Vec2::ZERO; 4];
        for (corner, pixel) in corners.iter_mut().zip(at) {
            let Some(hit) = cx.ray(pixel).and_then(|ray| ground(&ray)) else {
                return;
            };
            *corner = hit;
        }
        self.commands.push(Command::SelectBox { corners });
    }
}

impl Layer for Game {
    /// Registers the components the start scene names.
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        let world = cx.world_mut();
        world.register::<RtsCamera>()?;
        world.register::<Unit>()?;
        world.register::<Health>()?;
        world.register::<Attack>()?;
        world.register::<Route>()?;
        world.register::<Enemy>()?;
        world.register::<Obstacle>()
    }

    fn attach(&mut self, cx: &mut Context) {
        // Watched as the game runs, in development builds, which apply its saves. One that doesn't
        // load ends the run with its error.
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
        }
    }

    /// Lets go of what was held as the world was taken, as when `gg run` restarts the game.
    fn restored(&mut self, _: &mut Context) {
        self.commands.push(Command::Pan { dir: Vec2::ZERO });
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for command in self.commands.drain(..) {
            apply(world, command);
        }
        steer_camera(world);
        walk_units(world, &mut self.scratch);
        advance_routes(world);
        fight(world, &mut self.scratch);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    /// What `gg actions` lists, and `gg input` presses by name.
    fn actions(&self) -> Controls {
        self.actions.controls()
    }

    fn update(&mut self, cx: &mut Context, _: std::time::Duration) {
        let rings = &mut self.scratch.rings;
        rings.clear();
        for (unit, at) in cx.world().query::<(&Unit, &Transform)>() {
            if unit.selected {
                rings.push(at.translation.as_vec3());
            }
        }
        let lie = Quat::from_rotation_x(-FRAC_PI_2);
        let ring = Circle {
            color: Vec4::new(1.0, 0.85, 0.1, 1.0),
            thickness: 0.12,
            fade: 0.0,
        };
        for &at in &*rings {
            let at = at + Vec3::Y * 0.05;
            cx.draw_circle(
                Mat4::from_scale_rotation_translation(Vec3::splat(1.5), lie, at),
                ring,
            );
        }
        self.draw_bars(cx);
        self.draw_drag(cx);
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, so the next tick applies them.
        let commands = &mut self.commands;
        self.actions.event(event, |change| {
            if let Change::Axis2(Act::Pan, dir) = change {
                commands.push(Command::Pan { dir });
            }
        });
        match *event {
            Event::Scroll(scroll) => {
                let scale = cx.window().map_or(1.0, |window| window.scale());
                self.commands.push(Command::Zoom {
                    lines: scroll.lines(scale),
                });
            }
            Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            } if !cx.pointer_over_ui() => self.drag = cx.input().cursor(),
            Event::MouseButton {
                button: MouseButton::Left,
                pressed: false,
            } => {
                if let (Some(start), Some(end)) = (self.drag.take(), cx.input().cursor()) {
                    self.select(cx, start, end);
                }
            }
            Event::MouseButton {
                button: MouseButton::Right,
                pressed: true,
            } if !cx.pointer_over_ui() => {
                let hit = cx.input().cursor().and_then(|at| cx.ray(at));
                if let Some(to) = hit.and_then(|ray| ground(&ray)) {
                    let input = cx.input();
                    let queue = input.key_down(Key::ShiftLeft) || input.key_down(Key::ShiftRight);
                    self.commands.push(Command::Order { to, queue });
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

impl Game {
    /// Draws a bar above each unit with [`Health`]: a dark back and a front as long as it's full.
    fn draw_bars(&mut self, cx: &mut Context) {
        let bars = &mut self.scratch.bars;
        bars.clear();
        for (health, at) in cx.world().query::<(&Health, &Transform)>() {
            let full = (health.current / health.max).clamp(0.0, 1.0);
            bars.push((at.translation.as_vec3() + Vec3::Y * BAR[1], full));
        }
        let half = Vec3::X * (BAR[0] / 2.0);
        let back = Vec4::new(0.05, 0.05, 0.05, 1.0);
        for &(at, full) in &*bars {
            cx.draw_line(at - half, at + half, back, BAR[2] + 2.0);
            if full > 0.0 {
                let color = Vec4::new(1.0 - full, full, 0.1, 1.0);
                cx.draw_line(at - half, at - half + half * 2.0 * full, color, BAR[2]);
            }
        }
    }

    /// Draws the box a drag has swept, on the canvas, as a faint fill with a bright edge.
    fn draw_drag(&self, cx: &mut Context) {
        let (Some(start), Some(end), Some(view)) = (self.drag, cx.input().cursor(), cx.view())
        else {
            return;
        };
        if libm::hypot(end[0] - start[0], end[1] - start[1]) < CLICK_PIXELS {
            return;
        }
        // From the canvas's top left, +Y up, in units of the view's height.
        let unit = CANVAS_HEIGHT / view.size[1] as f32;
        let to_canvas = |at: [f64; 2]| {
            let from_view = |at: f64, origin: u32| (at - f64::from(origin)) as f32 * unit;
            Vec2::new(
                from_view(at[0], view.origin[0]),
                -from_view(at[1], view.origin[1]),
            )
        };
        let (a, b) = (to_canvas(start), to_canvas(end));
        let (low, high) = (a.min(b), a.max(b));
        let mut canvas = cx.canvas([Align::Start, Align::Start]);
        let mut rect = |from: Vec2, to: Vec2, color: Vec4| {
            let size = (to - from).extend(1.0);
            let at = Mat4::from_scale_rotation_translation(
                size,
                Quat::IDENTITY,
                ((from + to) / 2.0).extend(0.0),
            );
            canvas.draw_sprite(
                at,
                Sprite {
                    color,
                    ..Sprite::default()
                },
            );
        };
        rect(low, high, Vec4::new(0.3, 0.8, 1.0, 0.15));
        let edge = Vec4::new(0.5, 0.9, 1.0, 1.0);
        let w = 2.0 * unit;
        rect(Vec2::new(low.x, high.y - w), high, edge);
        rect(low, Vec2::new(high.x, low.y + w), edge);
        rect(low, Vec2::new(low.x + w, high.y), edge);
        rect(Vec2::new(high.x - w, low.y), high, edge);
    }
}

/// Where the ray meets the ground, as x and z, or `None` if it looks up or along it.
fn ground(ray: &Cast) -> Option<Vec2> {
    let hit = ray.meets_plane(DVec3::ZERO, Vec3::Y)?;
    Some(Vec2::new(hit.x as f32, hit.z as f32))
}

/// How far along the ray it meets the box of half sides `half` and height `high`, whose base's
/// middle is `base`, if it does.
fn hit_box(ray: (Vec3, Vec3), base: Vec3, half: f32, high: f32) -> Option<f32> {
    let (from, toward) = ray;
    let low = base + Vec3::new(-half, 0.0, -half);
    let top = base + Vec3::new(half, high, half);
    let (mut near, mut far) = (0.0_f32, f32::INFINITY);
    for axis in 0..3 {
        let inverse = 1.0 / toward[axis];
        let (a, b) = (
            (low[axis] - from[axis]) * inverse,
            (top[axis] - from[axis]) * inverse,
        );
        near = near.max(a.min(b));
        far = far.min(a.max(b));
    }
    (near <= far).then_some(near)
}

/// The nearest unit the ray goes through.
fn unit_under(world: &World, ray: Cast) -> Option<Entity> {
    let ray = (ray.from.as_vec3(), ray.toward);
    let [half, high] = UNIT_BOX;
    let mut best: Option<(f32, Entity)> = None;
    for (entity, _, at) in world.query::<(Entity, &Unit, &Transform)>() {
        let hit = hit_box(ray, at.translation.as_vec3(), half, high);
        if let Some(hit) = hit.filter(|&hit| best.is_none_or(|(nearest, _)| hit < nearest)) {
            best = Some((hit, entity));
        }
    }
    best.map(|(_, entity)| entity)
}

/// The units whose middle the view shows.
fn units_on_screen(cx: &Context) -> Vec<Entity> {
    let Some(view) = cx.view() else {
        return Vec::new();
    };
    let [left, top] = view.origin.map(f64::from);
    let [width, height] = view.size.map(f64::from);
    let mut seen = Vec::new();
    for (entity, _, at) in cx.world().query::<(Entity, &Unit, &Transform)>() {
        let middle = at.translation + DVec3::Y * f64::from(UNIT_BOX[1] / 2.0);
        if let Some([x, y]) = cx.pixel(middle)
            && (left..left + width).contains(&x)
            && (top..top + height).contains(&y)
        {
            seen.push(entity);
        }
    }
    seen
}

/// Whether `point` is within `slack` of the convex quad `corners`, in either winding.
fn in_quad(corners: &[Vec2; 4], point: Vec2, slack: f32) -> bool {
    let mut area = 0.0;
    for i in 0..4 {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        area += a.perp_dot(b);
    }
    let side = area.signum();
    (0..4).all(|i| {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        let edge = b - a;
        side * edge.perp_dot(point - a) >= -slack * edge.length()
    })
}

/// The ground position of a transform, as x and z.
fn on_ground(at: &Transform) -> Vec2 {
    Vec2::new(at.translation.x as f32, at.translation.z as f32)
}

/// Applies one command to the world.
fn apply(world: &mut World, command: Command) {
    match command {
        Command::Pan { dir } => {
            for rig in world.query_mut::<&mut RtsCamera>() {
                rig.pan = dir;
            }
        }
        Command::Zoom { lines } => {
            for rig in world.query_mut::<&mut RtsCamera>() {
                rig.distance = (rig.distance - lines * ZOOM_STEP).clamp(ZOOM[0], ZOOM[1]);
            }
        }
        Command::Select { unit } => {
            for (entity, unit_of) in world.query_mut::<(Entity, &mut Unit)>() {
                unit_of.selected = Some(entity) == unit;
            }
        }
        Command::SelectMany { units } => {
            for (entity, unit) in world.query_mut::<(Entity, &mut Unit)>() {
                unit.selected = units.contains(&entity);
            }
        }
        Command::SelectBox { corners } => {
            for (unit, at) in world.query_mut::<(&mut Unit, &Transform)>() {
                unit.selected = in_quad(&corners, on_ground(at), unit.radius);
            }
        }
        Command::Order { to, queue } => {
            let chosen = world.query::<&Unit>().filter(|unit| unit.selected).count();
            let columns = (chosen as f32).sqrt().ceil().max(1.0) as usize;
            let rows = chosen.div_ceil(columns);
            let middle = Vec2::new(columns as f32 - 1.0, rows as f32 - 1.0) / 2.0;
            // A unit that's walking keeps its route's places, then this one; the others start.
            let mut later = Vec::new();
            let mut started = Vec::new();
            for (nth, (entity, unit)) in world
                .query_mut::<(Entity, &mut Unit)>()
                .filter(|(_, unit)| unit.selected)
                .enumerate()
            {
                let cell = Vec2::new((nth % columns) as f32, (nth / columns) as f32);
                let slot = to + (cell - middle) * FORMATION;
                let slot = slot.clamp(Vec2::splat(-BOUND), Vec2::splat(BOUND));
                if queue && unit.moving {
                    later.push((entity, slot));
                } else {
                    unit.target = slot;
                    unit.moving = true;
                    unit.stalled = 0;
                    started.push(entity);
                }
            }
            for entity in started {
                if let Some(route) = world.get_mut::<Route>(entity) {
                    route.points.clear();
                }
            }
            for (entity, slot) in later {
                if let Some(route) = world.get_mut::<Route>(entity) {
                    if route.points.len() < ROUTE_MAX {
                        route.points.push(slot);
                    }
                } else {
                    let points = vec![slot];
                    world.insert(entity, Route { points }).expect("alive");
                }
            }
        }
    }
}

/// Pans and zooms the camera's transform to its rig.
fn steer_camera(world: &mut World) {
    let dt = Tick::DT.as_secs_f32();
    for (rig, at) in world.query_mut::<(&mut RtsCamera, &mut Transform)>() {
        let pan = Vec2::new(rig.pan.x, -rig.pan.y) * PAN_SPEED * dt * rig.distance / 24.0;
        rig.focus = (rig.focus + pan).clamp(Vec2::splat(-BOUND), Vec2::splat(BOUND));
        let back = Vec3::new(0.0, libm::sinf(rig.pitch), libm::cosf(rig.pitch)) * rig.distance;
        at.translation =
            DVec3::new(f64::from(rig.focus.x), 0.0, f64::from(rig.focus.y)) + back.as_dvec3();
        at.rotation = Quat::from_rotation_x(-rig.pitch);
    }
}

/// Whether a unit that has made little progress for `stalled` ticks stops short of its target,
/// `left` away: soon if it's within `radius` of it, else after a while. Its slot may be taken, or
/// inside an obstacle.
fn gave_up(stalled: u32, left: f32, radius: f32) -> bool {
    stalled >= if left <= radius { NEAR_STALL } else { STALL }
}

/// Walks the moving units toward their targets, keeps them apart and out of obstacles.
fn walk_units(world: &mut World, scratch: &mut Scratch) {
    let step = Tick::DT.as_secs_f32();
    scratch.obstacles.clear();
    for (obstacle, at) in world.query::<(&Obstacle, &Transform)>() {
        scratch.obstacles.push((on_ground(at), obstacle.radius));
    }
    scratch.units.clear();
    for (entity, _, at) in world.query::<(Entity, &Unit, &Transform)>() {
        scratch.units.push((entity, on_ground(at)));
    }
    // The others' positions are as the tick began.
    for (entity, unit, at) in world.query_mut::<(Entity, &mut Unit, &mut Transform)>() {
        let start = on_ground(at);
        let mut here = start;
        if unit.moving {
            let left = unit.target - here;
            let by = unit.speed * step;
            if left.length() <= by {
                here = unit.target;
                unit.moving = false;
            } else {
                here += left.normalize() * by;
            }
        }
        let room = unit.radius * 2.1;
        for &(other, there) in &scratch.units {
            let away = here - there;
            let near = away.length();
            if other != entity && near < room {
                let direction = if near > 1e-4 { away / near } else { Vec2::X };
                here += direction * (room - near) * 0.5;
            }
        }
        for &(there, radius) in &scratch.obstacles {
            let away = here - there;
            let near = away.length();
            let room = radius + unit.radius;
            if near < room {
                let direction = if near > 1e-4 { away / near } else { Vec2::X };
                here = there + direction * room;
            }
        }
        here = here.clamp(Vec2::splat(-BOUND), Vec2::splat(BOUND));
        if unit.moving {
            let gained = (unit.target - start).length() - (unit.target - here).length();
            unit.stalled = if gained < step * unit.speed * 0.25 {
                unit.stalled + 1
            } else {
                0
            };
            if gave_up(unit.stalled, (unit.target - here).length(), unit.radius) {
                unit.moving = false;
            }
        }
        at.translation.x = f64::from(here.x);
        at.translation.z = f64::from(here.y);
    }
}

/// Sends each unit that has stopped to the first place of its route.
fn advance_routes(world: &mut World) {
    for (unit, route) in world.query_mut::<(&mut Unit, &mut Route)>() {
        if !unit.moving && !route.points.is_empty() {
            unit.target = route.points.remove(0);
            unit.moving = true;
            unit.stalled = 0;
        }
    }
}

/// Has each selected unit hit the nearest enemy in its range once its cooldown ends, then removes
/// what has no health left.
fn fight(world: &mut World, scratch: &mut Scratch) {
    scratch.foes.clear();
    for (entity, _, at) in world.query::<(Entity, &Enemy, &Transform)>() {
        scratch.foes.push((entity, on_ground(at)));
    }
    scratch.hits.clear();
    for (unit, attack, at) in world.query_mut::<(&Unit, &mut Attack, &Transform)>() {
        attack.wait = attack.wait.saturating_sub(1);
        if !unit.selected || attack.wait > 0 {
            continue;
        }
        let here = on_ground(at);
        let nearest = (scratch.foes.iter())
            .map(|&(foe, there)| (foe, (there - here).length()))
            .filter(|&(_, away)| away <= attack.range)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((foe, _)) = nearest {
            scratch.hits.push((foe, attack.damage));
            attack.wait = attack.cooldown;
        }
    }
    for &(foe, damage) in &scratch.hits {
        if let Some(health) = world.get_mut::<Health>(foe) {
            health.current -= damage;
        }
    }
    scratch.dead.clear();
    for (entity, health) in world.query::<(Entity, &Health)>() {
        if health.current <= 0.0 {
            scratch.dead.push(entity);
        }
    }
    for &entity in &scratch.dead {
        // Fails only for an entity that's gone, which a parent's removal may have taken.
        let _ = world.despawn(entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let mut world = World::default();
        world.register::<RtsCamera>().expect("registers");
        world.register::<Unit>().expect("registers");
        world.register::<Health>().expect("registers");
        world.register::<Attack>().expect("registers");
        world.register::<Route>().expect("registers");
        world.register::<Enemy>().expect("registers");
        world.register::<Obstacle>().expect("registers");
        world
    }

    fn unit(world: &mut World, at: Vec2) -> Entity {
        let unit = Unit {
            speed: 6.0,
            radius: 0.5,
            selected: false,
            moving: false,
            target: at,
            stalled: 0,
        };
        let at = Transform {
            translation: DVec3::new(f64::from(at.x), 0.0, f64::from(at.y)),
            ..Transform::IDENTITY
        };
        world.spawn((unit, at))
    }

    fn run(game: &mut Game, world: &mut World, ticks: u64) {
        for number in 0..ticks {
            game.tick(world, Tick { number });
        }
    }

    /// The start scene saves to its own bytes, so a `gg scene save` changes only what was edited.
    #[test]
    fn the_scene_saves_as_it_is() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut cx = Context::seeded(1).root(dir);
        Game::new().load(&mut cx).expect("loaded");
        let path = dir.join("scenes/main.ron");
        let text = std::fs::read_to_string(&path).expect("the scene");
        let roots = cx.load_scene_file(path).expect("the scene loads");
        let saved = cx.world().save_scene(&roots).expect("saved");
        assert_eq!(saved, text);
    }

    #[test]
    fn rays_meet_the_ground_and_boxes() {
        let down = Vec3::new(0.0, -1.0, 1.0);
        let hit = ground(&Cast::new(DVec3::new(1.0, 10.0, -10.0), down)).expect("looks down");
        assert!((hit - Vec2::new(1.0, 0.0)).length() < 1e-4, "{hit}");
        assert_eq!(ground(&Cast::new(DVec3::Y, Vec3::Z)), None);
        let ray = (Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y);
        assert!(hit_box(ray, Vec3::ZERO, 0.4, 1.0).is_some());
        assert!(hit_box(ray, Vec3::new(2.0, 0.0, 0.0), 0.4, 1.0).is_none());
    }

    #[test]
    fn quads_hold_what_is_inside_either_way_round() {
        let square = [Vec2::ZERO, Vec2::X * 4.0, Vec2::splat(4.0), Vec2::Y * 4.0];
        let reversed = [square[3], square[2], square[1], square[0]];
        for quad in [square, reversed] {
            assert!(in_quad(&quad, Vec2::splat(2.0), 0.0));
            assert!(!in_quad(&quad, Vec2::new(5.0, 2.0), 0.0));
            assert!(in_quad(&quad, Vec2::new(4.3, 2.0), 0.5));
        }
    }

    #[test]
    fn selected_units_walk_to_the_order_apart_and_stop() {
        let mut world = world();
        let mut game = Game::new();
        let units: Vec<_> = (0..4)
            .map(|i| unit(&mut world, Vec2::new(i as f32 * 2.0, 5.0)))
            .collect();
        let bystander = unit(&mut world, Vec2::new(-10.0, 5.0));
        game.commands.push(Command::SelectBox {
            corners: [
                Vec2::new(-1.0, 4.0),
                Vec2::new(8.0, 4.0),
                Vec2::new(8.0, 6.0),
                Vec2::new(-1.0, 6.0),
            ],
        });
        game.commands.push(Command::Order {
            to: Vec2::new(0.0, -10.0),
            queue: false,
        });
        run(&mut game, &mut world, 640);
        let still = world.get::<Unit>(bystander).expect("alive");
        assert!(!still.selected && !still.moving);
        let spots: Vec<Vec2> = units
            .iter()
            .map(|&unit| on_ground(world.get::<Transform>(unit).expect("alive")))
            .collect();
        for (nth, spot) in spots.iter().enumerate() {
            assert!(!world.get::<Unit>(units[nth]).expect("alive").moving);
            assert!((*spot - Vec2::new(0.0, -10.0)).length() < 2.0, "{spot}");
            for other in &spots[nth + 1..] {
                assert!((*spot - *other).length() > 1.0, "{spot} {other}");
            }
        }
    }

    #[test]
    fn the_camera_pans_and_zooms_within_limits() {
        let mut world = world();
        let mut game = Game::new();
        let rig = RtsCamera {
            focus: Vec2::ZERO,
            distance: 24.0,
            pitch: 0.95,
            pan: Vec2::ZERO,
        };
        let camera = world.spawn((rig, Transform::IDENTITY));
        game.commands.push(Command::Pan { dir: Vec2::Y });
        game.commands.push(Command::Zoom { lines: 100.0 });
        run(&mut game, &mut world, 64);
        let rig = *world.get::<RtsCamera>(camera).expect("alive");
        assert!(rig.focus.y < -1.0 && rig.focus.x == 0.0, "{}", rig.focus);
        assert_eq!(rig.distance, ZOOM[0]);
    }

    #[test]
    fn units_stop_short_of_targets_they_cannot_reach() {
        assert!(!gave_up(3, 5.0, 0.5) && gave_up(STALL, 5.0, 0.5));
        assert!(gave_up(NEAR_STALL, 0.4, 0.5) && !gave_up(NEAR_STALL, 5.0, 0.5));
        let mut world = world();
        let mut game = Game::new();
        let inside = world.spawn((
            Obstacle { radius: 2.0 },
            Transform {
                translation: DVec3::new(0.0, 0.0, -10.0),
                ..Transform::IDENTITY
            },
        ));
        let walker = unit(&mut world, Vec2::new(0.0, 0.0));
        let edge = unit(&mut world, Vec2::new(20.0, 0.0));
        for unit in [walker, edge] {
            world.get_mut::<Unit>(unit).expect("alive").selected = true;
        }
        game.commands.push(Command::Order {
            to: Vec2::new(0.0, -10.0),
            queue: false,
        });
        run(&mut game, &mut world, 640);
        assert!(world.contains(inside));
        for unit in [walker, edge] {
            assert!(!world.get::<Unit>(unit).expect("alive").moving);
        }
        // The first slot lies inside the rock; the other is past the ground's edge.
        game.commands.push(Command::Order {
            to: Vec2::new(40.0, 0.0),
            queue: false,
        });
        run(&mut game, &mut world, 640);
        let at = on_ground(world.get::<Transform>(edge).expect("alive"));
        assert!(at.x <= BOUND, "{at}");
        assert!(!world.get::<Unit>(edge).expect("alive").moving);
    }

    #[test]
    fn queued_orders_are_walked_in_order() {
        let mut world = world();
        let mut game = Game::new();
        let walker = unit(&mut world, Vec2::ZERO);
        world.insert(walker, Route::default()).expect("alive");
        world.get_mut::<Unit>(walker).expect("alive").selected = true;
        let places = [
            Vec2::new(5.0, 0.0),
            Vec2::new(5.0, 5.0),
            Vec2::new(0.0, 5.0),
        ];
        for (nth, to) in places.into_iter().enumerate() {
            game.commands.push(Command::Order { to, queue: nth > 0 });
        }
        game.tick(&mut world, Tick { number: 0 });
        assert_eq!(world.get::<Route>(walker).expect("alive").points.len(), 2);
        let mut targets = vec![world.get::<Unit>(walker).expect("alive").target];
        for number in 1..640 {
            game.tick(&mut world, Tick { number });
            let target = world.get::<Unit>(walker).expect("alive").target;
            if targets.last() != Some(&target) {
                targets.push(target);
            }
        }
        assert_eq!(targets, places);
        let end = on_ground(world.get::<Transform>(walker).expect("alive"));
        assert!(end.distance(places[2]) < 0.1, "{end}");
    }

    #[test]
    fn selected_units_hit_the_nearest_enemy_and_the_dead_are_removed() {
        let mut world = world();
        let mut game = Game::new();
        let hero = unit(&mut world, Vec2::ZERO);
        let bystander = unit(&mut world, Vec2::new(0.0, 1.5));
        let attack = Attack {
            range: 5.0,
            damage: 30.0,
            cooldown: 4,
            wait: 0,
        };
        for unit in [hero, bystander] {
            world.insert(unit, attack).expect("alive");
            world
                .insert(
                    unit,
                    Health {
                        current: 50.0,
                        max: 50.0,
                    },
                )
                .expect("alive");
        }
        let health = Health {
            current: 50.0,
            max: 50.0,
        };
        let enemy = |world: &mut World, z: f64| {
            let at = Transform {
                translation: DVec3::new(0.0, 0.0, z),
                ..Transform::IDENTITY
            };
            world.spawn((Enemy, health, at))
        };
        let (near, far) = (enemy(&mut world, -3.0), enemy(&mut world, -9.0));
        world.get_mut::<Unit>(hero).expect("alive").selected = true;
        run(&mut game, &mut world, 1);
        assert_eq!(world.get::<Health>(near).expect("alive").current, 20.0);
        run(&mut game, &mut world, 8);
        assert!(!world.contains(near), "30 damage twice takes 50 health");
        assert_eq!(world.get::<Health>(far).expect("alive").current, 50.0);
        assert!(world.contains(bystander), "unselected units don't attack");
    }

    #[test]
    fn select_many_selects_exactly_those() {
        let mut world = world();
        let mut game = Game::new();
        let a = unit(&mut world, Vec2::ZERO);
        let b = unit(&mut world, Vec2::X * 3.0);
        game.commands.push(Command::SelectMany { units: vec![b] });
        run(&mut game, &mut world, 1);
        assert!(!world.get::<Unit>(a).expect("alive").selected);
        assert!(world.get::<Unit>(b).expect("alive").selected);
    }
}
