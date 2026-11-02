//! Shards, the engine's first demo: two players, each a disc in an arena, steering about, dashing
//! into each other, and collecting the shards that turn up where the match's own random numbers put
//! them, first to ten. Run alone, the keys steer player 0; or networked, in lockstep, a run given
//! `--host <address>` steering player 0 and one given `--join <address>` player 1, each with its own
//! keys. The arena's walls are its scene file, `scenes/arena.ron`, which a development build
//! applies as it's saved; its floor is its shader, `shaders/floor.slang`, which it reloads as it's
//! saved; and its scores are text in its font, `fonts/Inter-Bold.ttf`.

use std::fmt::Write as _;
use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use bytemuck::NoUninit;
use gg_engine::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use gg_engine::{
    Actions, Align, App, Binding, Blend, BodyType, Camera, Change, Circle, CircleCollider2D,
    Commands, Component, Context, Entity, Event, Font, Key, Layer, Mesh, Name, PadButton,
    Physics2D, Projection, Reflect, RigidBody2D, Shader, Source, Sprite, Stick, TextStyle, Tick,
    Transform, World, step_physics,
};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's Performance, Entities and Inspector panels: what `main` runs, and
/// what its recordings replay.
fn app() -> App {
    let app = App::new("Shards").project(env!("CARGO_MANIFEST_DIR"));
    let app = app.layer(Shards::new()).overlay(gg_engine::PerfPanel);
    app.overlay(gg_engine::Inspector::default())
}

/// How far the arena's inside reaches from its middle, across and up.
const ARENA: Vec2 = Vec2::new(8.0, 4.5);

/// How high the view is, in units: the arena, its walls and the scores above it.
const VIEW_HEIGHT: f32 = 11.5;

/// Each player's color, in linear RGB: blue and orange.
const COLORS: [[f32; 3]; 2] = [[0.15, 0.55, 1.0], [1.0, 0.45, 0.1]];

/// Where each player starts, and goes back to as a match is won.
const STARTS: [Vec2; 2] = [Vec2::new(-5.0, 0.0), Vec2::new(5.0, 0.0)];

/// How far across a player's disc is.
const PLAYER_SIZE: f32 = 0.8;

/// How far across a shard is, corner to corner.
const SHARD_SIZE: f32 = 0.5;

/// How many shards the arena holds at once.
const SHARDS: usize = 3;

/// How fast a player goes, in units a second, and how much of the way from how fast it goes to
/// that it gets each second.
const SPEED: f32 = 6.0;
const GRIP: f32 = 8.0;

/// A dash: how many times faster a player goes, for how many ticks, and how many ticks until the
/// next.
const DASH_SPEED: f32 = 2.5;
const DASH_TICKS: u32 = 10;
const DASH_EVERY: u32 = 64;

/// How many shards win a match, as the arena's Target says.
const WINS_AT: u32 = 10;

/// How fast shards turn, in radians a second.
const SHARD_SPIN: f32 = 1.5;

/// The z of what's drawn, from the back: the floor, the walls, the shards and the players.
const FLOOR_Z: f32 = 0.0;
const SHARD_Z: f32 = 0.2;
const PLAYER_Z: f32 = 0.3;

/// A player's disc: which player steers it, which way it's steered, and its dash, the ticks left
/// of it and until the next.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Player {
    index: u8,
    heading: Vec2,
    dash: u32,
    cooldown: u32,
}

/// A shard to collect, and how far it has turned.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Shard {
    #[reflect(degrees)]
    angle: f32,
}

/// The match: each player's shards and matches won, and the state of the random numbers that place
/// the shards, which the world holds, so that every peer, replay and restored run places them alike.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Match {
    scores: [u32; 2],
    wins: [u32; 2],
    random: u64,
}

/// What the players' keys and pads do, queued as commands for the next tick, each naming its
/// player, which a recording and a lockstep peer keep.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Has the player go this way, until the next.
    Steer { player: u8, heading: Vec2 },
    /// Has the player dash, if it can.
    Dash { player: u8 },
}

/// What the keys and pads do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// W, A, S and D, the arrows, the left stick and the D-pad.
    Steer,
    /// Space and South.
    Dash,
}

/// A corner of the floor's square, which `shaders/floor.slang` reads: its position in the square's
/// own space, and where it is across the square, from 0 to 1.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Corner {
    position: Vec3,
    uv: Vec2,
}

