//! fps, a game made with GGEngine.
//!
//! A first-person shooter's skeleton, at dusk. Its start scene, `scenes/main.ron`, holds a walled
//! arena, a few crates, targets that glow, two of them sliding along the north wall, a low sun and
//! a haze, a camera and the player, a capsule that a character controller moves. WASD walks, Shift
//! sprints, Space jumps, the mouse looks about while the cursor is grabbed, a click grabs it and
//! Escape lets it go. A click while it's grabbed fires a ray from the eye: three hits sink a
//! target, each throwing sparks and flashing it. The gun holds 12 rounds, and R reloads in a second
//! (an empty gun's click reloads too). A round's clock runs from its first shot until the last
//! target is down, and `Player.best` keeps the fastest; N stands the targets up for a new round.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::fmt::Write as _;
use std::ops::ControlFlow;
use std::process::ExitCode;

use gg_engine::math::DVec3;
use gg_engine::prelude::*;
use gg_engine::{
    Camera, Cast, CharacterController3D, EmitterShape, Hit3D, ParticleBlend, ParticleEmitter,
    RigidBody3D, Tint,
};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's debug overlays: what `main` runs, and what its recordings replay.
fn app() -> App {
    let app = App::new("fps")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
    app.layer(Game::new()).debug_overlays()
}

/// The furthest the player looks up or down: 89°, short of straight, where yaw would spin.
const MAX_PITCH: f32 = 1.553_343;

/// How far a shot reaches, in units.
const RANGE: f32 = 200.0;

/// The player: walks where it looks, as its character controller moves it, and fires.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Player {
    /// Walking speed, in units a second.
    #[reflect(min = 0.0, default = 8.0)]
    speed: f32,
    /// What `speed` is multiplied by while sprinting.
    #[reflect(min = 1.0, default = 1.6)]
    sprint: f32,
    /// How fast a jump leaves the ground, in units a second.
    #[reflect(min = 0.0, default = 6.5)]
    jump_speed: f32,
    /// How far a unit of the mouse's motion turns the view, in radians.
    #[reflect(min = 0.0, default = 0.0025)]
    sensitivity: f32,
    /// The eye's height above the capsule's center.
    #[reflect(default = 0.7)]
    eye: f32,
    /// As the last move set it, of length at most 1: across, then forward.
    heading: Vec2,
    /// Whether Shift is held, as the last sprint command set it.
    sprinting: bool,
    /// The turn to its left about up, in radians, from facing -Z.
    yaw: f32,
    /// The turn up, in radians, no further than [`MAX_PITCH`] either way.
    #[reflect(min = -1.553_343, max = 1.553_343)]
    pitch: f32,
    /// Rounds in the gun.
    #[reflect(default = 12)]
    ammo: u32,
    /// Rounds a full magazine holds.
    #[reflect(min = 1, default = 12)]
    magazine: u32,
    /// Seconds of the reload left, 0 when none is under way.
    #[reflect(min = 0.0)]
    reload: f32,
    /// How long a reload takes, in seconds.
    #[reflect(min = 0.0, default = 1.0)]
    reload_time: f32,
    /// Seconds since the round's first shot, while it runs and after it ended.
    #[reflect(min = 0.0)]
    clock: f32,
    /// Whether the round's clock runs: from its first shot until its last target is down.
    running: bool,
    /// Whether every target is down, the clock stopped at the round's time.
    finished: bool,
    /// The fastest round so far, in seconds: 0 for none yet.
    #[reflect(min = 0.0)]
    best: f32,
    /// How many shots it has fired.
    shots: u32,
    /// How many of them hit a target.
    hits: u32,
}

impl Player {
    /// Which way it faces, as a turn from facing -Z.
    fn look(&self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch)
    }
}

/// How long a target flashes after a hit, in seconds.
const FLASH: f32 = 0.18;

/// How long a burst of sparks lives before its entity goes, in seconds: longer than its sparks.
const SPARKS_LIFE: f32 = 1.0;

