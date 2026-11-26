//! platformer, a game made with GGEngine: a side-scrolling platformer skeleton.
//!
//! The player runs with A and D, the arrows or a pad's left stick, and jumps with Space or a pad's
//! south button. Rapier's 2D physics moves it; the camera follows it along the level.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::ops::ControlFlow;
use std::process::ExitCode;

use gg_engine::prelude::*;
use gg_engine::{Camera, Entity, PadButton, Physics2D, RigidBody2D, Source, Toward, step_physics};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's debug overlays: what `main` runs, and what its recordings replay.
fn app() -> App {
    let app = App::new("platformer")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
    app.layer(Game::new()).debug_overlays()
}

/// Ticks a jump press waits for the ground, so one just before landing still counts.
const JUMP_BUFFER: u32 = 6;
/// How far below a player's feet a platform's top may be for it to stand on it.
const FOOT_SLACK: f64 = 0.05;
/// Downward pull, units a second squared.
const GRAVITY: f32 = -25.0;

/// What the keys steer: a body that runs and jumps.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Player {
    /// Units a second along the level at full run.
    #[reflect(min = 0.0, default = 6.0)]
    speed: f32,
    #[reflect(min = 0.0, default = 11.0)]
    jump_speed: f32,
    /// As the last run set it, from -1 to 1.
    run: f32,
    /// Ticks a jump press stays wanted.
    jump_buffer: u32,
    /// Where it respawns.
    start: Vec2,
    /// The height below which it has fallen out of the level.
    fall_limit: f32,
    /// How many times it has reached the goal.
    levels_done: u32,
}

/// The box that ends the level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Component)]
struct Goal;

/// What the keys and pads do, applied at the next tick. Recordings keep them.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Runs every player along `run`, from -1 to 1, until the next.
    Run { run: f32 },
    /// Asks every player to jump as soon as it stands.
    Jump,
}

/// What the keys and pads are bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Run,
    Jump,
}

/// The game's layer.
struct Game {
    actions: Actions<Act>,
    commands: Vec<Command>,
}

impl Game {
    fn new() -> Self {
        let mut actions = Actions::default();
        let run = |negative, positive| Binding::Axis { negative, positive };
        let keys = |negative, positive| run(Source::Key(negative), Source::Key(positive));
        let bound = actions.bind_all([
            (Act::Run, keys(Key::KeyA, Key::KeyD)),
            (Act::Run, keys(Key::ArrowLeft, Key::ArrowRight)),
            (
                Act::Run,
                run(
                    Source::PadStick(Stick::Left, Toward::Left),
                    Source::PadStick(Stick::Left, Toward::Right),
                ),
            ),
            (Act::Jump, Binding::Button(Source::Key(Key::Space))),
            (
                Act::Jump,
                Binding::Button(Source::PadButton(PadButton::South)),
            ),
        ]);
        bound.expect("each action's bindings are of one kind");
        Self {
            actions,
            commands: Vec::new(),
        }
    }
}

impl Layer for Game {
    /// Registers what the start scene names.
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        cx.world_mut().register::<Player>()?;
        cx.world_mut().register::<Goal>()
    }

    fn attach(&mut self, cx: &mut Context) {
        let mut physics = Physics2D::default();
        physics.gravity = Vec2::new(0.0, GRAVITY);
        cx.world_mut().insert_resource(physics);
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
        }
    }

    /// Lets go of what was held as the world was taken, as when `gg run` restarts the game.
    fn restored(&mut self, _: &mut Context) {
        self.commands.push(Command::Run { run: 0.0 });
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for command in self.commands.drain(..) {
            for player in world.query_mut::<&mut Player>() {
                match command {
                    Command::Run { run } => player.run = run,
                    Command::Jump => player.jump_buffer = JUMP_BUFFER,
                }
            }
        }
        drive(world);
        step_physics(world);
        finish(world);
        follow(world);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    /// What `gg actions` lists, and `gg input` presses by name.
    fn actions(&self) -> Vec<(String, Binding)> {
        self.actions.bindings()
    }

    fn event(&mut self, _: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, so the next tick applies them.
        let commands = &mut self.commands;
        self.actions.event(event, |change| match change {
            Change::Axis(Act::Run, run) => commands.push(Command::Run { run }),
            Change::Pressed(Act::Jump) => commands.push(Command::Jump),
            _ => {}
        });
        ControlFlow::Continue(())
    }
}

/// Whether `player` stands: it touches a body whose top is at its feet or below.
fn grounded(world: &World, player: Entity) -> bool {
    let feet = |world: &World, entity| {
        let at = world.get::<Transform>(entity)?;
        Some(at.translation.y - f64::from(at.scale.y) * 0.5)
    };
    let Some(feet) = feet(world, player) else {
        return false;
    };
    world.contacts().any(|contact| {
        let Some(other) = contact.other(player) else {
            return false;
        };
        if contact.sensor {
            return false;
        }
        let Some(at) = world.get::<Transform>(other) else {
            return false;
        };
        at.translation.y + f64::from(at.scale.y) * 0.5 <= feet + FOOT_SLACK
    })
}

/// Sets each player's velocity from its run and its wanted jump, before physics steps.
fn drive(world: &mut World) {
    // Entities are listed first: `grounded` reads the world the body writes.
    let mut jumps = [None; 4];
    let mut count = 0;
    for (player, entity) in world.query::<(&Player, Entity)>() {
        if player.jump_buffer > 0 && count < jumps.len() {
            jumps[count] = Some(entity);
            count += 1;
        }
    }
    let mut ready = [false; 4];
    for (ready, entity) in ready.iter_mut().zip(jumps) {
        *ready = entity.is_some_and(|entity| grounded(world, entity));
    }
    for (player, body) in world.query_mut::<(&mut Player, &mut RigidBody2D)>() {
        body.velocity.x = player.run * player.speed;
        player.jump_buffer = player.jump_buffer.saturating_sub(1);
    }
    for (entity, ready) in jumps.into_iter().zip(ready) {
        if let (Some(entity), true) = (entity, ready) {
            let speed = world.get::<Player>(entity).map_or(0.0, |p| p.jump_speed);
            if let Some(body) = world.get_mut::<RigidBody2D>(entity) {
                body.velocity.y = speed;
            }
            if let Some(player) = world.get_mut::<Player>(entity) {
                player.jump_buffer = 0;
            }
        }
    }
}

/// Respawns players that fell out of the level or touched the goal.
fn finish(world: &mut World) {
    let mut reached = false;
    for contact in world.contacts() {
        let [a, b] = contact.entities;
        let goal = |e| world.get::<Goal>(e).is_some();
        let player = |e| world.get::<Player>(e).is_some();
        reached |= (goal(a) && player(b)) || (goal(b) && player(a));
    }
    for (player, at, body) in world.query_mut::<(&mut Player, &mut Transform, &mut RigidBody2D)>() {
        if reached {
            player.levels_done += 1;
            info!(levels_done = player.levels_done, "Goal reached");
        }
        if reached || at.translation.y < f64::from(player.fall_limit) {
            at.translation.x = f64::from(player.start.x);
            at.translation.y = f64::from(player.start.y);
            body.velocity = Vec2::ZERO;
        }
    }
}

/// Keeps the camera on the first player along the level.
fn follow(world: &mut World) {
    let Some(x) = world
        .query::<(&Player, &Transform)>()
        .next()
        .map(|(_, at)| at.translation.x)
    else {
        return;
    };
    for (_, at) in world.query_mut::<(&Camera, &mut Transform)>() {
        at.translation.x = x;
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
