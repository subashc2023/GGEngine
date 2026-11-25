//! topdown, a game made with GGEngine.
//!
//! A top-down twin-stick shooter. WASD, the arrows or a pad's left stick move the player, the mouse
//! or a pad's right stick aims, and the left button or right trigger fires. Enemies walk at the
//! player: a shot removes both, a touch sends the player back to the start. The room, the player
//! and the enemies are in `scenes/main.ron`, which a development build applies as it's saved.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::ops::ControlFlow;
use std::process::ExitCode;

use gg_engine::math::DVec3;
use gg_engine::prelude::*;
use gg_engine::{BodyType, BoxCollider2D, Camera, Physics2D, RigidBody2D};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's Performance and Inspector panels: what `main` runs, and what its
/// recordings replay.
fn app() -> App {
    let app = App::new("topdown")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
    app.layer(Game::new()).dev_panels()
}

/// Where the player starts, and goes back to when an enemy touches it.
const START: Vec2 = Vec2::new(0.0, 0.0);

/// The z the camera sees from.
const CAMERA_Z: f64 = 10.0;

/// A shot's speed in units a second, its lifetime in ticks (about 1.4 s), its size, and the ticks
/// between shots.
const SHOT_SPEED: f32 = 14.0;
const SHOT_TICKS: u32 = 90;
const SHOT_SIZE: f32 = 0.2;
const SHOT_EVERY: u32 = 10;

/// Collision groups, as the scene's colliders set them: the player is 1, which only enemies and
/// walls meet. A shot meets enemies and walls alone.
const ENEMY: u32 = 2;
const WALL: u32 = 4;
const SHOT: u32 = 8;

/// What the player steers and shoots with.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Player {
    #[reflect(min = 0.0, default = 5.0)]
    speed: f32,
    /// As the last steer set it, of length at most 1.
    heading: Vec2,
    /// Which way shots go, of length 1.
    aim: Vec2,
    /// Whether the trigger is held.
    firing: bool,
    /// Ticks until the next shot.
    cooldown: u32,
}

/// Walks at the player.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Enemy {
    #[reflect(min = 0.0, default = 2.0)]
    speed: f32,
    /// Where it goes back to when the player does.
    home: Vec2,
}

/// A shot, with the ticks it has left.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Shot {
    ticks: u32,
}

/// What the keys, mouse and pads do, applied at the next tick. Recordings keep them.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Turns every player toward `heading` until the next.
    Steer { heading: Vec2 },
    /// Aims every player along `aim`, a unit vector.
    Aim { aim: Vec2 },
    /// Holds or lets go of the trigger.
    Fire { held: bool },
}

/// What the keys, mouse and pads are bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Steer,
    AimStick,
    Fire,
}

/// The game's layer.
struct Game {
    actions: Actions<Act>,
    commands: Vec<Command>,
    /// The tick's entities to despawn, kept to reuse its memory.
    dead: Vec<Entity>,
    /// The player's place this tick.
    player_at: Vec<Vec2>,
}

impl Game {
    fn new() -> Self {
        let mut actions = Actions::default();
        let steer = [Binding::WASD, Binding::ARROWS, Binding::stick(Stick::Left)];
        let fire = [
            Binding::Button(Source::Button(MouseButton::Left)),
            Binding::Button(Source::PadButton(PadButton::RightTrigger)),
        ];
        let bound = actions.bind_all(
            steer
                .map(|binding| (Act::Steer, binding))
                .into_iter()
                .chain([(Act::AimStick, Binding::stick(Stick::Right))])
                .chain(fire.map(|binding| (Act::Fire, binding))),
        );
        bound.expect("the bindings are of one kind each, of other sources");
        Self {
            actions,
            commands: Vec::new(),
            dead: Vec::new(),
            player_at: Vec::new(),
        }
    }
}