/// A thing shots sink below the floor, after `health` hits.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Target {
    /// What a hit on it is worth.
    #[reflect(default = 1)]
    points: u32,
    /// How many hits it takes standing: what a new round gives back to `health`.
    #[reflect(min = 1, default = 3)]
    hits: u32,
    /// How many hits it has left: 0 while it's down, as the hit that takes the last sinks it.
    #[reflect(min = 0, default = 3)]
    health: u32,
    /// How many seconds of its flash are left: white as a hit leaves it, then back to `base`.
    #[reflect(min = 0.0)]
    flash: f32,
    /// Its tint before the flash, which the first hit of a flash records.
    #[reflect(color, default = Vec4::ONE)]
    base: Vec4,
}

/// How far a target that's down sinks below the floor, in units, until the next round.
const DROP: f64 = 100.0;

/// A target that slides back and forth along a line, on a kinematic body whose velocity this sets.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Slide {
    /// The middle of its line.
    center: Vec3,
    /// Which way the line runs, of length 1.
    #[reflect(default = Vec3::X)]
    axis: Vec3,
    /// How far either way from `center` it goes, in units.
    #[reflect(min = 0.0, default = 8.0)]
    range: f32,
    /// How fast it goes, in units a second.
    #[reflect(min = 0.0, default = 3.0)]
    speed: f32,
    /// 1 along `axis`, -1 against it.
    #[reflect(default = 1.0)]
    dir: f32,
}

/// A burst of sparks, whose entity goes when `left` seconds have passed.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Sparks {
    /// Seconds to go.
    #[reflect(min = 0.0, default = 1.0)]
    left: f32,
}

/// What the keys and the mouse do, applied at the next tick. Recordings keep them.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Walks the player along `heading`, across then forward, until the next.
    Move { heading: Vec2 },
    /// Turns the player's view by the mouse's motion, right and down.
    Look { by: Vec2 },
    /// Jumps, if the player stands.
    Jump,
    /// Starts or stops the sprint, which lasts until the next.
    Sprint { on: bool },
    /// Stands the targets back up and clears the clock, for a new round.
    NewRound,
    /// Starts a reload, if the magazine isn't full and none is under way.
    Reload,
    /// Fires a ray from the eye along the view.
    Fire,
}

/// What the keys and pads are bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Move,
    Jump,
    Sprint,
    Reload,
    NewRound,
}

/// The game's layer.
struct Game {
    actions: Actions<Act>,
    commands: Vec<Command>,
    /// Whether the mouse looks: from a click until Escape.
    grab: bool,
    /// The HUD's texts, written in place each frame, so frames allocate nothing once it's grown.
    line: String,
}

