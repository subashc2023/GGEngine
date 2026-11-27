//! tps, a third-person shooter skeleton made with GGEngine.
//!
//! WASD walks the player, Space jumps, Ctrl or a pad's east button rolls (a burst, with a
//! cooldown), and the mouse turns the camera while the cursor is grabbed. Holding the right button
//! aims, zooming in over the shoulder and slowing the walk, and the left one fires a ray from the
//! view's center at the targets, which tumble over and go. Pickups spin and bob until the player walks into one, and the
//! HUD (`ui/hud.ui`) counts them. A click in the view grabs the cursor, and Escape lets it go.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::f32::consts::{PI, TAU};
use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use gg_engine::math::libm;
use gg_engine::math::{DVec3, Vec3Swizzles};
use gg_engine::prelude::*;
use gg_engine::{BodyType, Cast, CharacterController3D, GameUi, RigidBody3D, Shape3D};

#[cfg(test)]
mod tests;

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's debug overlays: what `main` runs, and what its recordings replay.
fn app() -> App {
    let app = App::new("tps")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
    app.layer(Game::new()).debug_overlays()
}

/// The furthest a rig looks down and up, in radians: the camera orbits, so up puts it low.
const PITCH: [f32; 2] = [-1.2, 0.5];

/// How long a knocked target tumbles before it goes, in seconds.
const TOPPLE: f32 = 3.0;

/// How hard a shot knocks a target, in units a second, along the shot and a little up.
const KNOCK: f32 = 7.0;

/// How near the player's middle a pickup's must come to be collected, in units.
const REACH: f32 = 1.1;

/// How far a shot goes, in units.
const RANGE: f32 = 200.0;

/// What the keys steer: it walks `speed` units a second, and turns toward where it walks.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Player {
    #[reflect(min = 0.0, default = 6.0)]
    speed: f32,
    /// The same while the camera is fully aimed.
    #[reflect(min = 0.0, default = 2.5)]
    aim_speed: f32,
    /// How fast it leaves the ground, in units a second.
    #[reflect(min = 0.0, default = 6.5)]
    jump_speed: f32,
    /// How fast it turns to face its way, in radians a second.
    #[reflect(min = 0.0, default = 12.0)]
    turn_rate: f32,
    /// As the last steer set it: right and forward from the camera, of length at most 1.
    heading: Vec2,
    /// Its turn to its left about up, in radians, from facing -Z.
    facing: f32,
    /// Set by a jump command, and cleared as the tick takes it.
    jump: bool,
    /// How fast a dodge roll goes, in units a second.
    #[reflect(min = 0.0, default = 14.0)]
    roll_speed: f32,
    /// How long a roll lasts, in seconds.
    #[reflect(min = 0.0, default = 0.35)]
    roll_time: f32,
    /// How long after a roll starts the next may, in seconds.
    #[reflect(min = 0.0, default = 1.0)]
    roll_cooldown: f32,
    /// Set by a dodge command, and cleared as the tick takes it.
    dodge: bool,
    /// Seconds left of the roll under way; 0 when none is.
    rolling: f32,
    /// Seconds until a roll may start; 0 when one may.
    cooldown: f32,
    /// The way a roll goes, on the level, of length 1: where it started.
    roll_way: Vec2,
}

/// Puts its entity, a camera, behind `target`'s shoulder, turned by the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Rig {
    target: Option<Entity>,
    /// Its turn to its left about up, in radians, from facing -Z.
    yaw: f32,
    /// Its turn up, in radians, within [`PITCH`].
    pitch: f32,
    /// How far a unit of the mouse's motion turns it, in radians.
    #[reflect(min = 0.0, default = 0.0012)]
    sensitivity: f32,
    /// How far above the target's origin it orbits.
    pivot_height: f32,
    /// How far behind that point it sits, and how far right of it.
    #[reflect(min = 0.0, default = 7.5)]
    distance: f32,
    #[reflect(default = 1.9)]
    shoulder: f32,
    /// The same, aiming.
    #[reflect(min = 0.0, default = 1.8)]
    aim_distance: f32,
    #[reflect(default = 0.55)]
    aim_shoulder: f32,
    /// How far from `distance` to `aim_distance` it is now, from 0 to 1.
    #[reflect(min = 0.0, max = 1.0)]
    aim: f32,
    /// How fast `aim` follows the button, as a share of what's left of it a second.
    #[reflect(min = 0.0, default = 7.0)]
    aim_rate: f32,
    /// Whether the aim button is held.
    aiming: bool,
}

