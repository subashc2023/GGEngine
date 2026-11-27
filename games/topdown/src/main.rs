//! topdown, a game made with GGEngine.
//!
//! A top-down twin-stick shooter. WASD, the arrows or a pad's left stick move the player, the mouse
//! or a pad's right stick aims, the left button or right trigger fires, and Shift or a pad's east
//! button dashes, once a second. Enemies come in waves from the room's edges, each wave bigger than
//! the last, and walk at the player: it takes three shots to bring one down, and one in four leaves
//! a pickup. A touch costs one of the player's three lives, a pickup gives one back, and at zero
//! the run starts over from wave 1. The HUD shows the wave, the score and the lives
//! (`ui/hud.ui`). The room and the player are in `scenes/main.ron`, which a development build
//! applies as it's saved.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use gg_engine::math::DVec3;
use gg_engine::prelude::*;
use gg_engine::{BodyType, BoxCollider2D, Camera, GameUi, Physics2D, RigidBody2D, Rng};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's debug overlays: what `main` runs, and what its recordings replay.
fn app() -> App {
    let app = App::new("topdown")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
    app.layer(Game::new()).debug_overlays()
}

/// Where the player starts, and goes back to when a run starts over.
const START: Vec2 = Vec2::new(0.0, 0.0);

/// The Enemy aim overlay's lines: linear RGBA.
const AIM: Vec4 = Vec4::new(1.0, 0.25, 0.2, 0.8);

/// An enemy's colour, and the white it flashes.
const ENEMY_COLOR: Vec4 = Vec4::new(0.95, 0.3, 0.25, 1.0);
const FLASH_COLOR: Vec4 = Vec4::new(1.0, 1.0, 1.0, 1.0);

/// Hits an enemy takes, the ticks it flashes after one, and the speed a hit knocks it back at,
/// which each tick keeps `KNOCK_KEPT` of.
const ENEMY_HITS: u32 = 3;
const FLASH_TICKS: u32 = 8;
const KNOCK_SPEED: f32 = 7.0;
const KNOCK_KEPT: f32 = 0.82;

/// A dash's speed in units a second and its length in ticks, and the ticks before the next.
const DASH_SPEED: f32 = 18.0;
const DASH_TICKS: u32 = 8;
const DASH_COOLDOWN: u32 = 64;

/// A wave's enemies: `WAVE_BASE` and `WAVE_STEP` for each wave number. They come in `SPAWN_EVERY`
/// ticks apart, the first `WAVE_PAUSE` ticks after the last wave ended.
const WAVE_BASE: u32 = 2;
const WAVE_STEP: u32 = 2;
const SPAWN_EVERY: u32 = 24;
const WAVE_PAUSE: u32 = 96;

/// An enemy's speed, and the points it's worth.
const ENEMY_SPEED: f32 = 1.4;
const ENEMY_POINTS: u32 = 10;

/// Where enemies come in from: the room's edges, in from its walls by half an enemy.
const EDGE: Vec2 = Vec2::new(7.3, 5.3);

/// The player's colour; the lives it starts with, and the most it has; the ticks a touch leaves it
/// unhurt (and blinking), and the speed a touch bumps its enemy away at.
const PLAYER_COLOR: Vec4 = Vec4::new(0.2, 0.6, 1.0, 1.0);
const LIVES: u32 = 3;
const HURT_TICKS: u32 = 96;
const BUMP_SPEED: f32 = 9.0;
/// The HUD's node for each life, which hides as it's lost.
const LIFE_NODES: [&str; LIVES as usize] = ["life1", "life2", "life3"];

/// The chance an enemy that dies leaves a pickup, and how near the player takes it.
const DROP_CHANCE: f32 = 0.25;
const PICKUP_REACH: f32 = 0.7;

/// The z the camera sees from.
const CAMERA_Z: f64 = 10.0;

/// A shot's speed in units a second, its lifetime in ticks (about 1.4 s), its size, and the ticks
/// between shots.
const SHOT_SPEED: f32 = 22.0;
const SHOT_TICKS: u32 = 90;
const SHOT_SIZE: f32 = 0.2;
const SHOT_EVERY: u32 = 8;

