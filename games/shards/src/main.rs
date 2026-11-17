//! Shards, the engine's first demo: two players, each a disc in an arena, steer, dash into each
//! other and collect shards, first to ten. The match's own random numbers place the shards.
//!
//! Alone, the keys steer player 0. Networked, each run's keys steer its own player:
//!
//! - in lockstep, a run given `--host <address>` is player 0, and one given `--join <address>`
//!   player 1;
//! - as clients of a run given `--serve <address>`, each run given `--connect <address>` steers
//!   the player the server gives it, each disc its player's [`Owner`].
//!
//! A joiner or client also takes the session's `--code`, which its host or server prints. Behind
//! routers, each run takes `--relay <address>`, a relay `gg-relay` runs, a joiner or client in
//! place of its address.
//!
//! What it shows:
//!
//! - its walls, the scene `scenes/arena.ron`, which development builds apply as it's saved;
//! - the shards' spin, `scripts/spin.luau`, and the floor, `shaders/floor.slang`, each reloading
//!   as it's saved;
//! - mods, which read the world and write sprites: given `--mods mods`, `mods/rainbow` cycles the
//!   shards' colors;
//! - scores as text in `fonts/Inter-Bold.ttf` on the view's canvas;
//! - a menu, from Escape or a pad's Start, over the match, which goes on meanwhile;
//! - sounds, `sounds/*.wav`, placed where they happen, and music, `music/match.ogg`, quieter under
//!   the menu.

// A build without `dev`, as it ships, opens no console on Windows. Its JSON log still records the
// run, and output piped to a tool or CI still arrives.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::fmt::Write as _;
use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use bytemuck::NoUninit;
use gg_engine::math::DVec3;
use gg_engine::prelude::*;
use gg_engine::{
    BodyType, Camera, Circle, CircleCollider2D, Grant, Mods, Name, Owner, Physics2D, RigidBody2D,
};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, as `main` runs it and its recordings replay it.
fn app() -> App {
    let app = App::new("Shards")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
    let app = app.layer(Shards::new()).overlay(gg_engine::PerfPanel);
    app.overlay(gg_engine::Inspector::default())
}

/// How far the arena's inside reaches from its middle, across and up.
const ARENA: Vec2 = Vec2::new(8.0, 4.5);

/// How high the view is, in units: the arena, its walls and its Target above it.
const VIEW_HEIGHT: f32 = 11.5;

/// Each player's color, in linear RGB: blue and orange.
const COLORS: [[f32; 3]; 2] = [[0.15, 0.55, 1.0], [1.0, 0.45, 0.1]];

/// Where each player starts, and goes back to as a match is won.
const STARTS: [Vec2; 2] = [Vec2::new(-5.0, 0.0), Vec2::new(5.0, 0.0)];

const PLAYER_SIZE: f32 = 0.8;

/// Corner to corner.
const SHARD_SIZE: f32 = 0.5;

/// How many shards the arena holds at once.
const SHARDS: usize = 3;

/// A player's speed, in units a second, and the share of the way to it that it gains each second.
const SPEED: f32 = 6.0;
const GRIP: f32 = 8.0;

/// A dash: how many times faster a player goes, for how many ticks, and the ticks until the next.
const DASH_SPEED: f32 = 2.5;
const DASH_TICKS: u32 = 10;
const DASH_EVERY: u32 = 64;

/// How many shards win a match, as the arena's Target says.
const WINS_AT: u32 = 10;

/// The z of what's drawn, from the back: the floor, the walls, the shards and the players.
const FLOOR_Z: f32 = 0.0;
const SHARD_Z: f32 = 0.2;
const PLAYER_Z: f32 = 0.3;

/// The scores, in canvas units: their inset from the view's top corners, and the em of a score
/// and of its matches won, below it.
const HUD_INSET: f32 = 40.0;
const SCORE_SIZE: f32 = 80.0;
const WINS_SIZE: f32 = 32.0;

/// Top to bottom.
const ITEMS: [(Item, &str); 3] = [
    (Item::Resume, "RESUME"),
    (Item::NewMatch, "NEW MATCH"),
    (Item::Quit, "QUIT"),
];

