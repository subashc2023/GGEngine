//! platformer, a game made with GGEngine: a side-scrolling platformer skeleton.
//!
//! The player runs with A and D, the arrows or a pad's left stick, and jumps with Space or a pad's
//! south button. Held, the jump floats near its top; let go of early, it is cut short; one more
//! press in the air jumps again (`Player` has the values). Rapier's 2D physics moves it, and the
//! camera follows it along the level.
//!
//! The level is in `scenes/main.ron`: coins (`Coin`) count on the HUD (`ui/hud.ui`) and go as the
//! player touches them; a platform with a `Mover` goes back and forth between two points and
//! carries the player; spikes (`Spikes`) and falls send the player back to the last checkpoint
//! (`Checkpoint`) it touched, or the start.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use gg_engine::prelude::*;
use gg_engine::{
    Camera, Entity, GameUi, PadButton, Physics2D, RigidBody2D, Source, Toward, step_physics,
};

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
    /// Share of gravity that pulls while a held jump is near the top of its rise or fall: 1 is
    /// all of it, so no float, and 0 hangs.
    #[reflect(min = 0.0, max = 1.0, default = 0.5)]
    float_gravity: f32,
    /// Vertical speed, units a second, below which a held jump floats.
    #[reflect(min = 0.0, default = 2.5)]
    float_band: f32,
    /// What a rising jump's speed is multiplied by when jump is let go, so a tap hops.
    #[reflect(min = 0.0, max = 1.0, default = 0.4)]
    cut: f32,
    /// Whether jump is down.
    jump_down: bool,
    /// Jumps in the air a landing gives back: 1 is a double jump.
    #[reflect(default = 1)]
    air_jumps: u32,
    /// Jumps in the air it has left before it lands.
    air_jumps_left: u32,
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
    /// The coins it has collected.
    coins: u32,
    /// Where a fall or spikes send it back to: the last checkpoint it touched, else `start`.
    checkpoint: Vec2,
}

/// A hazard: a sensor box that sends a player touching it back to its checkpoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Component)]
struct Spikes;

/// A sensor box that makes its own place the checkpoint of a player touching it: the player comes
/// back to its middle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Component)]
struct Checkpoint;

/// A kinematic platform that goes back and forth between two points, carrying a player on it.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Mover {
    /// One end, in the world.
    a: Vec2,
    /// The other.
    b: Vec2,
    /// Units a second along the way.
    #[reflect(min = 0.0, default = 2.5)]
    speed: f32,
    /// Whether it goes toward `b`; otherwise toward `a`.
    to_b: bool,
}

/// A coin: a sensor box that goes, and counts for the player, when a player touches it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Component)]
struct Coin;

/// The box that ends the level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Component)]
struct Goal;

/// What the keys and pads do, applied at the next tick. Recordings keep them.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
enum Command {
    /// Runs every player along `run`, from -1 to 1, until the next.
    Run { run: f32 },
    /// Asks every player to jump as soon as it stands.
    Jump,
    /// Lets go of jump: a rising player's jump is cut short.
    JumpReleased,
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
    /// The HUD, from `ui/hud.ui`.
    hud: Option<GameUi>,
    /// Whether a failed HUD update was logged, so it is logged once.
    hud_warned: bool,
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
        // What `gg actions` says each control does.
        let run = "Runs the player: -1 left, 1 right; 1 is `Player.speed` units a second, less \
                   what the ground's friction takes";
        actions.describe(Act::Run, run);
        let jump = format!(
            "Asks the player to jump, and the press stays wanted for {JUMP_BUFFER} ticks: the \
             player leaves the ground at `Player.jump_speed` units a second on the first tick it \
             stands, so a press just before landing counts. A press in the air jumps again, at \
             that speed, up to `Player.air_jumps` times before landing. Held, gravity pulls \
             `Player.float_gravity` times as hard while the player's vertical speed is under \
             `Player.float_band`; let go \
             while rising, and the speed is multiplied by `Player.cut`. Gravity is {} units a \
             second squared",
            -GRAVITY
        );
        actions.describe(Act::Jump, jump);
        Self {
            actions,
            commands: Vec::new(),
            hud: None,
            hud_warned: false,
        }
    }
}

