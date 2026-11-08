//! Foxball, the engine's networked 3D demo: two players, each a fox in a walled arena, knocking a
//! ball into the other's goal, first to five. Run alone, the keys, a pad and the mouse steer player
//! 0; or networked, in lockstep, a run given `--host <address>` steering player 0 and one given
//! `--join <address>` player 1, each with its own keys; or as clients of a run given
//! `--serve <address>`, each given `--connect <address>` steering the player the server gives it,
//! each fox its player's [`Owner`]. W, A, S and D, or a pad's left stick, walk a fox forward, back and to
//! its sides; the mouse turns it, the cursor grabbed from a click in the view until Escape, as do Q
//! and E and the right stick; and Space, a pad's South, or a click with the cursor grabbed kicks
//! the ball, if it's in front of the fox. The foxes are Khronos's Fox (`models/fox`, CC0 by
//! PixelMannen, rigged and animated by tomkranis and made glTF by AsoboStudio and scurest under CC
//! BY 4.0, as its README there says), skinned, blending Survey, Walk and Run by how fast each goes,
//! characters that rapier3d's controller walks, which push the ball as they walk into it, under
//! Poly Haven's Kloofendal 48d Partly Cloudy Puresky (`textures/sky.hdr`, CC0); the ball and the
//! arena are 3D physics, the arena's walls and goals its scene file, `scenes/arena.ron`, which a
//! development build applies as it's saved, each wall, a goal's pocket's too, an instance of one
//! prefab, `prefabs/wall.prefab`, placed and sized by its own transform. Each run's view follows
//! its own fox, a little above and behind it, the mouse looking up and down too. A kick throws a
//! puff of dust from the ball, a goal a fountain in its scorer's color from the goal's mouth, and a
//! running fox kicks up dust behind it: emitters of the world, which the game bursts and sets at
//! its ticks. Its sounds, `sounds/*.wav`, play from where they happen, heard by their direction and
//! distance from the view: a kick from the ball, a goal from where it went in, and a match won.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::f32::consts::PI;
use std::fmt::Write as _;
use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use gg_engine::math::libm::expf;
use gg_engine::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use gg_engine::{
    Actions, Align, Animator, App, Binding, CapsuleCollider3D, Change, CharacterController3D,
    Commands, Component, Context, EmitterShape, Entity, Error, Event, Font, Key, Layer, Model,
    MouseButton, Name, Owner, PadButton, Parent, ParticleBlend, ParticleEmitter, Place, Play,
    Projection, Reflect, RigidBody3D, Sound, Source, Stick, TextStyle, Tick, Toward, Transform,
    Viewpoint, World, step_animation, step_physics,
};

#[cfg(test)]
mod tests;

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's Performance and Inspector panels: what `main` runs, and what its
/// recordings replay.
fn app() -> App {
    let app = App::new("Foxball").project(env!("CARGO_MANIFEST_DIR"));
    let app = app.layer(Foxball::new()).overlay(gg_engine::PerfPanel);
    app.overlay(gg_engine::Inspector::default())
}

/// How far the field reaches from its middle, across along X and long along Z, to its walls' inner
/// faces, as `scenes/arena.ron` builds them.
const FIELD: Vec2 = Vec2::new(7.0, 10.0);

/// How far each goal's mouth reaches across from the middle of its end.
const GOAL: f32 = 2.0;

/// How far past its end's wall the ball's middle goes into a goal to count.
const GOAL_DEPTH: f32 = 0.45;

/// Each player's color, in linear RGB: blue and orange, which the emitters of the goals' mouths in
/// `scenes/arena.ron` burst in too, brightened.
const COLORS: [[f32; 3]; 2] = [[0.15, 0.55, 1.0], [1.0, 0.45, 0.1]];