/// The menu, in canvas units from the view's middle: each button's size, the first item's middle
/// above the view's, the step down to each next, and the title's middle.
const BUTTON: Vec2 = Vec2::new(440.0, 84.0);
const FIRST_ITEM: f32 = 50.0;
const ITEM_STEP: f32 = 104.0;
const TITLE: f32 = 180.0;

/// The music's fade in as the game loads, its volume under the menu (about -9 dB), and how long
/// it takes to get there and back.
const MUSIC_FADE_IN: Duration = Duration::from_secs(2);
const MUSIC_UNDER_MENU: f32 = 0.35;
const MUSIC_DUCK: Duration = Duration::from_millis(300);

/// A player's disc: which player steers it, its heading, and the ticks left of its dash and until
/// the next.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Player {
    index: u8,
    heading: Vec2,
    dash: u32,
    cooldown: u32,
}

/// A shard to collect, and its turn.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Shard {
    #[reflect(degrees)]
    angle: f32,
}

/// The match: each player's shards and matches won, and the state of the random numbers placing
/// shards. The world holds it, so every peer, replay and restored run places them alike.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Match {
    scores: [u32; 2],
    wins: [u32; 2],
    random: u64,
}

/// What the players' keys and pads do, applied at the next tick, each naming its player.
/// Recordings and lockstep peers keep them.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Has the player go this way, until the next.
    Steer { player: u8, heading: Vec2 },
    /// Has the player dash, if it can.
    Dash { player: u8 },
    /// Starts a match afresh for both players: the menu's New match.
    NewMatch,
}

/// What the keys and pads do, as [`Shards::new`] binds them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Steer,
    Dash,
    Menu,
    Up,
    Down,
    Choose,
}

/// What the menu's items do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    /// Closes the menu.
    Resume,
    /// Closes it, and queues [`Command::NewMatch`].
    NewMatch,
    /// Ends the run.
    Quit,
}

/// A corner of the floor's square, laid out as `shaders/floor.slang` reads it.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Corner {
    position: Vec3,
    uv: Vec2,
}

/// The floor's look, laid out as `shaders/floor.slang` reads it: its color and its lines', linear
/// RGBA; its cells across and down; and its lines' width, in pixels.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Floor {
    color: [f32; 4],
    lines: [f32; 4],
    cells: [f32; 2],
    width: f32,
    pad: f32,
}

/// The game's sounds, from `sounds/`.
#[derive(Clone, Copy)]
struct Sounds {
    dash: Sound,
    collect: Sound,
    win: Sound,
    focus: Sound,
    choose: Sound,
}

/// The game's layer. An asset or script of its own, or the start scene, that doesn't load ends the
/// run.
struct Shards {
    /// The player this run's keys steer: 0, unless it joined a lockstep session or a server gave
    /// it another.
    player: u8,
    actions: Actions<Act>,
    commands: Vec<Command>,
    /// The menu's focused item, while it's open. It's this run's own, as each lockstep peer's is.
    menu: Option<usize>,
    /// Once loaded.
    floor: Option<(Mesh<Corner>, Shader<Corner, Floor>)>,
    /// The scores', once loaded.
    font: Option<Font>,
    /// Once loaded.
    sounds: Option<Sounds>,
    /// The shards' spin.
    scripts: Scripts,
    /// The installed mods, once loaded.
    mods: Option<Mods>,
    /// The scores' text, rewritten in place each frame.
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
        let key = |key| Binding::Button(Source::Key(key));
        let pad = |button| Binding::Button(Source::PadButton(button));
        let bound = [
            (Act::Steer, Binding::WASD),
            (Act::Steer, arrows),
            (Act::Steer, Binding::stick(Stick::Left)),
            (Act::Steer, d_pad),
            (Act::Dash, key(Key::Space)),
            (Act::Dash, pad(PadButton::South)),
            (Act::Menu, key(Key::Escape)),
            (Act::Menu, pad(PadButton::Start)),
            (Act::Up, key(Key::KeyW)),
            (Act::Up, key(Key::ArrowUp)),
            (Act::Up, pad(PadButton::DPadUp)),
            (Act::Down, key(Key::KeyS)),
            (Act::Down, key(Key::ArrowDown)),
            (Act::Down, pad(PadButton::DPadDown)),
            (Act::Choose, key(Key::Enter)),
            (Act::Choose, key(Key::Space)),
            (Act::Choose, pad(PadButton::South)),
        ]
        .into_iter()
        .try_for_each(|(action, binding)| actions.bind(action, binding));
        bound.expect("the game's bindings are each of one kind, of other sources");
        Self {
            player: 0,
            actions,
            commands: Vec::new(),
            menu: None,
            floor: None,
            font: None,
            sounds: None,
            scripts: Scripts::new().expect("Luau starts"),
            mods: None,
            text: String::new(),
        }
    }
}