impl Game {
    fn new() -> Self {
        let mut actions = Actions::default();
        let bound = actions.bind_all([
            (Act::Move, Binding::WASD),
            (Act::Move, Binding::ARROWS),
            (Act::Move, Binding::stick(Stick::Left)),
            (Act::Jump, Binding::Button(Source::Key(Key::Space))),
            (
                Act::Jump,
                Binding::Button(Source::PadButton(PadButton::South)),
            ),
            (Act::Sprint, Binding::Button(Source::Key(Key::ShiftLeft))),
            (Act::Sprint, Binding::Button(Source::Key(Key::ShiftRight))),
            (
                Act::Sprint,
                Binding::Button(Source::PadButton(PadButton::LeftThumb)),
            ),
            (Act::NewRound, Binding::Button(Source::Key(Key::KeyN))),
            (
                Act::NewRound,
                Binding::Button(Source::PadButton(PadButton::North)),
            ),
            (Act::Reload, Binding::Button(Source::Key(Key::KeyR))),
            (
                Act::Reload,
                Binding::Button(Source::PadButton(PadButton::West)),
            ),
        ]);
        bound.expect("the bindings are each of one kind, of other sources");
        // What `gg actions` says each control does.
        let walk = "Walks the player on the level: [x, y], right and forward of where it faces by \
                    `Player.yaw` alone; 1 is `Player.speed` units a second";
        actions.describe(Act::Move, walk);
        let jump = "Jumps if the player stands, leaving the ground at `Player.jump_speed` units a \
                    second; a press in the air does nothing. Gravity is 9.81 units a second \
                    squared, times `CharacterController3D.gravity`";
        actions.describe(Act::Jump, jump);
        let sprint = "Sprints while held: walking is `Player.sprint` times as fast";
        actions.describe(Act::Sprint, sprint);
        let reload = "Reloads the gun, which takes `Player.reload_time` seconds, after which it holds \
                      `Player.magazine` rounds; it does nothing with a full magazine or while \
                      reloading. An empty gun's click reloads too";
        actions.describe(Act::Reload, reload);
        let round = "Starts a new round: every target stands up again, its `Target.health` back \
                     at its `Target.hits`, the gun is full and the clock is cleared. `Player.best` \
                     is kept";
        actions.describe(Act::NewRound, round);
        let look = "Turns the view while the cursor is grabbed, `Player.sensitivity` radians a \
                    unit: right turns right, lowering `Player.yaw`, and down looks down, lowering \
                    `Player.pitch`. The pitch stops at 89° either way";
        actions.describe_control(Control::Motion, look);
        let fire = format!(
            "A press grabs the cursor; once it's grabbed, each fires along the view from the eye, \
             hitting the `Target` it meets first, up to {RANGE} units: sparks fly where it lands \
             and the target flashes, and the hit that takes its last `Target.health` sinks it. \
             Each shot counts in `Player.shots`, and each hit in `Player.hits`"
        );
        let click = Control::Source(Source::Button(MouseButton::Left));
        actions.describe_control(click, fire);
        let escape = Control::Source(Source::Key(Key::Escape));
        actions.describe_control(escape, "Lets go of the cursor, so the mouse stops looking");
        Self {
            actions,
            commands: Vec::new(),
            grab: false,
            line: String::new(),
        }
    }
}

/// Applies `command` to `world`.
fn apply(world: &mut World, command: &Command) {
    match *command {
        Command::Move { heading } => {
            for player in world.query_mut::<&mut Player>() {
                player.heading = heading;
            }
        }
        Command::Look { by } => {
            for player in world.query_mut::<&mut Player>() {
                // The mouse's right turns right, which is less yaw, and down looks down.
                player.yaw -= by.x * player.sensitivity;
                player.pitch =
                    (player.pitch - by.y * player.sensitivity).clamp(-MAX_PITCH, MAX_PITCH);
            }
        }
        Command::Sprint { on } => {
            for player in world.query_mut::<&mut Player>() {
                player.sprinting = on;
            }
        }
        Command::Jump => {
            for (player, controller) in world.query_mut::<(&Player, &mut CharacterController3D)>() {
                if controller.grounded {
                    controller.velocity.y = player.jump_speed;
                }
            }
        }
        Command::NewRound => new_round(world),
        Command::Reload => {
            for player in world.query_mut::<&mut Player>() {
                start_reload(player);
            }
        }
        Command::Fire => fire(world),
    }
}

/// Stands every target back up, its health back at its `hits`, fills the gun and clears the round's
/// clock.
fn new_round(world: &mut World) {
    for (target, at) in world.query_mut::<(&mut Target, &mut Transform)>() {
        if target.health == 0 {
            at.translation.y += DROP;
        }
        target.health = target.hits;
    }
    for player in world.query_mut::<&mut Player>() {
        player.ammo = player.magazine;
        player.reload = 0.0;
        player.clock = 0.0;
        player.running = false;
        player.finished = false;
    }
}

/// Starts a reload of `player`'s gun, unless it's full or already reloading.
fn start_reload(player: &mut Player) {
    if player.reload <= 0.0 && player.ammo < player.magazine {
        player.reload = player.reload_time.max(Tick::DT.as_secs_f32());
    }
}