/// Collision groups, as the scene's colliders set them: the player is 1, which only enemies and
/// walls meet. A shot meets enemies and walls alone.
const PLAYER: u32 = 1;
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
    /// Ticks the dash has left.
    dash: u32,
    /// Which way the dash goes, of length 1 while it lasts.
    dash_heading: Vec2,
    /// Ticks until the next dash may start.
    dash_cooldown: u32,
    /// Ticks after a touch in which another costs nothing.
    hurt: u32,
}

/// Walks at the player.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Enemy {
    #[reflect(min = 0.0, default = 2.0)]
    speed: f32,
    /// Hits it has left.
    #[reflect(min = 0, default = 3)]
    health: u32,
    /// Ticks it still shows white, from its last hit.
    flash: u32,
    /// The speed its last hits pushed it at, added to its own and dying away.
    knock: Vec2,
}

/// The run's waves and score: what the HUD shows, kept in the world so a restart keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Run {
    /// The wave under way, 0 before the first.
    wave: u32,
    score: u32,
    /// Lives left: at zero the run starts over.
    lives: u32,
    /// Enemies of this wave that are still to come in.
    waiting: u32,
    /// Ticks until the next enemy comes in, or the next wave starts.
    timer: u32,
    /// Where the enemies come in.
    rng: Rng,
}

/// Gives the player back a life when it's touched.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Pickup {
    /// Lives it gives.
    #[reflect(min = 1, default = 1)]
    lives: u32,
}

/// A shot, with the ticks it has left.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Shot {
    ticks: u32,
    /// Its direction of travel, of length 1.
    heading: Vec2,
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
    /// Starts a dash along each player's heading, or its aim at rest, if its cooldown is over.
    Dash,
}

/// What the keys, mouse and pads are bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Steer,
    AimStick,
    Fire,
    Dash,
}

/// The game's layer.
struct Game {
    actions: Actions<Act>,
    commands: Vec<Command>,
    /// The HUD, from `ui/hud.ui`.
    hud: Option<GameUi>,
    /// Whether a failed HUD update was logged, so it is logged once.
    hud_warned: bool,
    /// The enemies that touched the player this tick, kept to reuse its memory.
    touched: Vec<Entity>,
    /// Where the enemies that died this tick were, kept to reuse its memory.
    drops: Vec<Vec2>,
    /// The tick's entities to despawn, kept to reuse its memory.
    dead: Vec<Entity>,
    /// The enemies shots hit this tick, and the knock each shot gives, kept to reuse its memory.
    struck: Vec<(Entity, Vec2)>,
    /// The player's place this tick.
    player_at: Vec<Vec2>,
    /// The enemies' places, for the Enemy aim overlay, kept to reuse its memory.
    enemies_at: Vec<Vec3>,
}

impl Game {
    fn new() -> Self {
        let mut actions = Actions::default();
        let steer = [Binding::WASD, Binding::ARROWS, Binding::stick(Stick::Left)];
        let fire = [
            Binding::Button(Source::Button(MouseButton::Left)),
            Binding::Button(Source::PadButton(PadButton::RightTrigger)),
        ];
        let dash = [
            Binding::Button(Source::Key(Key::ShiftLeft)),
            Binding::Button(Source::Key(Key::ShiftRight)),
            Binding::Button(Source::PadButton(PadButton::East)),
        ];
        let bound = actions.bind_all(
            steer
                .map(|binding| (Act::Steer, binding))
                .into_iter()
                .chain([(Act::AimStick, Binding::stick(Stick::Right))])
                .chain(fire.map(|binding| (Act::Fire, binding)))
                .chain(dash.map(|binding| (Act::Dash, binding))),
        );
        bound.expect("the bindings are of one kind each, of other sources");
        // What `gg actions` says each control does.
        let walk = "Walks the player: [x, y], right and up the screen; 1 is `Player.speed` units \
                    a second";
        actions.describe(Act::Steer, walk);
        let stick = "Aims the player along the right stick's [x, y], right and up, once the stick \
                     is pushed past 0.3; a weaker push keeps the aim";
        actions.describe(Act::AimStick, stick);
        // No numbers: a constant `new` reads rebuilds the game when it's edited, which a value
        // being tuned shouldn't.
        let shoot = "Held, fires shots along the aim, every `SHOT_EVERY` ticks, at `SHOT_SPEED` \
                     units a second. A shot takes a hit off the `Enemy` it hits, flashes and \
                     pushes it, and vanishes";
        actions.describe(Act::Fire, shoot);
        let dash = "Dashes the player in a short burst the way it's moving, along the aim at rest. \
                    It can dash again about a second after a dash starts";
        actions.describe(Act::Dash, dash);
        let cursor = "Aims the player at the point of the ground under the cursor, each time the \
                      cursor moves, until the right stick aims again";
        actions.describe_control(Control::Cursor, cursor);
        Self {
            actions,
            commands: Vec::new(),
            hud: None,
            hud_warned: false,
            touched: Vec::new(),
            drops: Vec::new(),
            dead: Vec::new(),
            struck: Vec::new(),
            player_at: Vec::new(),
            enemies_at: Vec::new(),
        }
    }
}