/// What the floor's shader draws it in, as `shaders/floor.slang` reads it: its color and its
/// lines', in linear RGBA; how many cells it's crossed into, across and down; and how many pixels
/// wide its lines are.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Floor {
    color: [f32; 4],
    lines: [f32; 4],
    cells: [f32; 2],
    width: f32,
    pad: f32,
}

/// The game's layer: it builds the arena as it attaches, applies each player's commands at each
/// tick, steering the discs, which 2D physics moves, and collecting the shards they touch; and
/// draws the floor and the scores between ticks.
struct Shards {
    /// The player this run's keys steer: 0, unless it joined a lockstep session.
    player: u8,
    actions: Actions<Act>,
    commands: Vec<Command>,
    /// The floor's square and shader, once the game loads.
    floor: Option<(Mesh<Corner>, Shader<Corner, Floor>)>,
    /// The font the scores are drawn in, once the game loads.
    font: Option<Font>,
    /// The scores' text, written again in its memory each frame.
    text: String,
}

impl Shards {
    fn new() -> Self {
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
            (Act::Steer, Binding::WASD),
            (Act::Steer, arrows),
            (Act::Steer, Binding::stick(Stick::Left)),
            (Act::Steer, d_pad),
            (Act::Dash, Binding::Button(Source::Key(Key::Space))),
            (
                Act::Dash,
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
            floor: None,
            font: None,
            text: String::new(),
        }
    }
}

impl Layer for Shards {
    fn load(&mut self, cx: &mut Context) {
        // A shader that doesn't compile stops the game, with what went wrong.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let shader = cx.add_shader("floor", root.join("shaders/floor.slang"), Blend::Opaque);
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|[u, v]| Corner {
            position: Vec3::new(u - 0.5, v - 0.5, 0.0),
            uv: Vec2::new(u, v),
        });
        let square = cx.add_mesh("floor", &corners, &[0, 1, 2, 0, 2, 3]);
        self.floor = Some((
            square.expect("the floor is two whole triangles"),
            shader.expect("the floor's shader compiles"),
        ));
        let font = cx.add_font("Inter Bold", root.join("fonts/Inter-Bold.ttf"));
        self.font = Some(font.expect("the game's font bakes"));
        register(cx.world_mut());
    }

    fn attach(&mut self, cx: &mut Context) {
        self.player = cx.player().unwrap_or(0);
        // The arena, watched as the game runs, in development builds, which apply its saves.
        let loaded = cx.load_start_scene();
        loaded.unwrap_or_else(|error| panic!("{error:#}"));
        build(cx.world_mut());
    }

    /// Takes back which player this run steers, which a joiner's world, the host's, doesn't say,
    /// and lets go of what may have been held as the world was taken.
    fn restored(&mut self, cx: &mut Context) {
        self.player = cx.player().unwrap_or(0);
        let player = self.player;
        self.commands.push(Command::Steer {
            player,
            heading: Vec2::ZERO,
        });
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        let _zone = gg_engine::zone!();
        for command in self.commands.drain(..) {
            let (player, dash) = match command {
                Command::Steer { player, heading } => (player, Some(heading)),
                Command::Dash { player } => (player, None),
            };
            let players = world.query_mut::<&mut Player>();
            for disc in players.filter(|disc| disc.index == player) {
                match dash {
                    Some(heading) => disc.heading = heading.clamp_length_max(1.0),
                    None if disc.cooldown == 0 => {
                        disc.dash = DASH_TICKS;
                        disc.cooldown = DASH_EVERY;
                    }
                    None => {}
                }
            }
        }
        steer(world, Tick::DT);
        step_physics(world);
        collect(world);
        spin(world, Tick::DT);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    fn update(&mut self, cx: &mut Context, _: Duration) {
        if let Some((square, shader)) = self.floor {
            let floor = Floor {
                color: [0.03, 0.035, 0.05, 1.0],
                lines: [0.07, 0.08, 0.11, 1.0],
                cells: [ARENA.x * 2.0, ARENA.y * 2.0],
                width: 1.0,
                pad: 0.0,
            };
            let at = Mat4::from_scale_rotation_translation(
                (ARENA * 2.0).extend(1.0),
                Quat::IDENTITY,
                Vec3::Z * FLOOR_Z,
            );
            cx.draw(square, shader, at, floor);
        }
        let Some(&found) = cx.world().query::<&Match>().next() else {
            return;
        };
        if let Some(font) = self.font {
            draw_scores(cx, &found, font, &mut self.text);
        }
    }

    fn event(&mut self, _: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, rather than at the next update, so that the next tick applies them.
        let Self {
            player,
            actions,
            commands,
            ..
        } = self;
        let player = *player;
        actions.event(event, |change| match change {
            Change::Axis2(Act::Steer, heading) => commands.push(Command::Steer { player, heading }),
            Change::Pressed(Act::Dash) => commands.push(Command::Dash { player }),
            _ => {}
        });
        ControlFlow::Continue(())
    }
}