/// Casts a ray from the player's eye along its view, and hits the target it meets first.
fn fire(world: &mut World) {
    let shot = world
        .query_mut::<(Entity, &mut Player, &Transform)>()
        .find_map(|(entity, player, at)| {
            // Reloading, or empty, it doesn't fire: an empty gun reloads.
            if player.reload > 0.0 {
                return None;
            }
            if player.ammo == 0 {
                start_reload(player);
                return None;
            }
            player.ammo -= 1;
            // The first shot of a round starts its clock.
            if !player.running && !player.finished {
                player.running = true;
                player.clock = 0.0;
            }
            let eye = at.translation + DVec3::Y * f64::from(player.eye);
            let cast = Cast {
                from: eye,
                toward: player.look() * Vec3::NEG_Z,
                max_distance: RANGE,
                exclude: Some(entity),
            };
            Some((entity, cast))
        });
    let Some((shooter, cast)) = shot else {
        return;
    };
    let hit = world.cast_ray(cast);
    let target = hit.filter(|hit| world.get::<Target>(hit.entity).is_some());
    if let Some(hit) = target {
        hit_target(world, hit);
    }
    if let Some(player) = world.get_mut::<Player>(shooter) {
        player.shots = player.shots.saturating_add(1);
        player.hits = player.hits.saturating_add(u32::from(target.is_some()));
    }
}

/// Takes a hit's toll of its target: a flash, sparks where it landed, and the target itself on
/// the last.
fn hit_target(world: &mut World, hit: Hit3D) {
    let tint = world
        .get::<Tint>(hit.entity)
        .map_or(Vec4::ONE, |tint| tint.0);
    let Some(target) = world.get_mut::<Target>(hit.entity) else {
        return;
    };
    // A hit in a flash leaves the tint it hid as it was.
    if target.flash <= 0.0 {
        target.base = tint;
    }
    target.flash = FLASH;
    target.health = target.health.saturating_sub(1);
    let (points, health) = (target.points, target.health);
    spawn_sparks(world, hit);
    if health == 0 {
        // It sinks below the floor, where no shot reaches, until the next round.
        if let Some(at) = world.get_mut::<Transform>(hit.entity) {
            at.translation.y -= DROP;
        }
        info!(points, "Target down");
    } else {
        info!(points, health, "Target hit");
    }
}

/// Bursts sparks out of the surface a shot hit, along its normal.
fn spawn_sparks(world: &mut World, hit: Hit3D) {
    let at = Transform {
        translation: hit.point + (hit.normal * 0.02).as_dvec3(),
        rotation: Quat::from_rotation_arc(Vec3::Y, hit.normal),
        ..Transform::IDENTITY
    };
    let emitter = ParticleEmitter {
        rate: 0.0,
        burst: 24,
        bursts: 1,
        lifetime: [0.15, 0.4],
        speed: [3.0, 7.0],
        shape: EmitterShape::Cone {
            angle: 0.9,
            radius: 0.02,
        },
        acceleration: Vec3::new(0.0, -9.81, 0.0),
        drag: 0.5,
        size: [0.35, 0.06],
        color: [
            Vec4::new(6.0, 3.0, 0.8, 1.0),
            Vec4::new(2.0, 0.4, 0.05, 0.0),
        ],
        blend: ParticleBlend::Additive,
        ..ParticleEmitter::default()
    };
    world.spawn((at, emitter, Sparks { left: SPARKS_LIFE }));
}

/// Runs the round's clock, and stops it as the last target goes down.
fn run_clock(world: &mut World) {
    let up = world.query::<&Target>().any(|target| target.health > 0);
    for player in world.query_mut::<&mut Player>() {
        if !player.running {
            continue;
        }
        player.clock += Tick::DT.as_secs_f32();
        if !up {
            player.running = false;
            player.finished = true;
            if player.best <= 0.0 || player.clock < player.best {
                player.best = player.clock;
            }
            info!(seconds = player.clock, best = player.best, "Round done");
        }
    }
}