impl Layer for Shards {
    /// Loads the game's assets and script, and the installed mods. Fails with the first of the
    /// game's own that doesn't load, as a shader that doesn't compile.
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let shader = cx.add_shader("floor", root.join("shaders/floor.slang"), Blend::Opaque)?;
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|[u, v]| Corner {
            position: Vec3::new(u - 0.5, v - 0.5, 0.0),
            uv: Vec2::new(u, v),
        });
        let square = cx.add_mesh("floor", &corners, &[0, 1, 2, 0, 2, 3]);
        self.floor = Some((square.expect("the floor is two whole triangles"), shader));
        self.font = Some(cx.add_font("Inter Bold", root.join("fonts/Inter-Bold.ttf"))?);
        let sound = |cx: &mut Context, name: &str| {
            cx.add_sound(name, root.join(format!("sounds/{name}.wav")))
        };
        self.sounds = Some(Sounds {
            dash: sound(cx, "dash")?,
            collect: sound(cx, "collect")?,
            win: sound(cx, "win")?,
            focus: sound(cx, "focus")?,
            choose: sound(cx, "choose")?,
        });
        let music = cx.add_music("match", root.join("music/match.ogg"))?;
        cx.play_music(Some(music), MUSIC_FADE_IN);
        register(cx.world_mut())?;
        self.scripts.load(cx, root.join("scripts/spin.luau"))?;
        // Mods read the world, and write sprites alone, as the rainbow writes the shards' colors.
        let grant = Grant::parse(["read", "write:Sprite"]).expect("capabilities");
        self.mods = Some(Mods::load(cx, grant));
        Ok(())
    }

    fn attach(&mut self, cx: &mut Context) {
        self.player = cx.player().unwrap_or(0);
        // Development builds watch the arena and apply its saves.
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
            return;
        }
        build(cx.world_mut());
    }

    /// Takes back which player this run steers, which the world a joiner or client takes from its
    /// host or server doesn't say. Lets go of what was held as the world was taken.
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
                Command::NewMatch => {
                    if let Some(found) = world.query_mut::<&mut Match>().next() {
                        found.scores = [0; 2];
                        found.wins = [0; 2];
                    }
                    to_starts(world);
                    continue;
                }
            };
            let mut dashed = None;
            let players = world.query_mut::<(Entity, &mut Player)>();
            for (entity, disc) in players.filter(|(_, disc)| disc.index == player) {
                match dash {
                    Some(heading) => disc.heading = heading.clamp_length_max(1.0),
                    None if disc.cooldown == 0 => {
                        disc.dash = DASH_TICKS;
                        disc.cooldown = DASH_EVERY;
                        dashed = Some(entity);
                    }
                    None => {}
                }
            }
            if let (Some(sounds), Some(disc)) = (self.sounds, dashed) {
                world.play_sound(sounds.dash, Place::On(disc));
            }
        }
        steer(world, Tick::DT);
        step_physics(world);
        collect(world, self.sounds);
        self.scripts.run(world);
        if let Some(mods) = &mut self.mods {
            mods.run(world);
        }
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
            match self.menu {
                Some(focus) => {
                    // A server's client past the two players has no disc, nor color, of its own.
                    let lit = COLORS.get(usize::from(self.player)).copied();
                    draw_menu(cx, font, focus, lit.unwrap_or([1.0; 3]));
                }
                None => draw_hint(cx, font),
            }
        }
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, not at the next update, so the next tick applies them.
        let Self {
            player,
            actions,
            commands,
            menu,
            sounds,
            ..
        } = self;
        let (player, sounds, was) = (*player, *sounds, *menu);
        // Whether the menu opens or closes, and what's chosen in it.
        let (mut toggled, mut chosen) = (false, None);
        actions.event(event, |change| match (*menu, change) {
            (_, Change::Pressed(Act::Menu)) => toggled = true,
            (None, Change::Axis2(Act::Steer, heading)) => {
                commands.push(Command::Steer { player, heading });
            }
            (None, Change::Pressed(Act::Dash)) => commands.push(Command::Dash { player }),
            (Some(focus), Change::Pressed(Act::Up)) => {
                *menu = Some((focus + ITEMS.len() - 1) % ITEMS.len());
            }
            (Some(focus), Change::Pressed(Act::Down)) => *menu = Some((focus + 1) % ITEMS.len()),
            (Some(focus), Change::Pressed(Act::Choose)) => chosen = Some(ITEMS[focus].0),
            _ => {}
        });
        // The mouse over an item focuses it, and a click chooses it.
        if let Some(focus) = menu {
            let over = cx.canvas([Align::Center; 2]).cursor().and_then(item_at);
            match (event, over) {
                (Event::CursorMoved { .. }, Some(over)) => *focus = over,
                (
                    Event::MouseButton {
                        button: MouseButton::Left,
                        pressed: true,
                    },
                    Some(over),
                ) => chosen = Some(ITEMS[over].0),
                _ => {}
            }
        }
        match chosen {
            Some(Item::NewMatch) => commands.push(Command::NewMatch),
            Some(Item::Quit) => cx.exit(),
            _ => {}
        }
        let resumed = matches!(chosen, Some(Item::Resume | Item::NewMatch));
        if resumed || toggled && menu.is_some() {
            *menu = None;
            // As the keys and sticks are held now: the disc let go as the menu opened.
            let heading = actions.axis2(Act::Steer);
            commands.push(Command::Steer { player, heading });
        } else if toggled {
            *menu = Some(0);
            let heading = Vec2::ZERO;
            commands.push(Command::Steer { player, heading });
        }
        // This run's own, as its menu is.
        if let Some(sounds) = sounds {
            if chosen.is_some() {
                cx.play_sound(sounds.choose, Place::Nowhere);
            } else if *menu != was {
                cx.play_sound(sounds.focus, Place::Nowhere);
            }
        }
        // The music, quieter under this run's menu.
        let music = if menu.is_some() {
            MUSIC_UNDER_MENU
        } else {
            1.0
        };
        cx.set_volume(Bus::Music, music, MUSIC_DUCK);
        ControlFlow::Continue(())
    }
}