/// What a shot knocks over: it tumbles for [`TOPPLE`] seconds, then goes.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Target {
    /// What it adds to the score.
    points: u32,
    /// Seconds left before a knocked target goes; 0 while it stands.
    #[reflect(min = 0.0)]
    fall: f32,
}

/// A thing lying about that spins and bobs in place until the player walks into it.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Pickup {
    /// How high it floats when it bobs level, in the world's units. Each tick sets its height from
    /// this, so a `Transform` edit of its height doesn't last: edit this.
    home: f32,
    /// How fast it spins about up, in radians a second.
    #[reflect(default = 2.0)]
    spin: f32,
    /// How far it bobs above and below `home`.
    #[reflect(min = 0.0, default = 0.15)]
    bob: f32,
    /// How fast it bobs, in radians a second.
    #[reflect(min = 0.0, default = 3.0)]
    bob_rate: f32,
    /// Where it is in its bob, in radians: the clock of its motion, so it starts again from the
    /// world's as a restart restores it.
    phase: f32,
}

/// What the shots have done: read it with `cargo gg get Score`.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Score {
    hits: u32,
    points: u32,
    /// How many pickups the player has walked into.
    pickups: u32,
    /// Where the last hit landed.
    last: Vec3,
}

/// What the mouse and keys do, applied at the next tick. Recordings keep them.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Walks every player toward `heading` until the next.
    Steer { heading: Vec2 },
    /// Has every player jump, if it stands.
    Jump,
    /// Has every player roll, if its cooldown is over.
    Dodge,
    /// Turns every rig by the mouse's raw motion, right and down.
    Look { by: Vec2 },
    /// Holds or lets go of the aim.
    Aim { on: bool },
    /// Fires a ray from every rig's center.
    Fire,
}

/// What the keys are bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Steer,
    Jump,
    Dodge,
}

/// The game's layer.
struct Game {
    actions: Actions<Act>,
    commands: Vec<Command>,
    scripts: Scripts,
    /// Whether the mouse turns the camera: from a click in the view until Escape.
    grab: bool,
    /// The HUD, from `ui/hud.ui`.
    hud: Option<GameUi>,
    /// Whether the HUD's text failing to set has been warned of.
    hud_warned: bool,
}

impl Game {
    fn new() -> Self {
        let mut actions = Actions::default();
        let bound = actions.bind_all([
            (Act::Steer, Binding::WASD),
            (Act::Steer, Binding::ARROWS),
            (Act::Steer, Binding::stick(Stick::Left)),
            (Act::Jump, Binding::Button(Source::Key(Key::Space))),
            (
                Act::Jump,
                Binding::Button(Source::PadButton(PadButton::South)),
            ),
            (Act::Dodge, Binding::Button(Source::Key(Key::ControlLeft))),
            (Act::Dodge, Binding::Button(Source::Key(Key::ControlRight))),
            (
                Act::Dodge,
                Binding::Button(Source::PadButton(PadButton::East)),
            ),
        ]);
        bound.expect("the bindings are pairs of axes, of other sources");
        // What `gg actions` says each control does.
        let walk = "Walks the player on the level: [x, y], right and forward of where the camera \
                    looks by `Rig.yaw` alone; 1 is `Player.speed` units a second. The player turns \
                    to face its way, or the camera's while aiming";
        actions.describe(Act::Steer, walk);
        let jump = "Jumps if the player stands, leaving the ground at `Player.jump_speed` units a \
                    second; a press in the air does nothing. Gravity is 9.81 units a second \
                    squared, times `CharacterController3D.gravity`";
        actions.describe(Act::Jump, jump);
        let dodge = "Rolls the player in the way it walks, or faces if it stands, at \
                     `Player.roll_speed` units a second for `Player.roll_time` seconds, steering \
                     nothing meanwhile. It needs the ground, and `Player.roll_cooldown` seconds \
                     since the last roll began";
        actions.describe(Act::Dodge, dodge);
        let look = format!(
            "Turns the camera around the player while the cursor is grabbed, `Rig.sensitivity` \
             radians a unit. Right turns right, lowering `Rig.yaw`, kept within 0 to 2π; down \
             looks down, lowering `Rig.pitch`, kept within {} to {}",
            PITCH[0], PITCH[1]
        );
        actions.describe_control(Control::Motion, look);
        let click = Control::Source(Source::Button(MouseButton::Left));
        let fire = format!(
            "A click in the view grabs the cursor, unless it is over the UI, and fires nothing. \
             Once it's grabbed, each click fires a ray from the camera's center, knocking \
             over the `Target` it hits first, up to {RANGE} units: it tumbles {TOPPLE} seconds, \
             then goes. A hit adds to `Score.hits` and `Score.points`, once for each target"
        );
        actions.describe_control(click, fire);
        let aim = Control::Source(Source::Button(MouseButton::Right));
        let zoom = "While the cursor is grabbed, holding it aims: the camera eases in over the \
                    shoulder, the player faces where the camera looks and walks at \
                    `Player.aim_speed`. Letting go stops it";
        actions.describe_control(aim, zoom);
        let escape = Control::Source(Source::Key(Key::Escape));
        let release = "Lets go of the cursor, so the mouse stops looking, and stops aiming";
        actions.describe_control(escape, release);
        Self {
            actions,
            commands: Vec::new(),
            scripts: Scripts::new().expect("Luau starts"),
            grab: false,
            hud: None,
            hud_warned: false,
        }
    }
}