/// Slides each sliding target along its line, turning it back at either end.
fn slide(world: &mut World) {
    for (slide, body, at) in world.query_mut::<(&mut Slide, &mut RigidBody3D, &Transform)>() {
        let offset = (at.translation.as_vec3() - slide.center).dot(slide.axis);
        if offset >= slide.range {
            slide.dir = -1.0;
        } else if offset <= -slide.range {
            slide.dir = 1.0;
        }
        body.velocity = slide.axis * slide.speed * slide.dir;
    }
}

/// Runs the flash of each target down, and removes the sparks that have burnt out.
fn animate(world: &mut World) {
    let dt = Tick::DT.as_secs_f32();
    let white = Vec4::new(4.0, 4.0, 4.0, 1.0);
    for player in world.query_mut::<&mut Player>() {
        if player.reload > 0.0 {
            player.reload = (player.reload - dt).max(0.0);
            if player.reload == 0.0 {
                player.ammo = player.magazine;
            }
        }
    }
    for (target, tint) in world.query_mut::<(&mut Target, &mut Tint)>() {
        if target.flash > 0.0 {
            target.flash = (target.flash - dt).max(0.0);
            tint.0 = target.base.lerp(white, target.flash / FLASH);
        }
    }
    let mut gone = Vec::new();
    for (entity, sparks) in world.query_mut::<(Entity, &mut Sparks)>() {
        sparks.left -= dt;
        if sparks.left <= 0.0 {
            gone.push(entity);
        }
    }
    for entity in gone {
        // Fails only for an entity gone already, as a scene change this tick takes every one.
        let _ = world.despawn(entity);
    }
}

/// Writes `seconds` to `out` as a clock reads, to the hundredth: `0:03.42`, `1:00.00`.
fn write_clock(out: &mut String, seconds: f32) {
    // Rounded whole, so 59.996 reads `1:00.00`, not `0:60.00`.
    let hundredths = (seconds.max(0.0) * 100.0).round() as u64;
    let (minutes, seconds) = (hundredths / 6000, hundredths / 100 % 60);
    let _ = write!(out, "{minutes}:{seconds:02}.{:02}", hundredths % 100);
}

impl Layer for Game {
    /// Registers the components the start scene names.
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        cx.world_mut()
            .register_all::<(Player, Target, Slide, Sparks)>()
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
        self.commands.push(Command::Move { heading });
        self.commands.push(Command::Sprint { on: false });
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for command in self.commands.drain(..) {
            apply(world, &command);
        }
        slide(world);
        // Walks where it looks, keeping the fall or jump the controller has.
        for (player, controller) in world.query_mut::<(&Player, &mut CharacterController3D)>() {
            let turn = Quat::from_rotation_y(player.yaw);
            let pace = match player.sprinting {
                true => player.speed * player.sprint,
                false => player.speed,
            };
            let across = turn * Vec3::new(player.heading.x, 0.0, -player.heading.y) * pace;
            controller.velocity = Vec3::new(across.x, controller.velocity.y, across.z);
        }
        step_physics(world);
        animate(world);
        run_clock(world);
        // The camera sits at the eye, as the step left the player.
        let eye = world
            .query::<(&Player, &Transform)>()
            .next()
            .map(|(player, at)| {
                let height = DVec3::Y * f64::from(player.eye);
                (at.translation + height, player.look())
            });
        if let Some((translation, rotation)) = eye {
            for (_, at) in world.query_mut::<(&Camera, &mut Transform)>() {
                at.translation = translation;
                at.rotation = rotation;
            }
        }
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    /// What `gg actions` lists, and `gg input` presses by name.
    fn actions(&self) -> Controls {
        self.actions.controls()
    }