impl Layer for Game {
    /// Registers the game's components, which the start scene names.
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        cx.world_mut().register_all::<(Player, Enemy, Shot)>()
    }

    fn attach(&mut self, cx: &mut Context) {
        // No gravity: the room is seen from above.
        let mut physics = Physics2D::default();
        physics.gravity = Vec2::ZERO;
        cx.world_mut().insert_resource(physics);
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
        }
    }

    /// Lets go of what was held as the world was taken, as when `gg run` restarts the game.
    fn restored(&mut self, _: &mut Context) {
        let heading = Vec2::ZERO;
        self.commands.push(Command::Steer { heading });
        self.commands.push(Command::Fire { held: false });
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        let _zone = gg_engine::zone!();
        for command in self.commands.drain(..) {
            for player in world.query_mut::<&mut Player>() {
                match command {
                    Command::Steer { heading } => player.heading = heading,
                    Command::Aim { aim } => player.aim = aim,
                    Command::Fire { held } => player.firing = held,
                }
            }
        }
        self.player_at.clear();
        for (player, body, at) in world.query_mut::<(&Player, &mut RigidBody2D, &Transform)>() {
            body.velocity = player.heading * player.speed;
            self.player_at.push(at.translation.truncate().as_vec2());
        }
        let target = self.player_at.first().copied();
        if let Some(target) = target {
            for (enemy, body, at) in world.query_mut::<(&Enemy, &mut RigidBody2D, &Transform)>() {
                let toward = target - at.translation.truncate().as_vec2();
                body.velocity = toward.normalize_or_zero() * enemy.speed;
            }
            for (camera, at) in world.query_mut::<(&Camera, &mut Transform)>() {
                if camera.primary {
                    at.translation = target.extend(0.0).as_dvec3() + DVec3::Z * CAMERA_Z;
                }
            }
        }
        self.shoot(world);
        step_physics(world);
        self.hit(world);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, so the next tick applies them.
        let commands = &mut self.commands;
        self.actions.event(event, |change| match change {
            Change::Axis2(Act::Steer, heading) => commands.push(Command::Steer { heading }),
            Change::Axis2(Act::AimStick, stick) if stick.length() > 0.3 => {
                commands.push(Command::Aim {
                    aim: stick.normalize(),
                });
            }
            Change::Pressed(Act::Fire) => commands.push(Command::Fire { held: true }),
            Change::Released(Act::Fire) => commands.push(Command::Fire { held: false }),
            _ => {}
        });
        // The aim is from the player to the world point under the cursor, wherever the camera is.
        if let Event::CursorMoved { x, y } = *event {
            let hit = (cx.ray([x, y])).and_then(|ray| ray.meets_plane(DVec3::ZERO, Vec3::Z));
            let player = cx.world().query::<(&Player, &Transform)>().next();
            if let (Some(hit), Some((_, at))) = (hit, player) {
                let aim = (hit - at.translation).truncate().as_vec2();
                if aim.length() > 0.01 {
                    self.commands.push(Command::Aim {
                        aim: aim.normalize(),
                    });
                }
            }
        }
        ControlFlow::Continue(())
    }
}

impl Game {
    /// Counts each player's cooldown down, and spawns a shot from each whose trigger is held.
    fn shoot(&mut self, world: &mut World) {
        let mut shots = [None; 4];
        let mut count = 0;
        for (player, at) in world.query_mut::<(&mut Player, &Transform)>() {
            player.cooldown = player.cooldown.saturating_sub(1);
            if player.firing && player.cooldown == 0 && count < shots.len() {
                player.cooldown = SHOT_EVERY;
                let from = at.translation.truncate().as_vec2() + player.aim * 0.7;
                shots[count] = Some((from, player.aim));
                count += 1;
            }
        }
        for (from, aim) in shots.into_iter().flatten() {
            let at = Transform {
                translation: from.extend(0.2).as_dvec3(),
                scale: Vec3::new(SHOT_SIZE, SHOT_SIZE, 1.0),
                ..Transform::IDENTITY
            };
            let sprite = Sprite {
                color: Vec4::new(1.0, 0.9, 0.3, 1.0),
                ..Sprite::default()
            };
            let body = RigidBody2D {
                body_type: BodyType::Dynamic,
                fixed_rotation: true,
                continuous_collision: true,
                velocity: aim * SHOT_SPEED,
                ..RigidBody2D::default()
            };
            let collider = BoxCollider2D {
                friction: 0.0,
                membership: SHOT,
                filter: ENEMY | WALL,
                ..BoxCollider2D::default()
            };
            world.spawn((at, sprite, body, collider, Shot { ticks: SHOT_TICKS }));
        }
    }

    /// Removes shots that hit or ran out, and the enemies they hit, and sends a touched player home.
    fn hit(&mut self, world: &mut World) {
        self.dead.clear();
        let mut home = false;
        for contact in world.contacts() {
            let [a, b] = contact.entities;
            for (one, other) in [(a, b), (b, a)] {
                if world.get::<Shot>(one).is_some() {
                    self.dead.push(one);
                    if world.get::<Enemy>(other).is_some() {
                        self.dead.push(other);
                    }
                } else if world.get::<Player>(one).is_some() && world.get::<Enemy>(other).is_some()
                {
                    home = true;
                }
            }
        }
        for (entity, shot) in world.query_mut::<(Entity, &mut Shot)>() {
            shot.ticks = shot.ticks.saturating_sub(1);
            if shot.ticks == 0 {
                self.dead.push(entity);
            }
        }
        // A shot may be listed twice, as it touched a wall and an enemy.
        self.dead.sort_unstable();
        self.dead.dedup();
        for entity in self.dead.drain(..) {
            let _ = world.despawn(entity);
        }
        if home {
            for (_, at, body) in world.query_mut::<(&Player, &mut Transform, &mut RigidBody2D)>() {
                at.translation = START.extend(0.3).as_dvec3();
                body.velocity = Vec2::ZERO;
            }
            for (enemy, at, body) in world.query_mut::<(&Enemy, &mut Transform, &mut RigidBody2D)>()
            {
                at.translation = enemy.home.extend(0.3).as_dvec3();
                body.velocity = Vec2::ZERO;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