/// Registers the game's components, so its scene and the tool API know them before any entity has
/// one.
fn register(world: &mut World) -> Result<(), Error> {
    world.register::<Player>()?;
    world.register::<Shard>()?;
    world.register::<Match>()
}

/// Builds what the arena's scene doesn't hold: the camera, the match, the players and the first
/// shards. No gravity, as the arena is seen from above.
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
        translation: DVec3::Z * 10.0,
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
            translation: start.extend(PLAYER_Z).as_dvec3(),
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
        let owner = Owner {
            player: index as u8,
        };
        world.spawn((at, body, collider, disc, player, owner, name));
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
        // Its spin, which its script declares.
        world
            .add_default(shard, "Spin")
            .expect("the game's scripts loaded");
        place(world, shard);
    }
}

/// Moves `shard` to where the match's random numbers say next, clear of the players.
fn place(world: &mut World, shard: Entity) {
    let players: Vec<Vec2> = world
        .query::<(&Player, &Transform)>()
        .map(|(_, at)| at.translation.truncate().as_vec2())
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
        transform.translation = at.extend(SHARD_Z).as_dvec3();
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

/// Moves each player's velocity, over `dt`, toward its heading at its speed, faster while it
/// dashes, and counts its dash down.
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

/// Scores each shard a player touches and places it again, chiming `sounds`' collect where it was.
/// The shard that wins a match gives it to its player and starts another, with the players back at
/// their starts, the rest touched that tick left, and `sounds`' fanfare.
fn collect(world: &mut World, sounds: Option<Sounds>) {
    let players: Vec<(u8, Vec2)> = world
        .query::<(&Player, &Transform)>()
        .map(|(player, at)| (player.index, at.translation.truncate().as_vec2()))
        .collect();
    let touched: Vec<(Entity, u8)> = world
        .query::<(Entity, &Shard, &Transform)>()
        .filter_map(|(shard, _, at)| {
            let at = at.translation.truncate().as_vec2();
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
        let at = world.get::<Transform>(shard).map(|at| at.translation);
        if let (Some(sounds), Some(at)) = (sounds, at) {
            world.play_sound(sounds.collect, Place::At(at));
        }
        place(world, shard);
    }
    if won.is_some() {
        to_starts(world);
        if let Some(sounds) = sounds {
            world.play_sound(sounds.win, Place::Nowhere);
        }
    }
}

/// Puts each player back at its start, at rest.
fn to_starts(world: &mut World) {
    let players = world.query_mut::<(&Player, &mut Transform, &mut RigidBody2D)>();
    for (player, at, body) in players {
        at.translation = STARTS[usize::from(player.index)]
            .extend(PLAYER_Z)
            .as_dvec3();
        body.velocity = Vec2::ZERO;
    }
}

/// Draws each player's shards, and its matches won below them, in its color in the top corner on
/// its side, growing toward the middle, where the arena's Target says how many shards win. Each
/// text is written into `text`.
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

/// Draws the keys that open the menu at the view's bottom.
fn draw_hint(cx: &mut Context, font: Font) {
    let style = TextStyle {
        font: Some(font),
        size: 28.0,
        color: Vec4::new(1.0, 1.0, 1.0, 0.45),
        align: [Align::Center, Align::End],
    };
    let at = Mat4::from_translation(Vec3::new(0.0, 24.0, 0.0));
    let mut canvas = cx.canvas([Align::Center, Align::End]);
    canvas.draw_text(at, "ESC / START: MENU", &style);
}

/// Draws the menu in the view's middle: a dark panel, its title, and each item's button, `focus`'s
/// lit in `lit`, linear RGB.
fn draw_menu(cx: &mut Context, font: Font, focus: usize, lit: [f32; 3]) {
    let mut canvas = cx.canvas([Align::Center; 2]);
    let rect = |middle: Vec2, size: Vec2| {
        Mat4::from_translation(middle.extend(0.0)) * Mat4::from_scale(size.extend(1.0))
    };
    let filled = |color: Vec4| Sprite {
        color,
        ..Sprite::default()
    };
    let panel = filled(Vec4::new(0.01, 0.012, 0.02, 0.92));
    canvas.draw_sprite(rect(Vec2::new(0.0, 10.0), Vec2::new(560.0, 480.0)), panel);
    let style = |size: f32, color: Vec4| TextStyle {
        font: Some(font),
        size,
        color,
        align: [Align::Center; 2],
    };
    let at = |middle: Vec2| Mat4::from_translation(middle.extend(0.0));
    let title = style(64.0, Vec4::ONE);
    canvas.draw_text(at(Vec2::new(0.0, TITLE)), "SHARDS", &title);
    for (index, &(_, label)) in ITEMS.iter().enumerate() {
        let middle = item_middle(index);
        let (fill, ink) = if index == focus {
            (Vec3::from(lit).extend(1.0), Vec4::ONE)
        } else {
            (
                Vec4::new(1.0, 1.0, 1.0, 0.06),
                Vec4::new(1.0, 1.0, 1.0, 0.7),
            )
        };
        canvas.draw_sprite(rect(middle, BUTTON), filled(fill));
        canvas.draw_text(at(middle), label, &style(36.0, ink));
    }
}

/// Item `index`'s middle on the canvas, from the view's middle.
fn item_middle(index: usize) -> Vec2 {
    Vec2::new(0.0, FIRST_ITEM - ITEM_STEP * index as f32)
}

/// The item whose button is at `at` on the canvas, from the view's middle.
fn item_at(at: Vec2) -> Option<usize> {
    let over = |index| (at - item_middle(index)).abs().cmple(BUTTON / 2.0).all();
    (0..ITEMS.len()).find(|&index| over(index))
}

#[cfg(test)]
mod tests;