    /// Grabs the cursor while the mouse looks, and draws the HUD.
    fn update(&mut self, cx: &mut Context, _: std::time::Duration) {
        cx.set_cursor_grabbed(self.grab);
        let player = cx.world().query::<&Player>().next().copied();
        let mut canvas = cx.canvas([Align::Center; 2]);
        let sprite = Sprite {
            color: Vec4::new(1.0, 1.0, 1.0, 0.85),
            ..Sprite::default()
        };
        // Two bars with a gap between, which a reload dims.
        let sprite = match player {
            Some(player) if player.reload > 0.0 => Sprite {
                color: Vec4::new(1.0, 0.45, 0.25, 0.6),
                ..sprite
            },
            _ => sprite,
        };
        for size in [Vec3::new(26.0, 3.0, 1.0), Vec3::new(3.0, 26.0, 1.0)] {
            canvas.draw_sprite(Mat4::from_scale(size), sprite);
        }
        let Some(player) = player else {
            return;
        };
        let text = |size: f32, color: Vec4, align: [Align; 2]| TextStyle {
            font: None,
            size,
            color,
            align,
        };
        if player.reload > 0.0 {
            let style = text(34.0, Vec4::new(1.0, 0.6, 0.3, 1.0), [Align::Center; 2]);
            let at = Mat4::from_translation(Vec3::new(0.0, -64.0, 0.0));
            canvas.draw_text(at, "Reloading", &style);
        }
        let mut top = cx.canvas([Align::Center, Align::Start]);
        let color = match player.finished {
            true => Vec4::new(0.5, 1.0, 0.5, 1.0),
            false => Vec4::ONE,
        };
        let style = text(64.0, color, [Align::Center; 2]);
        let at = Mat4::from_translation(Vec3::new(0.0, -70.0, 0.0));
        let line = &mut self.line;
        line.clear();
        write_clock(line, player.clock);
        top.draw_text(at, line, &style);
        line.clear();
        match player.best > 0.0 {
            true => {
                line.push_str("Best ");
                write_clock(line, player.best);
            }
            false => line.push_str("Best --"),
        }
        let style = text(30.0, Vec4::new(1.0, 0.85, 0.5, 1.0), [Align::Center; 2]);
        let at = Mat4::from_translation(Vec3::new(0.0, -128.0, 0.0));
        top.draw_text(at, line, &style);
        if player.finished {
            let style = text(26.0, Vec4::new(1.0, 1.0, 1.0, 0.8), [Align::Center; 2]);
            let at = Mat4::from_translation(Vec3::new(0.0, -170.0, 0.0));
            top.draw_text(at, "All down: N for a new round", &style);
        }
        let mut corner = cx.canvas([Align::End, Align::End]);
        let low = player.ammo == 0 || player.reload > 0.0;
        let color = match low {
            true => Vec4::new(1.0, 0.35, 0.2, 1.0),
            false => Vec4::new(1.0, 1.0, 1.0, 1.0),
        };
        let style = text(56.0, color, [Align::End, Align::End]);
        let at = Mat4::from_translation(Vec3::new(-48.0, 44.0, 0.0));
        line.clear();
        let _ = write!(line, "{} / {}", player.ammo, player.magazine);
        corner.draw_text(at, line, &style);
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, so the next tick applies them.
        let commands = &mut self.commands;
        self.actions.event(event, |change| match change {
            Change::Axis2(Act::Move, heading) => commands.push(Command::Move { heading }),
            Change::Pressed(Act::Jump) => commands.push(Command::Jump),
            Change::Pressed(Act::NewRound) => commands.push(Command::NewRound),
            Change::Pressed(Act::Reload) => commands.push(Command::Reload),
            Change::Pressed(Act::Sprint) => commands.push(Command::Sprint { on: true }),
            Change::Released(Act::Sprint) => commands.push(Command::Sprint { on: false }),
            _ => {}
        });
        match *event {
            Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            } if !cx.pointer_over_ui() => {
                // The click that grabs the cursor doesn't fire.
                if self.grab {
                    commands.push(Command::Fire);
                }
                self.grab = true;
            }
            Event::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => self.grab = false,
            // Each tick turns the view once, by the motion since the tick before.
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

#[cfg(test)]
mod tests {
    use gg_engine::{BodyType, BoxCollider3D, CapsuleCollider3D, RigidBody3D, SphereCollider3D};

    use super::*;

    fn fixed() -> RigidBody3D {
        RigidBody3D {
            body_type: BodyType::Static,
            ..RigidBody3D::default()
        }
    }

    /// A floor, a player 10 units from a target straight ahead, and a camera.
    fn arena() -> (World, Entity, Entity) {
        let mut world = World::default();
        world
            .register_all::<(Player, Target, Slide, Sparks)>()
            .expect("they register");
        let floor = Transform {
            translation: DVec3::new(0.0, -0.5, 0.0),
            scale: Vec3::new(40.0, 1.0, 40.0),
            ..Transform::IDENTITY
        };
        world.spawn((floor, fixed(), BoxCollider3D::default()));
        let at = |z: f64, y: f64| Transform {
            translation: DVec3::new(0.0, y, z),
            ..Transform::IDENTITY
        };
        let player = Player {
            speed: 8.0,
            sprint: 1.6,
            jump_speed: 6.5,
            sensitivity: 0.0025,
            eye: 0.7,
            heading: Vec2::ZERO,
            sprinting: false,
            yaw: 0.0,
            pitch: 0.0,
            ammo: 12,
            magazine: 12,
            reload: 0.0,
            reload_time: 1.0,
            clock: 0.0,
            running: false,
            finished: false,
            best: 0.0,
            shots: 0,
            hits: 0,
        };
        let player = world.spawn((
            at(5.0, 1.0),
            player,
            CharacterController3D::default(),
            CapsuleCollider3D {
                height: 1.8,
                radius: 0.35,
                ..CapsuleCollider3D::default()
            },
        ));
        let target = world.spawn((
            at(-5.0, 1.7),
            Target {
                points: 1,
                hits: 3,
                health: 3,
                flash: 0.0,
                base: Vec4::ONE,
            },
            fixed(),
            SphereCollider3D::default(),
        ));
        let camera = Camera {
            projection: Projection::Perspective {
                fov: 1.2,
                near: 0.1,
            },
            primary: true,
        };
        world.spawn((Transform::IDENTITY, camera));
        (world, player, target)
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
    fn walking_looking_jumping_and_the_camera_follow_the_player() {
        let (mut world, player, _) = arena();
        let mut game = Game::new();
        run(&mut game, &mut world, 30);
        let start = world.get::<Transform>(player).expect("placed").translation;
        let walk = Command::Move {
            heading: Vec2::new(0.0, 1.0),
        };
        game.commands.push(walk);
        run(&mut game, &mut world, 30);
        let walked = world.get::<Transform>(player).expect("placed").translation;
        assert!(
            walked.z < start.z - 2.0 && walked.x.abs() < 0.01,
            "{walked}"
        );
        // A quarter turn left, then forward goes along -X.
        let by = Vec2::new(-std::f32::consts::FRAC_PI_2 / 0.0025, 0.0);
        game.commands.push(Command::Look { by });
        run(&mut game, &mut world, 30);
        let turned = world.get::<Transform>(player).expect("placed").translation;
        assert!(turned.x < walked.x - 2.0, "{turned}");
        game.commands.push(Command::Move {
            heading: Vec2::ZERO,
        });
        game.commands.push(Command::Jump);
        run(&mut game, &mut world, 10);
        let up = world.get::<Transform>(player).expect("placed").translation;
        assert!(up.y > turned.y + 0.5, "{up}");
        let (at, _) = world
            .query::<(&Transform, &Camera)>()
            .next()
            .expect("a camera");
        assert!((at.translation - up - DVec3::Y * 0.7).length() < 1e-6);
    }

    #[test]
    fn three_shots_sink_the_target_they_hit_and_only_that() {
        let (mut world, player, target) = arena();
        let mut game = Game::new();
        run(&mut game, &mut world, 10);
        // Aimed away from it, nothing is hit.
        let by = Vec2::new(400.0, 0.0);
        game.commands.push(Command::Look { by });
        game.commands.push(Command::Fire);
        run(&mut game, &mut world, 1);
        assert!(world.contains(target));
        let by = Vec2::new(-400.0, 0.0);
        game.commands.push(Command::Look { by });
        // It takes three hits, each throwing sparks.
        for hit in 1..=3 {
            game.commands.push(Command::Fire);
            run(&mut game, &mut world, 1);
            let sunk = world
                .get::<Transform>(target)
                .expect("placed")
                .translation
                .y
                < 0.0;
            assert_eq!(sunk, hit == 3);
            assert_eq!(world.query::<&Sparks>().count(), hit);
        }
        let shooter = world.get::<Player>(player).expect("a player");
        assert_eq!((shooter.shots, shooter.hits), (4, 3));
        // That was the last target: the round is done, and its time the best.
        assert!(shooter.finished && !shooter.running && shooter.best > 0.0);
        // The sparks burn out.
        run(&mut game, &mut world, 70);
        assert_eq!(world.query::<&Sparks>().count(), 0);
    }

    /// A new round stands a target up with its own `hits`, and a second round's slower time leaves
    /// the best as it was.
    #[test]
    fn a_new_round_gives_each_target_its_hits_and_keeps_the_best() {
        let (mut world, player, target) = arena();
        world.get_mut::<Target>(target).expect("a target").hits = 5;
        let mut game = Game::new();
        run(&mut game, &mut world, 10);
        let fire = |game: &mut Game, world: &mut World, shots: u32| {
            for _ in 0..shots {
                game.commands.push(Command::Fire);
                run(game, world, 1);
            }
        };
        fire(&mut game, &mut world, 3);
        let best = world.get::<Player>(player).expect("a player").best;
        game.commands.push(Command::NewRound);
        run(&mut game, &mut world, 1);
        let stood = world.get::<Target>(target).expect("a target");
        assert_eq!(stood.health, 5);
        let at = world.get::<Transform>(target).expect("placed").translation;
        assert!(at.y > 0.0, "{at}");
        let shooter = world.get::<Player>(player).expect("a player");
        assert!(!shooter.finished && shooter.clock == 0.0 && shooter.ammo == 12);
        // Five hits a tick apart take longer than three: the best stays.
        fire(&mut game, &mut world, 5);
        let shooter = world.get::<Player>(player).expect("a player");
        assert!(
            shooter.finished && shooter.clock > best,
            "{}",
            shooter.clock
        );
        assert_eq!(shooter.best, best);
    }

    /// A full gun doesn't reload; an empty one's click reloads it, firing nothing, and it fires
    /// again once `reload_time` has passed.
    #[test]
    fn an_empty_gun_reloads_and_a_full_one_doesnt() {
        let (mut world, player, _) = arena();
        let mut game = Game::new();
        run(&mut game, &mut world, 10);
        game.commands.push(Command::Reload);
        run(&mut game, &mut world, 1);
        assert_eq!(world.get::<Player>(player).expect("a player").reload, 0.0);
        // Aimed away, so no target's hit ends the round.
        game.commands.push(Command::Look {
            by: Vec2::new(400.0, 0.0),
        });
        world.get_mut::<Player>(player).expect("a player").ammo = 0;
        game.commands.push(Command::Fire);
        run(&mut game, &mut world, 1);
        let shooter = world.get::<Player>(player).expect("a player");
        assert!(shooter.reload > 0.0 && shooter.shots == 0 && !shooter.running);
        let ticks = (1.0 / Tick::DT.as_secs_f32()).ceil() as u64;
        run(&mut game, &mut world, ticks);
        let shooter = world.get::<Player>(player).expect("a player");
        assert_eq!((shooter.reload, shooter.ammo), (0.0, 12));
        game.commands.push(Command::Fire);
        run(&mut game, &mut world, 1);
        let shooter = world.get::<Player>(player).expect("a player");
        assert_eq!((shooter.shots, shooter.ammo), (1, 11));
    }

    #[test]
    fn a_clock_reads_to_the_hundredth() {
        let read = |seconds: f32| {
            let mut line = String::new();
            write_clock(&mut line, seconds);
            line
        };
        assert_eq!(read(3.42), "0:03.42");
        assert_eq!(read(59.996), "1:00.00");
        assert_eq!(read(-1.0), "0:00.00");
    }
}