impl Layer for Game {
    /// Registers what the start scene names.
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        cx.world_mut()
            .register_all::<(Player, Goal, Coin, Mover, Spikes, Checkpoint)>()?;
        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/hud.ui");
        self.hud = Some(cx.add_ui("hud", ui)?);
        Ok(())
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
        self.commands.push(Command::JumpReleased);
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for command in self.commands.drain(..) {
            for player in world.query_mut::<&mut Player>() {
                match command {
                    Command::Run { run } => player.run = run,
                    Command::Jump => {
                        player.jump_buffer = JUMP_BUFFER;
                        player.jump_down = true;
                    }
                    Command::JumpReleased => player.jump_down = false,
                }
            }
            if command == Command::JumpReleased {
                for (player, body) in world.query_mut::<(&Player, &mut RigidBody2D)>() {
                    if body.velocity.y > 0.0 {
                        body.velocity.y *= player.cut;
                    }
                }
            }
        }
        drive(world);
        step_physics(world);
        finish(world);
        follow(world);
    }

    /// Draws the HUD: the first player's coins.
    fn update(&mut self, cx: &mut Context, _: Duration) {
        let Some(hud) = self.hud else { return };
        let coins = cx.world().query::<&Player>().next().map(|p| p.coins);
        if let Some(coins) = coins {
            let set = hud.set_text_fmt(cx, "coins", format_args!("COINS {coins}"));
            if let Err(error) = set
                && !self.hud_warned
            {
                self.hud_warned = true;
                warn!(code = "platformer.hud", error = %error, "the HUD's coins text can't be set");
            }
        }
        hud.draw(cx);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    /// What `gg actions` lists, and `gg input` presses by name.
    fn actions(&self) -> Controls {
        self.actions.controls()
    }

    fn event(&mut self, _: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, so the next tick applies them.
        let commands = &mut self.commands;
        self.actions.event(event, |change| match change {
            Change::Axis(Act::Run, run) => commands.push(Command::Run { run }),
            Change::Pressed(Act::Jump) => commands.push(Command::Jump),
            Change::Released(Act::Jump) => commands.push(Command::JumpReleased),
            _ => {}
        });
        ControlFlow::Continue(())
    }
}

/// The body `player` stands on: one it touches whose top is at its feet or below.
fn grounded(world: &World, player: Entity) -> Option<Entity> {
    let feet = |world: &World, entity| {
        let at = world.get::<Transform>(entity)?;
        Some(at.translation.y - f64::from(at.scale.y) * 0.5)
    };
    let feet = feet(world, player)?;
    world.contacts().find_map(|contact| {
        let other = contact.other(player).filter(|_| !contact.sensor)?;
        let at = world.get::<Transform>(other)?;
        (at.translation.y + f64::from(at.scale.y) * 0.5 <= feet + FOOT_SLACK).then_some(other)
    })
}

/// Sets each mover's velocity toward the end it goes to, turning it round on arrival.
fn move_platforms(world: &mut World) {
    let dt = Tick::DT.as_secs_f32();
    for (mover, at, body) in world.query_mut::<(&mut Mover, &Transform, &mut RigidBody2D)>() {
        let target = if mover.to_b { mover.b } else { mover.a };
        let here = Vec2::new(at.translation.x as f32, at.translation.y as f32);
        let delta = target - here;
        if delta.length() <= mover.speed * dt {
            // The tick's step ends on the end exactly, so the next starts back from it.
            body.velocity = delta / dt;
            mover.to_b = !mover.to_b;
        } else {
            body.velocity = delta.normalize() * mover.speed;
        }
    }
}