/// Where each player's fox starts, and goes back to after each goal, and the way it faces then: 0
/// along +Z, player 0 toward the far goal, which it attacks, and player 1 toward the near one.
const STARTS: [(Vec3, f32); 2] = [
    (Vec3::new(0.0, 0.55, -5.0), 0.0),
    (Vec3::new(0.0, 0.55, 5.0), PI),
];

/// Where the ball starts, and goes back to after each goal, or once it has left the arena.
const BALL_START: Vec3 = Vec3::new(0.0, 0.4, 0.0);

/// A fox's character: how tall its capsule is and how far it is across.
const CAPSULE: (f32, f32) = (1.0, 0.35);

/// How large the fox's model is drawn, its units centimeters.
const FOX_SCALE: f32 = 0.02;

/// How fast a fox runs, in units a second, and how fast it walks as Walk alone plays.
const RUN_SPEED: f32 = 5.0;
const WALK_SPEED: f32 = 1.6;

/// The dust a fox kicks up behind its feet as it runs: where its emitter is from the capsule's
/// middle, the puffs a second it gives at the fox's running speed, fewer as it runs slower and none
/// as it walks, as [`animate`] sets them, and how it looks.
const DUST_AT: Vec3 = Vec3::new(0.0, -0.45, -0.3);
const DUST_RATE: f32 = 24.0;
const DUST: ParticleEmitter = ParticleEmitter {
    rate: 0.0,
    burst: 0,
    bursts: 0,
    lifetime: [0.45, 0.8],
    speed: [0.3, 0.8],
    shape: EmitterShape::Cone {
        angle: 0.9,
        radius: 0.15,
    },
    acceleration: Vec3::new(0.0, 0.4, 0.0),
    drag: 2.5,
    size: [0.2, 0.6],
    color: [
        Vec4::new(0.95, 0.92, 0.84, 0.45),
        Vec4::new(0.95, 0.92, 0.84, 0.0),
    ],
    spin: [0.0; 2],
    texture: None,
    cells: [1, 1],
    blend: ParticleBlend::Alpha,
};

/// How fast Q and E or a stick turn a fox, at full tilt, in radians a second.
const TURN_SPEED: f32 = 3.0;

/// How far a unit of the mouse's motion turns a fox, or looks up or down, in radians.
const SENSITIVITY: f32 = 0.0025;

/// A kick: how far the ball's middle may be from just in front of the fox, how fast it sends the
/// ball forward and up, and how many ticks until the fox's next.
const KICK_REACH: f32 = 1.1;
const KICK_SPEED: f32 = 9.0;
const KICK_LIFT: f32 = 1.5;
const KICK_EVERY: u32 = 20;

/// How much of its speed and spin the ball loses each second as it rolls, so that it comes to rest:
/// the engine's bodies keep theirs, but for what they hit.
const BALL_DRAG: f32 = 0.6;

/// How many goals win a match.
const WINS_AT: u32 = 5;

/// The view behind a run's own fox: how far back it is, how high above the fox's middle it looks
/// at, and how far it looks down, as it starts, and the least and most, in radians.
const VIEW_DISTANCE: f32 = 5.0;
const VIEW_ABOVE: f32 = 1.5;
const VIEW_PITCH: f32 = 0.35;
const VIEW_PITCHES: (f32, f32) = (-0.25, 1.2);

/// How quickly the view catches up with its fox, which moves at the ticks, a share of the way left
/// each second as `1 - e^(-rate)`: smoothing the ticks' steps between frames.
const VIEW_FOLLOW: f32 = 14.0;

/// The scores on the canvas, in its units: how far they are from the view's top corners, and how
/// tall a score's em is and its matches won's, below it.
const HUD_INSET: f32 = 40.0;
const SCORE_SIZE: f32 = 80.0;
const WINS_SIZE: f32 = 32.0;