/// Registers the game's components with `world`, so that its scene and the tool API know them
/// before any entity has one.
fn register(world: &mut World) {
    let registered = world.register::<Player>().and_then(|()| {
        world.register::<Shard>()?;
        world.register::<Match>()
    });
    registered.expect("the game's components describe themselves");
}

/// Builds what the arena's scene doesn't hold: the camera, the match, the players at their starts
/// and the first shards, without gravity, as the arena is seen from above.
fn build(world: &mut World) {
    let mut physics = Physics2D::default();
    physics.gravity = Vec2::ZERO;
    world.insert_resource(physics);
    let camera = Camera {
        projection: Projection::Orthographic {
            height: VIEW_HEIGHT,
            near: 0.1,
            far: 100.0,
        },
        primary: true,
    };
    let above = Transform {
        translation: Vec3::Z * 10.0,
        ..Transform::IDENTITY
    };
    world.spawn((above, camera, Name("Camera".to_owned())));
    world.spawn((
        Match {
            scores: [0; 2],
            wins: [0; 2],
            random: 0x5_4a2d_5eed,
        },
        Name("Match".to_owned()),
    ));
    for (index, start) in STARTS.into_iter().enumerate() {
        let at = Transform {
            translation: start.extend(PLAYER_Z),
            scale: Vec3::splat(PLAYER_SIZE),
            ..Transform::IDENTITY
        };
        let body = RigidBody2D {
            body_type: BodyType::Dynamic,
            fixed_rotation: true,
            ..RigidBody2D::default()
        };
        let collider = CircleCollider2D {
            restitution: 0.6,
            friction: 0.0,
            ..CircleCollider2D::default()
        };
        let disc = Circle {
            color: Vec3::from(COLORS[index]).extend(1.0),
            ..Circle::default()
        };
        let player = Player {
            index: index as u8,
            heading: Vec2::ZERO,
            dash: 0,
            cooldown: 0,
        };
        let name = Name(format!("Player {}", index + 1));
        world.spawn((at, body, collider, disc, player, name));
    }
    for _ in 0..SHARDS {
        let at = Transform {
            scale: Vec3::new(SHARD_SIZE, SHARD_SIZE, 1.0) * std::f32::consts::FRAC_1_SQRT_2,
            ..Transform::IDENTITY
        };
        let sprite = Sprite {
            color: Vec4::new(0.95, 0.85, 0.3, 1.0),
            ..Sprite::default()
        };
        let shard = world.spawn((at, sprite, Shard { angle: 0.0 }));
        place(world, shard);
    }
}

/// Moves `shard` to where the match's random numbers say next, clear of the players.
fn place(world: &mut World, shard: Entity) {
    let players: Vec<Vec2> = world
        .query::<(&Player, &Transform)>()
        .map(|(_, at)| at.translation.truncate())
        .collect();
    let Some(found) = world.query_mut::<&mut Match>().next() else {
        return;
    };
    let mut at = Vec2::ZERO;
    // Up to 8 tries for a place clear of the players, then the last.
    for _ in 0..8 {
        let inside = ARENA - Vec2::splat(1.0);
        let x = (unit(&mut found.random) * 2.0 - 1.0) * inside.x;
        let y = (unit(&mut found.random) * 2.0 - 1.0) * inside.y;
        at = Vec2::new(x, y);
        if players.iter().all(|player| player.distance(at) > 1.5) {
            break;
        }
    }
    if let Some(transform) = world.get_mut::<Transform>(shard) {
        transform.translation = at.extend(SHARD_Z);
    }
}

/// The next of `random`'s numbers, SplitMix64's, from 0 up to 1, in steps of 2⁻²⁴.
fn unit(random: &mut u64) -> f32 {
    *random = random.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *random;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f32 / (1 << 24) as f32
}