impl Layer for Game {
    /// Registers the game's components, which the start scene names, and loads its script.
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        // Registered first: the script names Score.
        cx.world_mut()
            .register_all::<(Player, Rig, Target, Pickup, Score)>()?;
        self.scripts.load(cx, root.join("scripts/report.luau"))?;
        self.hud = Some(cx.add_ui("hud", root.join("ui/hud.ui"))?);
        Ok(())
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
        let heading = Vec2::ZERO;
        self.commands.push(Command::Steer { heading });
        self.commands.push(Command::Aim { on: false });
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for command in self.commands.drain(..) {
            apply(world, &command);
        }
        walk(world);
        fall_away(world);
        float(world);
        collect(world);
        step_physics(world);
        orbit(world);
        self.scripts.run(world);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    /// What `gg actions` lists, and `gg input` presses by name.
    fn actions(&self) -> Controls {
        self.actions.controls()
    }

    /// Grabs the cursor while the world has a rig to turn with it, and draws the HUD.
    fn update(&mut self, cx: &mut Context, _: Duration) {
        if let Some(hud) = self.hud {
            let pickups = cx.world().query::<&Score>().next().map_or(0, |s| s.pickups);
            let set = hud.set_text_fmt(cx, "pickups", format_args!("PICKUPS {pickups}"));
            if let Err(error) = set
                && !self.hud_warned
            {
                self.hud_warned = true;
                warn!(code = "tps.hud", error = %error, "the HUD's pickups text can't be set");
            }
            hud.draw(cx);
        }
        self.grab &= cx.world().query::<&Rig>().next().is_some();
        cx.set_cursor_grabbed(self.grab);
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, so the next tick applies them.
        let commands = &mut self.commands;
        self.actions.event(event, |change| match change {
            Change::Axis2(Act::Steer, heading) => commands.push(Command::Steer { heading }),
            Change::Pressed(Act::Jump) => commands.push(Command::Jump),
            Change::Pressed(Act::Dodge) => commands.push(Command::Dodge),
            _ => {}
        });
        let rigs = cx.world().query::<&Rig>().next().is_some();
        match *event {
            // The click that grabs the cursor fires nothing.
            Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            } if rigs && !self.grab => self.grab = !cx.pointer_over_ui(),
            Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            } if self.grab => commands.push(Command::Fire),
            Event::MouseButton {
                button: MouseButton::Right,
                pressed,
            } if self.grab || !pressed => commands.push(Command::Aim { on: pressed }),
            Event::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => {
                self.grab = false;
                commands.push(Command::Aim { on: false });
            }
            // Each tick turns the rigs once, by the motion since the tick before.
            Event::MouseMotion { x, y } if self.grab => {
                let by = Vec2::new(x as f32, y as f32);
                match commands.last_mut() {
                    Some(Command::Look { by: queued }) => *queued += by,
                    _ => commands.push(Command::Look { by }),
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

/// Applies `command` to `world`.
fn apply(world: &mut World, command: &Command) {
    match *command {
        Command::Steer { heading } => {
            for player in world.query_mut::<&mut Player>() {
                player.heading = heading.clamp_length_max(1.0);
            }
        }
        Command::Jump => {
            for player in world.query_mut::<&mut Player>() {
                player.jump = true;
            }
        }
        Command::Dodge => {
            for player in world.query_mut::<&mut Player>() {
                player.dodge = true;
            }
        }
        Command::Look { by } => {
            for rig in world.query_mut::<&mut Rig>() {
                // Right turns to the right, about the world's up, and down looks down.
                rig.yaw = (rig.yaw - by.x * rig.sensitivity).rem_euclid(TAU);
                rig.pitch = (rig.pitch - by.y * rig.sensitivity).clamp(PITCH[0], PITCH[1]);
            }
        }
        Command::Aim { on } => {
            for rig in world.query_mut::<&mut Rig>() {
                rig.aiming = on;
            }
        }
        Command::Fire => fire(world),
    }
}

/// Where a rig's camera looks from, and which way.
fn view(world: &World) -> Option<(Option<Entity>, DVec3, Vec3)> {
    let (rig, at) = world.query::<(&Rig, &Transform)>().next()?;
    Some((rig.target, at.translation, at.rotation * Vec3::NEG_Z))
}

/// Shoots a ray from the camera's middle, knocking over the target it hits first.
fn fire(world: &mut World) {
    let Some((shooter, from, toward)) = view(world) else {
        return;
    };
    let mut cast = Cast::new(from, toward).within(RANGE);
    cast.exclude = shooter;
    let Some(hit) = world.cast_ray(cast) else {
        return;
    };
    let Some(&Target { points, fall }) = world.get::<Target>(hit.entity) else {
        return;
    };
    // One already down scores nothing more.
    if fall > 0.0 {
        return;
    }
    knock(world, hit.entity, toward);
    for score in world.query_mut::<&mut Score>() {
        score.hits += 1;
        score.points += points;
        score.last = hit.point.as_vec3();
    }
}

/// Knocks `target` along `toward`: a dynamic body now, tumbling and thrown, until [`fall_away`]
/// removes it.
fn knock(world: &mut World, target: Entity, toward: Vec3) {
    if let Some(target) = world.get_mut::<Target>(target) {
        target.fall = TOPPLE;
    }
    if let Some(body) = world.get_mut::<RigidBody3D>(target) {
        body.body_type = BodyType::Dynamic;
        body.velocity = toward.with_y(0.0).normalize_or_zero() * KNOCK + Vec3::Y * 2.0;
        // About the axis across the shot, so it rolls the way it was hit.
        body.angular_velocity = Vec3::Y.cross(toward).normalize_or_zero() * (KNOCK * 1.5);
    }
}

/// Counts knocked targets down and removes those whose time is up.
fn fall_away(world: &mut World) {
    let dt = Tick::DT.as_secs_f32();
    let mut gone = Vec::new();
    for (entity, target) in world.query_mut::<(Entity, &mut Target)>() {
        if target.fall > 0.0 {
            target.fall -= dt;
            if target.fall <= 0.0 {
                gone.push(entity);
            }
        }
    }
    for entity in gone {
        world.despawn(entity).expect("a target that fell");
    }
}

/// Spins and bobs each pickup.
fn float(world: &mut World) {
    let dt = Tick::DT.as_secs_f32();
    for (pickup, at) in world.query_mut::<(&mut Pickup, &mut Transform)>() {
        pickup.phase = (pickup.phase + pickup.bob_rate * dt).rem_euclid(TAU);
        let y = pickup.home + pickup.bob * libm::sinf(pickup.phase);
        at.translation.y = f64::from(y);
        at.rotation = (Quat::from_rotation_y(pickup.spin * dt) * at.rotation).normalize();
    }
}

/// Removes each pickup a player is within [`REACH`] of, and counts it.
fn collect(world: &mut World) {
    let Some(player) = world.query::<(&Player, &Transform)>().next() else {
        return;
    };
    let from = player.1.translation;
    let mut taken = Vec::new();
    for (entity, _, at) in world.query::<(Entity, &Pickup, &Transform)>() {
        if (at.translation - from).length() < f64::from(REACH) {
            taken.push(entity);
        }
    }
    for entity in taken {
        world.despawn(entity).expect("a pickup just seen");
        for score in world.query_mut::<&mut Score>() {
            score.pickups += 1;
            info!(pickups = score.pickups, "Pickup collected");
        }
    }
}

/// Sets each player's velocity from its heading, turning it toward the way it goes, and jumps it.
fn walk(world: &mut World) {
    let dt = Tick::DT.as_secs_f32();
    let yaw = world.query::<&Rig>().next().map_or(0.0, |rig| rig.yaw);
    let aiming = world.query::<&Rig>().next().is_some_and(|rig| rig.aiming);
    let aim = world.query::<&Rig>().next().map_or(0.0, |rig| rig.aim);
    let query = world.query_mut::<(&mut Player, &mut Transform, &mut CharacterController3D)>();
    for (player, at, character) in query {
        // Forward is where the camera looks, and right is to its right.
        let across =
            Quat::from_rotation_y(yaw) * Vec3::new(player.heading.x, 0.0, -player.heading.y);
        let up = if player.jump && character.grounded {
            player.jump_speed
        } else {
            character.velocity.y
        };
        player.jump = false;
        // Slower the further the camera has zoomed in.
        let speed = player.speed + (player.aim_speed - player.speed) * aim;
        player.cooldown = (player.cooldown - dt).max(0.0);
        player.rolling = (player.rolling - dt).max(0.0);
        if player.dodge && player.cooldown <= 0.0 && character.grounded {
            // The way it walks, else the way it faces.
            let way = if across.xz().length_squared() > 1e-6 {
                across.xz().normalize()
            } else {
                (Quat::from_rotation_y(player.facing) * Vec3::NEG_Z).xz()
            };
            player.roll_way = way;
            player.rolling = player.roll_time;
            player.cooldown = player.roll_cooldown;
        }
        player.dodge = false;
        if player.rolling > 0.0 {
            // A burst along the way it began, whatever the keys do, facing along it.
            let way = player.roll_way;
            character.velocity =
                Vec3::new(way.x, up, way.y) * Vec3::new(player.roll_speed, 1.0, player.roll_speed);
            player.facing = libm::atan2f(-way.x, -way.y).rem_euclid(TAU);
            at.rotation = Quat::from_rotation_y(player.facing);
            continue;
        }
        character.velocity = (across * speed).with_y(up);
        // Facing the camera's way while aiming, else the way it walks.
        let wants = if aiming {
            Some(yaw)
        } else {
            (across.xz().length_squared() > 1e-6).then(|| libm::atan2f(-across.x, -across.z))
        };
        if let Some(wants) = wants {
            let delta = (wants - player.facing + PI).rem_euclid(TAU) - PI;
            let step = player.turn_rate * dt;
            player.facing = (player.facing + delta.clamp(-step, step)).rem_euclid(TAU);
            at.rotation = Quat::from_rotation_y(player.facing);
        }
    }
}

/// Puts each rig's camera behind its target's shoulder, nearer where a wall would come between.
fn orbit(world: &mut World) {
    let dt = Tick::DT.as_secs_f32();
    let Some((camera, rig)) = world.query::<(Entity, &Rig)>().next().map(|(e, r)| (e, *r)) else {
        return;
    };
    // Eases in and out: it closes `aim_rate` of what's left each second, and the view takes the
    // smoothstep of it.
    let goal = f32::from(rig.aiming);
    let aim = rig.aim + (goal - rig.aim) * (1.0 - libm::expf(-rig.aim_rate * dt));
    let eased = aim * aim * (3.0 - 2.0 * aim);
    let Some(target) = rig.target.and_then(|t| world.get::<Transform>(t)) else {
        return;
    };
    let pivot = target.translation + DVec3::Y * f64::from(rig.pivot_height);
    let turn = Quat::from_rotation_y(rig.yaw) * Quat::from_rotation_x(rig.pitch);
    let distance = rig.distance + (rig.aim_distance - rig.distance) * eased;
    let shoulder = rig.shoulder + (rig.aim_shoulder - rig.shoulder) * eased;
    let offset = turn * Vec3::new(shoulder, 0.0, distance);
    // A sphere cast from the pivot, so a wall pulls the camera in rather than hiding it.
    let mut cast = Cast::new(pivot, offset).within(offset.length());
    cast.exclude = rig.target;
    let reach = match world.cast_shape(Shape3D::Sphere { radius: 0.2 }, Quat::IDENTITY, cast) {
        Some(hit) => hit.distance,
        None => offset.length(),
    };
    let at = pivot + (offset.normalize_or_zero() * reach).as_dvec3();
    if let Some(rig) = world.get_mut::<Rig>(camera) {
        rig.aim = aim;
    }
    if let Some(transform) = world.get_mut::<Transform>(camera) {
        transform.translation = at;
        transform.rotation = turn;
    }
}
