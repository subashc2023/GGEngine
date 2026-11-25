//! tps, a third-person shooter skeleton made with GGEngine.
//!
//! WASD walks the player, Space jumps, and the mouse turns the camera while the cursor is grabbed.
//! Holding the right button aims, and the left one fires a ray from the view's center at the
//! targets. A click in the view grabs the cursor, and Escape lets it go.

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
use gg_engine::{Cast, CharacterController3D, Shape3D};

#[cfg(test)]
mod tests;

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's Performance and Inspector panels: what `main` runs, and what its
/// recordings replay.
fn app() -> App {
    let app = App::new("tps")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
    app.layer(Game::new()).dev_panels()
}

/// The furthest a rig looks down and up, in radians: the camera orbits, so up puts it low.
const PITCH: [f32; 2] = [-1.2, 0.5];

/// How far a shot goes, in units.
const RANGE: f32 = 200.0;

/// What the keys steer: it walks `speed` units a second, and turns toward where it walks.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Player {
    #[reflect(min = 0.0, default = 6.0)]
    speed: f32,
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
    #[reflect(min = 0.0, default = 0.0025)]
    sensitivity: f32,
    /// How far above the target's origin it orbits.
    pivot_height: f32,
    /// How far behind that point it sits, and how far right of it.
    #[reflect(min = 0.0, default = 4.5)]
    distance: f32,
    #[reflect(default = 0.8)]
    shoulder: f32,
    /// The same, aiming.
    #[reflect(min = 0.0, default = 1.8)]
    aim_distance: f32,
    #[reflect(default = 0.55)]
    aim_shoulder: f32,
    /// How far from `distance` to `aim_distance` it is now, from 0 to 1.
    #[reflect(min = 0.0, max = 1.0)]
    aim: f32,
    /// Whether the aim button is held.
    aiming: bool,
}

/// What a shot removes.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Target {
    /// What it adds to the score.
    points: u32,
}

/// What the shots have done: read it with `cargo gg get Score`.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Score {
    hits: u32,
    points: u32,
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
}

/// The game's layer.
struct Game {
    actions: Actions<Act>,
    commands: Vec<Command>,
    scripts: Scripts,
    /// Whether the mouse turns the camera: from a click in the view until Escape.
    grab: bool,
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
        ]);
        bound.expect("the bindings are pairs of axes, of other sources");
        Self {
            actions,
            commands: Vec::new(),
            scripts: Scripts::new().expect("Luau starts"),
            grab: false,
        }
    }
}

impl Layer for Game {
    /// Registers the game's components, which the start scene names, and loads its script.
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        // Registered first: the script names Score.
        cx.world_mut()
            .register_all::<(Player, Rig, Target, Score)>()?;
        self.scripts.load(cx, root.join("scripts/report.luau"))?;
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
        step_physics(world);
        orbit(world);
        self.scripts.run(world);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    /// Grabs the cursor while the world has a rig to turn with it.
    fn update(&mut self, cx: &mut Context, _: Duration) {
        self.grab &= cx.world().query::<&Rig>().next().is_some();
        cx.set_cursor_grabbed(self.grab);
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, so the next tick applies them.
        let commands = &mut self.commands;
        self.actions.event(event, |change| match change {
            Change::Axis2(Act::Steer, heading) => commands.push(Command::Steer { heading }),
            Change::Pressed(Act::Jump) => commands.push(Command::Jump),
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

/// Shoots a ray from the camera's middle, removing the target it hits first.
fn fire(world: &mut World) {
    let Some((shooter, from, toward)) = view(world) else {
        return;
    };
    let mut cast = Cast::new(from, toward).within(RANGE);
    cast.exclude = shooter;
    let Some(hit) = world.cast_ray(cast) else {
        return;
    };
    let Some(&Target { points }) = world.get::<Target>(hit.entity) else {
        return;
    };
    world.despawn(hit.entity).expect("the target the ray hit");
    for score in world.query_mut::<&mut Score>() {
        score.hits += 1;
        score.points += points;
        score.last = hit.point.as_vec3();
    }
}

/// Sets each player's velocity from its heading, turning it toward the way it goes, and jumps it.
fn walk(world: &mut World) {
    let dt = Tick::DT.as_secs_f32();
    let yaw = world.query::<&Rig>().next().map_or(0.0, |rig| rig.yaw);
    let aiming = world.query::<&Rig>().next().is_some_and(|rig| rig.aiming);
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
        character.velocity = (across * player.speed).with_y(up);
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
    let aim = rig.aim + (f32::from(rig.aiming) - rig.aim).clamp(-8.0 * dt, 8.0 * dt);
    let Some(target) = rig.target.and_then(|t| world.get::<Transform>(t)) else {
        return;
    };
    let pivot = target.translation + DVec3::Y * f64::from(rig.pivot_height);
    let turn = Quat::from_rotation_y(rig.yaw) * Quat::from_rotation_x(rig.pitch);
    let distance = rig.distance + (rig.aim_distance - rig.distance) * aim;
    let shoulder = rig.shoulder + (rig.aim_shoulder - rig.shoulder) * aim;
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