/// A player's fox, the character its capsule is: which player steers it, which way the keys or a
/// stick walk it, how fast it turns, the way it faces, and the ticks until its next kick.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Player {
    index: u8,
    /// Across to its right and up forward, no longer than 1.
    heading: Vec2,
    /// To its right, from -1 to 1.
    spin: f32,
    /// Its turn to its left about the world's up, in radians, from facing +Z.
    yaw: f32,
    cooldown: u32,
}

/// The ball, which the foxes push and kick, and whose emitter bursts as it's kicked.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Ball;

/// A goal's mouth, whose emitter bursts as the ball goes in: the player that scores by it.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Goal {
    player: u8,
}

/// The match: each player's goals in it, and matches won.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Match {
    scores: [u32; 2],
    wins: [u32; 2],
}

/// What the players' keys, pads and mice do, queued as commands for the next tick, each naming its
/// player, which a recording and a lockstep peer keep.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Has the player's fox walk this way, until the next: across to its right and up forward.
    Walk { player: u8, heading: Vec2 },
    /// Has the player's fox turn at this share of its speed, to its right, until the next.
    Spin { player: u8, rate: f32 },
    /// Turns the player's fox to its right by this many radians, as the mouse moved.
    Turn { player: u8, by: f32 },
    /// Has the player's fox kick the ball, if it can.
    Kick { player: u8 },
}

/// What the keys and pads do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// W, A, S and D, the arrows and the left stick.
    Walk,
    /// Q and E, and the right stick across.
    Spin,
    /// Space and South.
    Kick,
}

/// The game's sounds, from its `sounds` folder: a kick, a goal and a match won.
#[derive(Clone, Copy)]
struct Sounds {
    kick: Sound,
    goal: Sound,
    win: Sound,
}

/// What the game loads: its sounds, its font and the fox.
struct Assets {
    sounds: Sounds,
    font: Font,
    fox: Model,
}

/// The game's layer: it builds the match and the players' foxes as it attaches, in the arena its
/// start scene holds; applies each player's commands at each tick, walking, turning and kicking,
/// then blends each fox's clips by how fast it goes, steps animation and physics, and counts the
/// goals; and as it updates, has the view follow this run's fox and draws the scores. An asset of
/// its own, or its start scene, that doesn't load ends the run with its error, through
/// [`Context::fail`].
struct Foxball {
    /// The player this run's keys steer: 0, unless it joined a lockstep session or a server gave
    /// it another.
    player: u8,
    actions: Actions<Act>,
    commands: Vec<Command>,
    /// Once loaded.
    assets: Option<Assets>,
    /// Whether this run wants the cursor grabbed, for the mouse to turn its fox: from a click in
    /// the view until Escape.
    grab: bool,
    /// How far this run's view looks down, which the mouse moves: this run's own, as it's no part
    /// of the world.
    pitch: f32,
    /// What this run's view looks at, as it catches up with its fox.
    seen: Option<Vec3>,
    /// The scores' text, written again in its memory each frame.
    text: String,
}

impl Foxball {
    fn new() -> Self {
        let mut actions = Actions::default();
        let arrows = Binding::Axis2 {
            x: [Key::ArrowLeft, Key::ArrowRight].map(Source::Key),
            y: [Key::ArrowDown, Key::ArrowUp].map(Source::Key),
        };
        let spin_keys = Binding::Axis {
            negative: Source::Key(Key::KeyQ),
            positive: Source::Key(Key::KeyE),
        };
        let spin_stick = Binding::Axis {
            negative: Source::PadStick(Stick::Right, Toward::Left),
            positive: Source::PadStick(Stick::Right, Toward::Right),
        };
        let bound = [
            (Act::Walk, Binding::WASD),
            (Act::Walk, arrows),
            (Act::Walk, Binding::stick(Stick::Left)),
            (Act::Spin, spin_keys),
            (Act::Spin, spin_stick),
            (Act::Kick, Binding::Button(Source::Key(Key::Space))),
            (
                Act::Kick,
                Binding::Button(Source::PadButton(PadButton::South)),
            ),
        ]
        .into_iter()
        .try_for_each(|(action, binding)| actions.bind(action, binding));
        bound.expect("the game's bindings are each of one kind, of other sources");
        Self {
            player: 0,
            actions,
            commands: Vec::new(),
            assets: None,
            grab: false,
            pitch: VIEW_PITCH,
            seen: None,
            text: String::new(),
        }
    }