/// Sets each player's velocity, `dt` on, toward its heading at its speed, faster while it dashes,
/// counting its dash down.
fn steer(world: &mut World, dt: Duration) {
    let dt = dt.as_secs_f32();
    for (player, body) in world.query_mut::<(&mut Player, &mut RigidBody2D)>() {
        let speed = if player.dash > 0 {
            SPEED * DASH_SPEED
        } else {
            SPEED
        };
        let wanted = player.heading * speed;
        // A dash goes at once; otherwise the disc slides toward where it's steered.
        let grip = if player.dash > 0 {
            1.0
        } else {
            (GRIP * dt).min(1.0)
        };
        body.velocity += (wanted - body.velocity) * grip;
        player.dash = player.dash.saturating_sub(1);
        player.cooldown = player.cooldown.saturating_sub(1);
    }
}

/// Gives each shard a player touches to that player, placing it again, and, at the shard that wins
/// a match, gives the match to its player and starts another, the players back at their starts,
/// the rest touched that tick left where they are.
fn collect(world: &mut World) {
    let players: Vec<(u8, Vec2)> = world
        .query::<(&Player, &Transform)>()
        .map(|(player, at)| (player.index, at.translation.truncate()))
        .collect();
    let touched: Vec<(Entity, u8)> = world
        .query::<(Entity, &Shard, &Transform)>()
        .filter_map(|(shard, _, at)| {
            let at = at.translation.truncate();
            let reach = (PLAYER_SIZE + SHARD_SIZE) / 2.0;
            let by = players
                .iter()
                .find(|(_, player)| player.distance(at) < reach);
            by.map(|&(index, _)| (shard, index))
        })
        .collect();
    let mut won = None;
    for (shard, index) in touched {
        if won.is_some() {
            break;
        }
        if let Some(found) = world.query_mut::<&mut Match>().next() {
            let index = usize::from(index);
            found.scores[index] += 1;
            if found.scores[index] >= WINS_AT {
                found.wins[index] += 1;
                found.scores = [0; 2];
                won = Some(index);
            }
        }
        place(world, shard);
    }
    if won.is_some() {
        let players = world.query_mut::<(&Player, &mut Transform, &mut RigidBody2D)>();
        for (player, at, body) in players {
            at.translation = STARTS[usize::from(player.index)].extend(PLAYER_Z);
            body.velocity = Vec2::ZERO;
        }
    }
}

/// Turns each shard `dt` on.
fn spin(world: &mut World, dt: Duration) {
    for (shard, at) in world.query_mut::<(&mut Shard, &mut Transform)>() {
        shard.angle = (shard.angle + SHARD_SPIN * dt.as_secs_f32()) % std::f32::consts::TAU;
        at.rotation = Quat::from_rotation_z(shard.angle + std::f32::consts::FRAC_PI_4);
    }
}

/// Draws the match's scores above the arena, in `font`, writing each text into `text`: each
/// player's shards in its color at its side, and the matches it has won beside them, toward the
/// middle, where the arena's Target says how many shards win a match.
fn draw_scores(cx: &mut Context, found: &Match, font: Font, text: &mut String) {
    // Between the top wall and the view's top, as the Target is.
    let y = ARENA.y + 0.9;
    let at = |x: f32| Mat4::from_translation(Vec3::new(x, y, 0.5));
    let style = |size: f32, color: Vec4, across: Align| TextStyle {
        font: Some(font),
        size,
        color,
        align: [across, Align::Center],
    };
    for (index, &color) in COLORS.iter().enumerate() {
        // Each side's text grows from its edge toward the middle.
        let (side, across) = if index == 0 {
            (-1.0, Align::Start)
        } else {
            (1.0, Align::End)
        };
        let color = Vec3::from(color);
        let write = |text: &mut String, value: u32, then: &str| {
            text.clear();
            write!(text, "{value}{then}").expect("a string takes any text");
        };
        write(text, found.scores[index], "");
        let edge = side * ARENA.x;
        cx.draw_text(at(edge), text, &style(0.55, color.extend(1.0), across));
        let won = found.wins[index];
        write(text, won, if won == 1 { " WIN" } else { " WINS" });
        let beside = style(0.3, color.extend(0.7), across);
        cx.draw_text(at(edge - side * 0.65), text, &beside);
    }
}

#[cfg(test)]
mod tests;
