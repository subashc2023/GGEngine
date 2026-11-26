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

mod components;
mod systems;
mod ui;

use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use bytemuck::NoUninit;
#[cfg(doc)]
use gg_engine::Owner;
use gg_engine::prelude::*;
use gg_engine::{Grant, Mods};

use components::{Match, Player, register};
use systems::{build, collect, steer, to_starts};
use ui::{draw_hint, draw_menu, draw_scores, item_at};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, as `main` runs it and its recordings replay it.
fn app() -> App {
    let app = App::new("Shards")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
    app.layer(Shards::new()).debug_overlays()
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

/// Top to bottom.
const ITEMS: [(Item, &str); 3] = [
    (Item::Resume, "RESUME"),
    (Item::NewMatch, "NEW MATCH"),
    (Item::Quit, "QUIT"),
];

/// The music's fade in as the game loads, its volume under the menu (about -9 dB), and how long
/// it takes to get there and back.
const MUSIC_FADE_IN: Duration = Duration::from_secs(2);
const MUSIC_UNDER_MENU: f32 = 0.35;
const MUSIC_DUCK: Duration = Duration::from_millis(300);

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
        let d_pad = Binding::Axis2 {
            x: [PadButton::DPadLeft, PadButton::DPadRight].map(Source::PadButton),
            y: [PadButton::DPadDown, PadButton::DPadUp].map(Source::PadButton),
        };
        let key = |key| Binding::Button(Source::Key(key));
        let pad = |button| Binding::Button(Source::PadButton(button));
        let bound = actions.bind_all([
            (Act::Steer, Binding::WASD),
            (Act::Steer, Binding::ARROWS),
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
        ]);
        bound.expect("the game's bindings are each of one kind, of other sources");
        // What `gg actions` says each control does.
        let steer = format!(
            "Steers the player's disc: [x, y], right and up, and 1 is {SPEED} units a second. The \
             disc slides toward that velocity. It does nothing while the menu is open"
        );
        actions.describe(Act::Steer, steer);
        let dash = format!(
            "Dashes the player's disc along its steering at {DASH_SPEED} times its speed, at once, \
             for {DASH_TICKS} ticks. The next dash is ready {DASH_EVERY} ticks after the last, and \
             a press before that does nothing. It does nothing while the menu is open"
        );
        actions.describe(Act::Dash, dash);
        let menu = "Opens the menu over the match, which goes on, and lets go of the disc; a press \
                    with the menu open closes it and the disc steers again";
        actions.describe(Act::Menu, menu);
        let up = "In the menu, moves the focus up one item, wrapping from the first to the last. \
                  Outside it, does nothing as an action";
        actions.describe(Act::Up, up);
        let down = "In the menu, moves the focus down one item, wrapping from the last to the \
                    first. Outside it, does nothing as an action";
        actions.describe(Act::Down, down);
        let choose = "In the menu, chooses the focused item: Resume closes the menu, New match \
                      zeroes both players' scores and wins, puts both discs at their starts and \
                      closes it, and Quit ends the run. Outside it, does nothing";
        actions.describe(Act::Choose, choose);
        let over = "In the menu, the cursor over an item focuses it";
        actions.describe_control(Control::Cursor, over);
        let click = Control::Source(Source::Button(MouseButton::Left));
        actions.describe_control(
            click,
            "In the menu, a press with the cursor over an item chooses it, as Choose does",
        );
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
    /// host or server doesn't say. Lets go of what was held as the world was taken, and closes the
    /// menu, as after its Quit stops the editor's Play.
    fn restored(&mut self, cx: &mut Context) {
        self.menu = None;
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

    /// What `gg actions` lists, and `gg input` presses by name.
    fn actions(&self) -> Controls {
        self.actions.controls()
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

#[cfg(test)]
mod tests;