    /// Loads the game's sounds, font and fox, and registers its components. Fails with the first
    /// that doesn't load.
    fn load_game(&mut self, cx: &mut Context) -> Result<(), Error> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let sound = |cx: &mut Context, name: &str| {
            cx.add_sound(name, root.join(format!("sounds/{name}.wav")))
        };
        let sounds = Sounds {
            kick: sound(cx, "kick")?,
            goal: sound(cx, "goal")?,
            win: sound(cx, "win")?,
        };
        let font = cx.add_font("Inter Bold", root.join("fonts/Inter-Bold.ttf"))?;
        let fox = cx.add_model("Fox", root.join("models/fox/Fox.gltf"))?;
        register(cx.world_mut());
        self.assets = Some(Assets { sounds, font, fox });
        Ok(())
    }

    /// How far the commands queued for this run's player, and not yet applied, turn its fox: so
    /// that the view turns with the mouse at once, rather than at the next tick.
    fn turning(&self) -> f32 {
        let player = self.player;
        let turns = self.commands.iter().filter_map(|command| match *command {
            Command::Turn { player: of, by } if of == player => Some(by),
            _ => None,
        });
        turns.sum()
    }
}

impl Layer for Foxball {
    fn load(&mut self, cx: &mut Context) {
        if let Err(error) = self.load_game(cx) {
            cx.fail(error);
        }
    }