impl Layer for Game {
    /// Registers the game's components, which the start scene names.
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        cx.world_mut()
            .register_all::<(Player, Enemy, Shot, Run, Pickup)>()?;
        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/hud.ui");
        self.hud = Some(cx.add_ui("hud", ui)?);
        Ok(())
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
                    Command::Dash => {
                        if player.dash_cooldown == 0 {
                            let moving = player.heading.length() > 0.1;
                            let way = if moving { player.heading } else { player.aim };
                            player.dash_heading = way.normalize_or_zero();
                            player.dash = DASH_TICKS;
                            player.dash_cooldown = DASH_COOLDOWN;
                        }
                    }
                }
            }
        }
        self.player_at.clear();
        let players = world.query_mut::<(&mut Player, &mut RigidBody2D, &Transform, &mut Sprite)>();
        for (player, body, at, sprite) in players {
            player.dash_cooldown = player.dash_cooldown.saturating_sub(1);
            // Blinks while a touch can't cost another life.
            let blink = player.hurt > 0 && player.hurt / 8 % 2 == 0;
            sprite.color = PLAYER_COLOR * Vec4::new(1.0, 1.0, 1.0, if blink { 0.3 } else { 1.0 });
            player.hurt = player.hurt.saturating_sub(1);
            body.velocity = if player.dash > 0 {
                player.dash -= 1;
                player.dash_heading * DASH_SPEED
            } else {
                player.heading * player.speed
            };
            self.player_at.push(at.translation.truncate().as_vec2());
        }
        let target = self.player_at.first().copied();
        let enemies = world.query_mut::<(&mut Enemy, &mut RigidBody2D, &Transform, &mut Sprite)>();
        for (enemy, body, at, sprite) in enemies {
            let toward = target.map_or(Vec2::ZERO, |target| {
                (target - at.translation.truncate().as_vec2()).normalize_or_zero()
            });
            body.velocity = toward * enemy.speed + enemy.knock;
            enemy.knock *= KNOCK_KEPT;
            sprite.color = if enemy.flash > 0 {
                FLASH_COLOR
            } else {
                ENEMY_COLOR
            };
            enemy.flash = enemy.flash.saturating_sub(1);
        }
        if let Some(target) = target {
            for (camera, at) in world.query_mut::<(&Camera, &mut Transform)>() {
                if camera.primary {
                    at.translation = target.extend(0.0).as_dvec3() + DVec3::Z * CAMERA_Z;
                }
            }
        }
        self.waves(world);
        self.shoot(world);
        step_physics(world);
        self.hit(world);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    /// What `gg actions` lists, and `gg input` presses by name.
    fn actions(&self) -> Controls {
        self.actions.controls()
    }

    /// Draws the game's own debug overlay, Enemy aim, while it's on: a line from each enemy to the
    /// player it walks at. The overlays' bar lists it under Game.
    fn update(&mut self, cx: &mut Context, _: Duration) {
        self.draw_hud(cx);
        if !cx.debug_overlay("Enemy aim") {
            return;
        }
        let player = cx.world().query::<(&Player, &Transform)>().next();
        let Some(player) = player.map(|(_, at)| at.translation.as_vec3()) else {
            return;
        };
        self.enemies_at.clear();
        let enemies = cx.world().query::<(&Enemy, &Transform)>();
        (self.enemies_at).extend(enemies.map(|(_, at)| at.translation.as_vec3()));
        for &enemy in &self.enemies_at {
            cx.draw_line(enemy, player, AIM, 2.0);
        }
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
            Change::Pressed(Act::Dash) => commands.push(Command::Dash),
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
    /// Sets the HUD's texts from the run and draws it.
    fn draw_hud(&mut self, cx: &mut Context) {
        let Some(hud) = self.hud else { return };
        let run = cx.world().query::<&Run>().next().copied();
        if let Some(run) = run {
            let wave = hud.set_text_fmt(cx, "wave", format_args!("WAVE {}", run.wave.max(1)));
            let score = hud.set_text_fmt(cx, "score", format_args!("SCORE {}", run.score));
            let mut failed = wave.err().or(score.err());
            for life in 1..=LIVES {
                let shown = hud.set_visible(cx, LIFE_NODES[life as usize - 1], life <= run.lives);
                failed = failed.or(shown.err());
            }
            if let Some(error) = failed
                && !self.hud_warned
            {
                self.hud_warned = true;
                warn!(code = "topdown.hud", error = %error, "the HUD's values can't be set");
            }
        }
        hud.draw(cx);
    }

    /// Brings the waves in: an enemy at a time from a random point of the room's edge, and the next
    /// wave, a bigger one, once the room is clear.
    fn waves(&mut self, world: &mut World) {
        let clear = world.query::<&Enemy>().next().is_none();
        let mut arrival = None;
        for run in world.query_mut::<&mut Run>() {
            run.timer = run.timer.saturating_sub(1);
            if run.timer > 0 {
                continue;
            }
            if run.waiting > 0 {
                run.waiting -= 1;
                run.timer = SPAWN_EVERY;
                let along = run.rng.range(-1.0, 1.0);
                let across = if run.rng.chance(0.5) { 1.0 } else { -1.0 };
                arrival = Some(if run.rng.chance(0.5) {
                    Vec2::new(along * EDGE.x, across * EDGE.y)
                } else {
                    Vec2::new(across * EDGE.x, along * EDGE.y)
                });
            } else if clear {
                run.wave += 1;
                run.waiting = WAVE_BASE + WAVE_STEP * run.wave;
                run.timer = WAVE_PAUSE;
            }
        }
        if let Some(from) = arrival {
            let at = Transform {
                translation: from.extend(0.3).as_dvec3(),
                scale: Vec3::new(0.8, 0.8, 1.0),
                ..Transform::IDENTITY
            };
            let sprite = Sprite {
                color: ENEMY_COLOR,
                ..Sprite::default()
            };
            let body = RigidBody2D {
                body_type: BodyType::Dynamic,
                fixed_rotation: true,
                ..RigidBody2D::default()
            };
            let collider = BoxCollider2D {
                friction: 0.0,
                membership: ENEMY,
                filter: PLAYER | ENEMY | WALL | SHOT,
                ..BoxCollider2D::default()
            };
            let enemy = Enemy {
                speed: ENEMY_SPEED,
                health: ENEMY_HITS,
                flash: 0,
                knock: Vec2::ZERO,
            };
            world.spawn((at, sprite, body, collider, enemy));
        }
    }

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
            world.spawn((
                at,
                sprite,
                body,
                collider,
                Shot {
                    ticks: SHOT_TICKS,
                    heading: aim,
                },
            ));
        }
    }

    /// Costs the player a life for the enemies that touched it, unless a touch is still recent,
    /// and starts the run over at the last. Each enemy that touched is pushed away from the player.
    fn touch(&mut self, world: &mut World) {
        let mut hurt = None;
        for (player, at) in world.query_mut::<(&mut Player, &Transform)>() {
            if player.hurt == 0 && !self.touched.is_empty() {
                player.hurt = HURT_TICKS;
                hurt = Some(at.translation.truncate().as_vec2());
            }
        }
        let Some(player_at) = hurt else {
            return;
        };
        for entity in self.touched.drain(..) {
            let at = world.get::<Transform>(entity).map(|at| at.translation);
            if let (Some(at), Some(enemy)) = (at, world.get_mut::<Enemy>(entity)) {
                let away = at.truncate().as_vec2() - player_at;
                enemy.knock += away.normalize_or_zero() * BUMP_SPEED;
            }
        }
        let mut over = false;
        for run in world.query_mut::<&mut Run>() {
            run.lives = run.lives.saturating_sub(1);
            over |= run.lives == 0;
        }
        if over {
            self.restart(world);
        }
    }

    /// Lets the player pick up what it stands on, one life each, while it has less than it started
    /// with.
    fn collect(&mut self, world: &mut World) {
        let Some(player) = self.player_at.first().copied() else {
            return;
        };
        self.dead.clear();
        let mut lives = 0;
        for run in world.query_mut::<&mut Run>() {
            lives = run.lives;
        }
        let mut gained = 0;
        for (entity, pickup, at) in world.query_mut::<(Entity, &Pickup, &Transform)>() {
            let near = at.translation.truncate().as_vec2().distance(player) < PICKUP_REACH;
            if near && lives + gained + pickup.lives <= LIVES {
                gained += pickup.lives;
                self.dead.push(entity);
            }
        }
        for entity in self.dead.drain(..) {
            let _ = world.despawn(entity);
        }
        for run in world.query_mut::<&mut Run>() {
            run.lives += gained;
        }
    }

    /// Starts the run over from wave 1: the room is emptied, the player is back at the start, with
    /// all its lives, and the score is nothing.
    fn restart(&mut self, world: &mut World) {
        self.dead.clear();
        self.dead
            .extend(world.query::<(Entity, &Enemy)>().map(|(entity, _)| entity));
        self.dead
            .extend(world.query::<(Entity, &Shot)>().map(|(entity, _)| entity));
        self.dead
            .extend(world.query::<(Entity, &Pickup)>().map(|(entity, _)| entity));
        for entity in self.dead.drain(..) {
            let _ = world.despawn(entity);
        }
        for run in world.query_mut::<&mut Run>() {
            let rng = run.rng;
            *run = Run {
                wave: 0,
                score: 0,
                lives: LIVES,
                waiting: 0,
                timer: WAVE_PAUSE,
                rng,
            };
        }
        for (player, at, body) in
            world.query_mut::<(&mut Player, &mut Transform, &mut RigidBody2D)>()
        {
            at.translation = START.extend(0.3).as_dvec3();
            body.velocity = Vec2::ZERO;
            player.dash = 0;
            player.hurt = 0;
        }
    }

    /// Removes shots that hit or ran out, takes hits off the enemies they hit, and costs a touched
    /// player a life.
    fn hit(&mut self, world: &mut World) {
        self.dead.clear();
        self.touched.clear();
        for contact in world.contacts() {
            let [a, b] = contact.entities;
            for (one, other) in [(a, b), (b, a)] {
                if world.get::<Shot>(one).is_some() {
                    self.dead.push(one);
                    if world.get::<Enemy>(other).is_some() {
                        let heading = world.get::<Shot>(one).map_or(Vec2::ZERO, |s| s.heading);
                        self.struck.push((other, heading * KNOCK_SPEED));
                    }
                } else if world.get::<Player>(one).is_some() && world.get::<Enemy>(other).is_some()
                {
                    self.touched.push(other);
                }
            }
        }
        let mut points = 0;
        for (entity, push) in self.struck.drain(..) {
            let at = world.get::<Transform>(entity).map(|at| at.translation);
            let Some(enemy) = world.get_mut::<Enemy>(entity) else {
                continue;
            };
            // A second shot on a dying enemy is spent but counts nothing.
            if enemy.health == 0 {
                continue;
            }
            enemy.health = enemy.health.saturating_sub(1);
            enemy.flash = FLASH_TICKS;
            enemy.knock += push;
            if enemy.health == 0 {
                self.dead.push(entity);
                points += ENEMY_POINTS;
                if let Some(at) = at {
                    self.drops.push(at.truncate().as_vec2());
                }
            }
        }
        // Without a run, there is no rng to roll the chance: no drops.
        if world.query::<&Run>().next().is_none() {
            self.drops.clear();
        }
        for run in world.query_mut::<&mut Run>() {
            run.score += points;
            // One enemy in four leaves a pickup where it died.
            let rng = &mut run.rng;
            self.drops.retain(|_| rng.chance(DROP_CHANCE));
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
        for at in self.drops.drain(..) {
            spawn_pickup(world, at);
        }
        self.touch(world);
        self.collect(world);
    }
}

/// Leaves a pickup at `at`.
fn spawn_pickup(world: &mut World, at: Vec2) {
    let transform = Transform {
        translation: at.extend(0.2).as_dvec3(),
        scale: Vec3::new(0.45, 0.45, 1.0),
        ..Transform::IDENTITY
    };
    let sprite = Sprite {
        color: Vec4::new(0.3, 1.0, 0.45, 1.0),
        ..Sprite::default()
    };
    world.spawn((transform, sprite, Pickup { lives: 1 }));
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