/// Sets each player's velocity from its run and its wanted jump, before physics steps. A player
/// on a body that moves goes with it.
fn drive(world: &mut World) {
    move_platforms(world);
    // Entities are listed first: `grounded` reads the world the body writes.
    let mut players = [None; 4];
    let mut count = 0;
    for (_, entity) in world.query::<(&Player, Entity)>() {
        if count < players.len() {
            players[count] = Some(entity);
            count += 1;
        }
    }
    // What each stands on, and how fast along the level that goes.
    let mut ground = [(false, 0.0); 4];
    for (ground, entity) in ground.iter_mut().zip(players) {
        let on = entity.and_then(|entity| grounded(world, entity));
        let carry = on
            .and_then(|on| world.get::<RigidBody2D>(on))
            .map(|b| b.velocity.x);
        *ground = (on.is_some(), carry.unwrap_or(0.0));
    }
    for (player, body, entity) in world.query_mut::<(&mut Player, &mut RigidBody2D, Entity)>() {
        let (stands, carry) = players
            .iter()
            .zip(ground)
            .find_map(|(other, ground)| (*other == Some(entity)).then_some(ground))
            .unwrap_or_default();
        // A press of this tick alone may jump in the air: an older one waits for the ground.
        let fresh = player.jump_buffer == JUMP_BUFFER;
        let wanted = player.jump_buffer > 0;
        player.jump_buffer = player.jump_buffer.saturating_sub(1);
        body.velocity.x = player.run * player.speed + carry;
        if stands {
            player.air_jumps_left = player.air_jumps;
        }
        // Near the top, a held jump gives back part of what gravity takes this tick.
        if player.jump_down && !stands && body.velocity.y.abs() < player.float_band {
            body.velocity.y -= GRAVITY * (1.0 - player.float_gravity) * Tick::DT.as_secs_f32();
        }
        let in_the_air = wanted && fresh && !stands && player.air_jumps_left > 0;
        if wanted && stands || in_the_air {
            if in_the_air {
                player.air_jumps_left -= 1;
            }
            // A tap let go of before the launch is a short hop from the start.
            let hop = if player.jump_down { 1.0 } else { player.cut };
            body.velocity.y = player.jump_speed * hop;
            player.jump_buffer = 0;
        }
    }
}

/// Collects the coins players touch, makes checkpoints of those they reach, and respawns players
/// that fell out of the level, touched spikes or touched the goal. As with the goal, what one
/// player touches happens to every player.
fn finish(world: &mut World) {
    let mut reached = false;
    let mut spiked = false;
    let mut checkpoint = None;
    // The coins touched this tick, with the player that took each.
    let mut taken = [None; 8];
    let mut count = 0;
    for contact in world.contacts() {
        let [a, b] = contact.entities;
        let goal = |e| world.get::<Goal>(e).is_some();
        let player = |e| world.get::<Player>(e).is_some();
        reached |= (goal(a) && player(b)) || (goal(b) && player(a));
        for (hazard, by) in [(a, b), (b, a)] {
            if player(by) && world.get::<Spikes>(hazard).is_some() {
                spiked = true;
            }
            if player(by) && world.get::<Checkpoint>(hazard).is_some() {
                let at = world.get::<Transform>(hazard);
                checkpoint =
                    at.map(|at| Vec2::new(at.translation.x as f32, at.translation.y as f32));
            }
        }
        for (coin, by) in [(a, b), (b, a)] {
            let is_coin = world.get::<Coin>(coin).is_some();
            if is_coin && player(by) && count < taken.len() {
                taken[count] = Some((coin, by));
                count += 1;
            }
        }
    }
    for (coin, by) in taken.into_iter().flatten() {
        if world.despawn(coin).is_ok()
            && let Some(player) = world.get_mut::<Player>(by)
        {
            player.coins += 1;
            info!(coins = player.coins, "Coin collected");
        }
    }
    for (player, at, body) in world.query_mut::<(&mut Player, &mut Transform, &mut RigidBody2D)>() {
        if reached {
            player.levels_done += 1;
            player.checkpoint = player.start;
            info!(levels_done = player.levels_done, "Goal reached");
        } else if let Some(at) = checkpoint
            && at != player.checkpoint
        {
            player.checkpoint = at;
            info!(x = at.x, y = at.y, "Checkpoint reached");
        }
        let fell = at.translation.y < f64::from(player.fall_limit);
        if spiked {
            info!(x = at.translation.x, "Spiked");
        }
        if reached || spiked || fell {
            at.translation.x = f64::from(player.checkpoint.x);
            at.translation.y = f64::from(player.checkpoint.y);
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