    fn attach(&mut self, cx: &mut Context) {
        // A run that ends as it loads builds no match.
        if cx.exiting() {
            return;
        }
        self.player = cx.player().unwrap_or(0);
        // The arena, watched as the game runs, in development builds, which apply its saves.
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
            return;
        }
        if let Some(assets) = &self.assets {
            build(cx.world_mut(), assets.fox);
        }
    }

    /// Takes back which player this run steers, which a joiner's world, the host's, and a client's,
    /// the server's, don't say, and lets go of what may have been held as the world was taken.
    fn restored(&mut self, cx: &mut Context) {
        self.player = cx.player().unwrap_or(0);
        let player = self.player;
        self.commands.extend([
            Command::Walk {
                player,
                heading: Vec2::ZERO,
            },
            Command::Spin { player, rate: 0.0 },
        ]);
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        let _zone = gg_engine::zone!();
        let sounds = self.assets.as_ref().map(|assets| assets.sounds);
        for command in self.commands.drain(..) {
            apply(world, &command, sounds);
        }
        walk(world, Tick::DT);
        roll(world, Tick::DT);
        animate(world);
        step_animation(world);
        step_physics(world);
        score(world, sounds);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    /// Grabs the cursor while this run wants it, has the view follow this run's fox, and draws the
    /// scores.
    fn update(&mut self, cx: &mut Context, dt: Duration) {
        cx.set_cursor_grabbed(self.grab);
        let turning = self.turning();
        let own = cx.world().query::<(&Player, &Transform)>();
        let own = own
            .into_iter()
            .find(|(player, _)| player.index == self.player);
        if let Some((player, at)) = own.map(|(player, at)| (*player, *at)) {
            let target = at.translation + Vec3::Y * VIEW_ABOVE;
            let caught = 1.0 - expf(-VIEW_FOLLOW * dt.as_secs_f32());
            let seen = self.seen.map_or(target, |seen| seen.lerp(target, caught));
            self.seen = Some(seen);
            cx.set_viewpoint(follow(seen, player.yaw - turning, self.pitch));
        }
        let Some(assets) = &self.assets else {
            return;
        };
        let font = assets.font;
        if let Some(&found) = cx.world().query::<&Match>().next() {
            draw_scores(cx, &found, font, &mut self.text);
            draw_hint(cx, font, self.grab);
        }
    }

    fn event(&mut self, _: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, rather than at the next update, so that the next tick applies them.
        let Self {
            player,
            actions,
            commands,
            grab,
            pitch,
            ..
        } = self;
        let player = *player;
        actions.event(event, |change| match change {
            Change::Axis2(Act::Walk, heading) => commands.push(Command::Walk { player, heading }),
            Change::Axis(Act::Spin, rate) => commands.push(Command::Spin { player, rate }),
            Change::Pressed(Act::Kick) => commands.push(Command::Kick { player }),
            _ => {}
        });
        match *event {
            // A click grabs the cursor, and with it grabbed, kicks.
            Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            } => {
                if *grab {
                    commands.push(Command::Kick { player });
                }
                *grab = true;
            }
            Event::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => *grab = false,
            // Each tick turns the fox once, by the motion since the tick before; up and down is
            // the view's alone.
            Event::MouseMotion { x, y } if *grab => {
                let by = x as f32 * SENSITIVITY;
                match commands.last_mut() {
                    Some(Command::Turn {
                        player: of,
                        by: queued,
                    }) if *of == player => {
                        *queued += by;
                    }
                    _ => commands.push(Command::Turn { player, by }),
                }
                let (least, most) = VIEW_PITCHES;
                *pitch = (*pitch + y as f32 * SENSITIVITY).clamp(least, most);
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

/// Registers the game's components with `world`, so that its scene and the tool API know them
/// before any entity has one.
fn register(world: &mut World) {
    let registered = world.register::<Player>().and_then(|()| {
        world.register::<Ball>()?;
        world.register::<Goal>()?;
        world.register::<Match>()
    });
    registered.expect("the game's components describe themselves");
}

/// Builds what the arena's scene doesn't hold: the match, and each player's fox at its start, a
/// character's capsule with `fox`'s model under it, its feet at the capsule's bottom, its clips
/// played by an animator, which [`animate`] blends, and its dust behind them.
fn build(world: &mut World, fox: Model) {
    world.spawn((
        Match {
            scores: [0; 2],
            wins: [0; 2],
        },
        Name("Match".to_owned()),
    ));
    let (height, radius) = CAPSULE;
    for (index, &(start, yaw)) in STARTS.iter().enumerate() {
        let at = Transform {
            translation: start,
            rotation: Quat::from_rotation_y(yaw),
            ..Transform::IDENTITY
        };
        let capsule = CapsuleCollider3D {
            height,
            radius,
            friction: 0.0,
            ..CapsuleCollider3D::default()
        };
        let player = Player {
            index: index as u8,
            heading: Vec2::ZERO,
            spin: 0.0,
            yaw,
            cooldown: 0,
        };
        let owner = Owner {
            player: index as u8,
        };
        let name = Name(format!("Player {}", index + 1));
        let character = CharacterController3D::default();
        let body = world.spawn((at, capsule, character, player, owner, name));
        let feet = Transform {
            translation: Vec3::new(0.0, -height / 2.0, 0.0),
            scale: Vec3::splat(FOX_SCALE),
            ..Transform::IDENTITY
        };
        let model = fox.spawn(world, feet);
        let named = world.insert(model, Name(format!("Fox {}", index + 1)));
        let played = named.and_then(|()| world.insert(model, Animator(vec![Play::new("Survey")])));
        let parented = played.and_then(|()| world.set_parent(model, Some(body)));
        parented.expect("the fox and its body, just spawned");
        let behind = Transform {
            translation: DUST_AT,
            ..Transform::IDENTITY
        };
        let dust = world.spawn((behind, DUST, Name(format!("Dust {}", index + 1))));
        let parented = world.set_parent(dust, Some(body));
        parented.expect("the dust and its body, just spawned");
    }
}

/// Applies `command` to `world`: to its player's fox, which way it walks, how fast it turns, a turn
/// or a kick, which sends the ball forward and up if it's in front of the fox, bursts the ball's
/// emitter and plays `sounds`' kick from it.
fn apply(world: &mut World, command: &Command, sounds: Option<Sounds>) {
    let player = match *command {
        Command::Walk { player, .. }
        | Command::Spin { player, .. }
        | Command::Turn { player, .. }
        | Command::Kick { player } => player,
    };
    let mut kicker = None;
    let players = world.query_mut::<(&mut Player, &mut Transform)>();
    for (fox, at) in players.filter(|(fox, _)| fox.index == player) {
        match *command {
            Command::Walk { heading, .. } => fox.heading = heading.clamp_length_max(1.0),
            Command::Spin { rate, .. } => fox.spin = rate.clamp(-1.0, 1.0),
            Command::Turn { by, .. } => {
                fox.yaw = wrapped(fox.yaw - by);
                at.rotation = Quat::from_rotation_y(fox.yaw);
            }
            Command::Kick { .. } if fox.cooldown == 0 => {
                kicker = Some((at.translation, fox.yaw));
                fox.cooldown = KICK_EVERY;
            }
            Command::Kick { .. } => {}
        }
    }
    let Some((from, yaw)) = kicker else {
        return;
    };
    let forward = Quat::from_rotation_y(yaw) * Vec3::Z;
    let front = from + forward * CAPSULE.1;
    let balls = world.query_mut::<(Entity, &Ball, &Transform, &mut RigidBody3D)>();
    let mut kicked = None;
    for (ball, _, at, body) in balls {
        let flat = Vec3::new(at.translation.x, front.y, at.translation.z);
        if flat.distance(front) < KICK_REACH && (at.translation.y - from.y).abs() < 1.0 {
            body.velocity = forward * KICK_SPEED + Vec3::Y * KICK_LIFT;
            kicked = Some(ball);
        }
    }
    let Some(ball) = kicked else {
        return;
    };
    if let Some(emitter) = world.get_mut::<ParticleEmitter>(ball) {
        emitter.bursts = emitter.bursts.wrapping_add(1);
    }
    if let Some(sounds) = sounds {
        world.play_sound(sounds.kick, Place::On(ball));
    }
}

/// `angle`, in radians, brought within a turn of 0 each way, so that a fox turned round and round
/// keeps its yaw's precision.
fn wrapped(angle: f32) -> f32 {
    if angle > PI {
        angle - 2.0 * PI
    } else if angle < -PI {
        angle + 2.0 * PI
    } else {
        angle
    }
}

/// Turns each fox, `dt` on, as fast as its spin says, and has its character walk the way its
/// heading says, as it faces, at up to its running speed, keeping what its character's fall gives;
/// counting its kick's cooldown down.
fn walk(world: &mut World, dt: Duration) {
    let dt = dt.as_secs_f32();
    let foxes = world.query_mut::<(&mut Player, &mut Transform, &mut CharacterController3D)>();
    for (fox, at, character) in foxes {
        if fox.spin != 0.0 {
            fox.yaw = wrapped(fox.yaw - fox.spin * TURN_SPEED * dt);
            at.rotation = Quat::from_rotation_y(fox.yaw);
        }
        // Facing +Z, its right is -X.
        let turned = Quat::from_rotation_y(fox.yaw);
        let way = turned * Vec3::new(-fox.heading.x, 0.0, fox.heading.y);
        let across = way * RUN_SPEED;
        character.velocity = Vec3::new(across.x, character.velocity.y, across.z);
        fox.cooldown = fox.cooldown.saturating_sub(1);
    }
}

/// Slows the ball, `dt` on, by [`BALL_DRAG`]: its speed and its spin alike.
fn roll(world: &mut World, dt: Duration) {
    let kept = 1.0 - BALL_DRAG * dt.as_secs_f32();
    for (_, body) in world.query_mut::<(&Ball, &mut RigidBody3D)>() {
        body.velocity *= kept;
        body.angular_velocity *= kept;
    }
}

/// Sets each fox's clips' weights by how fast its character walks: Survey alone standing, Walk
/// alone at [`WALK_SPEED`], and Run alone at [`RUN_SPEED`], blending between; a clip at no weight
/// leaves its animator, joining again from its start once it has some. Its dust's rate goes with
/// Run's weight, to [`DUST_RATE`] at its running speed.
fn animate(world: &mut World) {
    let speeds: Vec<(Entity, f32)> = world
        .query::<(Entity, &Player, &CharacterController3D)>()
        .map(|(entity, _, character)| (entity, character.velocity.with_y(0.0).length()))
        .collect();
    let speed_of = |body: Entity| speeds.iter().find(|(entity, _)| *entity == body);
    let running = |speed: f32| ((speed - WALK_SPEED) / (RUN_SPEED - WALK_SPEED)).clamp(0.0, 1.0);
    for (&Parent(body), dust) in world.query_mut::<(&Parent, &mut ParticleEmitter)>() {
        if let Some(&(_, speed)) = speed_of(body) {
            dust.rate = DUST_RATE * running(speed);
        }
    }
    for (&Parent(body), clips) in world.query_mut::<(&Parent, &mut Animator)>() {
        let Some(&(_, speed)) = speed_of(body) else {
            continue;
        };
        let walking = (speed / WALK_SPEED).min(1.0);
        let running = running(speed);
        let weights = [
            ("Survey", 1.0 - walking),
            ("Walk", walking * (1.0 - running)),
            ("Run", running),
        ];
        for (clip, weight) in weights {
            match clips.0.iter_mut().find(|play| play.clip == clip) {
                Some(play) => (play.weight, play.target) = (weight, weight),
                None if weight > 0.0 => clips.0.push(Play {
                    weight,
                    target: weight,
                    ..Play::new(clip)
                }),
                None => {}
            }
        }
    }
}

/// Counts a goal once the ball has gone into one: to player 0 for the far goal, at +Z, and player 1
/// for the near one, bursting the emitters of the scorer's goal's mouth and playing `sounds`' goal
/// where it went in, and at the goal that wins a match, giving the match to its player and starting
/// another, playing their fanfare; then puts the ball and the foxes back at their starts. A ball
/// that has left the arena goes back to its start alone.
fn score(world: &mut World, sounds: Option<Sounds>) {
    let ball = world.query::<(&Ball, &Transform)>().next();
    let Some(at) = ball.map(|(_, at)| at.translation) else {
        return;
    };
    let into = at.z.abs() > FIELD.y + GOAL_DEPTH && at.x.abs() < GOAL;
    let out = at.y < -5.0 || at.x.abs() > FIELD.x + 3.0 || at.z.abs() > FIELD.y + 4.0;
    if into {
        let scorer = usize::from(at.z < 0.0);
        let mut won = false;
        if let Some(found) = world.query_mut::<&mut Match>().next() {
            found.scores[scorer] += 1;
            if found.scores[scorer] >= WINS_AT {
                found.scores = [0; 2];
                found.wins[scorer] += 1;
                won = true;
            }
        }
        let mouths = world.query_mut::<(&Goal, &mut ParticleEmitter)>();
        for (_, emitter) in mouths.filter(|(goal, _)| usize::from(goal.player) == scorer) {
            emitter.bursts = emitter.bursts.wrapping_add(1);
        }
        if let Some(sounds) = sounds {
            world.play_sound(sounds.goal, Place::At(at));
            if won {
                world.play_sound(sounds.win, Place::Nowhere);
            }
        }
        to_starts(world);
    } else if out {
        to_ball_start(world);
    }
}

/// Puts the ball and the foxes back at their starts, still.
fn to_starts(world: &mut World) {
    to_ball_start(world);
    let foxes = world.query_mut::<(&mut Player, &mut Transform, &mut CharacterController3D)>();
    for (fox, at, character) in foxes {
        let (start, yaw) = STARTS[usize::from(fox.index) % STARTS.len()];
        fox.yaw = yaw;
        at.translation = start;
        at.rotation = Quat::from_rotation_y(yaw);
        character.velocity = Vec3::ZERO;
    }
}

/// Puts the ball back at its start, still.
fn to_ball_start(world: &mut World) {
    for (_, at, body) in world.query_mut::<(&Ball, &mut Transform, &mut RigidBody3D)>() {
        at.translation = BALL_START;
        at.rotation = Quat::IDENTITY;
        body.velocity = Vec3::ZERO;
        body.angular_velocity = Vec3::ZERO;
    }
}

/// A view looking at `seen` from [`VIEW_DISTANCE`] behind it, the way a fox at `yaw` faces,
/// `pitch` radians down.
fn follow(seen: Vec3, yaw: f32, pitch: f32) -> Viewpoint {
    // A view looks along its -Z: turned half round, it looks along +Z, the way a fox at 0 faces.
    let rotation = Quat::from_rotation_y(yaw + PI) * Quat::from_rotation_x(-pitch);
    let looking = rotation * Vec3::NEG_Z;
    Viewpoint {
        position: seen - looking * VIEW_DISTANCE,
        rotation,
        projection: Projection::Perspective {
            fov: 1.1,
            near: 0.05,
        },
    }
}

/// Draws the match's scores on the canvas, in `font`, writing each text into `text`: each player's
/// goals in its color in the view's top corner on its side, and below them the matches it has won.
fn draw_scores(cx: &mut Context, found: &Match, font: Font, text: &mut String) {
    for (index, &color) in COLORS.iter().enumerate() {
        let (corner, inward) = if index == 0 {
            (Align::Start, 1.0)
        } else {
            (Align::End, -1.0)
        };
        let mut canvas = cx.canvas([corner, Align::Start]);
        let at = |down: f32| Mat4::from_translation(Vec3::new(inward * HUD_INSET, -down, 0.0));
        let style = |size: f32, alpha: f32| TextStyle {
            font: Some(font),
            size,
            color: Vec3::from(color).extend(alpha),
            align: [corner, Align::Start],
        };
        let write = |text: &mut String, value: u32, then: &str| {
            text.clear();
            write!(text, "{value}{then}").expect("a string takes any text");
        };
        write(text, found.scores[index], "");
        canvas.draw_text(at(HUD_INSET), text, &style(SCORE_SIZE, 1.0));
        let won = found.wins[index];
        write(text, won, if won == 1 { " WIN" } else { " WINS" });
        let below = HUD_INSET + SCORE_SIZE * 1.25;
        canvas.draw_text(at(below), text, &style(WINS_SIZE, 0.7));
    }
}

/// Draws, at the view's bottom, in `font`, the keys: how to look about, while the cursor isn't
/// grabbed, or how to let it go, while it is.
fn draw_hint(cx: &mut Context, font: Font, grabbed: bool) {
    let style = TextStyle {
        font: Some(font),
        size: 26.0,
        color: Vec4::new(1.0, 1.0, 1.0, 0.5),
        align: [Align::Center, Align::End],
    };
    let at = Mat4::from_translation(Vec3::new(0.0, 24.0, 0.0));
    let hint = if grabbed {
        "WASD: WALK  ·  MOUSE: LOOK  ·  CLICK OR SPACE: KICK  ·  ESC: FREE THE CURSOR"
    } else {
        "WASD: WALK  ·  Q, E: TURN  ·  SPACE: KICK  ·  CLICK TO LOOK WITH THE MOUSE"
    };
    cx.canvas([Align::Center, Align::End])
        .draw_text(at, hint, &style);
}
