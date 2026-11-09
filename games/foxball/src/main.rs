//! Foxball, the engine's networked 3D demo: two players, each a fox in a walled arena, knocking a
//! ball into the other's goal, first to five. Run alone, the keys, a pad and the mouse steer player
//! 0, and the game's bot plays player 1, its fox running to the ball over the arena's navmesh and
//! kicking it toward player 0's goal; or networked, both foxes their players', in lockstep, a run
//! given `--host <address>` steering player 0 and one given `--join <address>` player 1, each with
//! its own keys; or as clients of a run given `--serve <address>`, each given `--connect <address>`
//! steering the player the server gives it, each fox its player's [`Owner`]. W, A, S and D, or a
//! pad's left stick, walk a fox forward, back and to its sides; the mouse turns it, the cursor
//! grabbed from a click in the view until Escape, as do Q and E and the right stick; and Space, a
//! pad's South, or a click with the cursor grabbed kicks the ball, if it's in front of the fox.
//! Escape with the cursor free, or a pad's Start, opens the menu, `ui/menu.ui`, over the match,
//! which goes on meanwhile, the run's fox standing: Resume, New match, Options and Quit, its
//! options choosing how well the bot plays, a command each player's runs and replays apply as
//! they do New match, and the volumes of the main, music and sound buses and how the view is
//! antialiased, by TAA, SMAA or neither, this run's own, which no world holds. The scores and the
//! keys' hint are the same file's, on the view's canvas. The foxes are Khronos's Fox (`models/fox`,
//! CC0 by PixelMannen, rigged and animated by tomkranis and made glTF by AsoboStudio and scurest
//! under CC BY 4.0, as its README there says), skinned, each played by a state machine, Survey
//! standing and Walk blended with Run by how fast it goes, characters that rapier3d's controller
//! walks, which push the ball as they walk into it, late in an afternoon, under a sky made from its
//! low sun and in a haze that it lights, the view's exposure adapting as it turns toward the sun
//! and away, within a stop either way, through a light vignette; the ball and the arena are 3D
//! physics, the arena's walls, goals and navmesh's volume its scene file, `scenes/arena.ron`, which
//! a development build applies as it's saved, each wall, a goal's pocket's too, an instance of one
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

use gg_engine::math::libm::{atan2f, expf};
use gg_engine::math::{Quat, Vec2, Vec3, Vec4};
use gg_engine::{
    Actions, AnimationParam, AnimationState, AnimationStates, Animator, Antialiasing, App, Binding,
    BlendPoint, Bus, CapsuleCollider3D, Change, CharacterController3D, Commands, Component,
    Context, EmitterShape, Entity, Environment, Error, Event, GameUi, Key, Layer, Model,
    MouseButton, Name, NavAgent, Owner, PadButton, ParamCondition, ParamTest, ParamValue, Parent,
    ParticleBlend, ParticleEmitter, Place, Play, Playback, Projection, Reflect, RigidBody3D, Sound,
    Source, StateMotion, StateTransition, Stick, Tick, Toward, Transform, UiEvent, UiValue,
    Viewpoint, World, step_animation, step_navigation, step_physics,
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

/// A fox's state machine, as [`states`] makes it: the name of its parameter that [`animate`] sets
/// to how fast the fox goes across, in units a second; how fast it goes to leave `Idle` for `Move`,
/// and slower than which it goes back; and the seconds each fades into the other.
const SPEED: &str = "speed";
const MOVING: f32 = 0.25;
const STATE_FADE: f32 = 0.2;

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

/// The player whose fox the game's bot plays in a run alone, whose own keys steer player 0.
const BOT_PLAYER: u8 = 1;

/// How fast the game's bot turns, in radians a second.
const BOT_TURN_SPEED: f32 = 7.0;

/// How well the game's bot plays, which the menu's options choose, as [`Match::difficulty`] has it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
enum Difficulty {
    Easy,
    #[default]
    Normal,
    Hard,
}

impl Difficulty {
    /// Each, in the order of the menu's options.
    const ALL: [Self; 3] = [Self::Easy, Self::Normal, Self::Hard];

    /// How fast the bot runs, in units a second: at Normal a little slower than a player, and at
    /// Hard a little faster.
    fn speed(self) -> f32 {
        match self {
            Self::Easy => 3.8,
            Self::Normal => 4.6,
            Self::Hard => 5.2,
        }
    }

    /// Every how many ticks the bot sees where the ball is, its reaction, heading where that says
    /// until it next does.
    fn reaction(self) -> u32 {
        match self {
            Self::Easy => 12,
            Self::Normal => 6,
            Self::Hard => 3,
        }
    }

    /// How near facing where it aims the bot kicks, as the cosine of the angle it faces away from
    /// it, its aim: the wider, the farther its shots go astray.
    fn aim(self) -> f32 {
        match self {
            Self::Easy => 0.7,
            Self::Normal => 0.85,
            Self::Hard => 0.95,
        }
    }
}

/// Where the bot goes about the ball, in units from the ball's middle: how far behind it, from the
/// goal the bot attacks, it lines up, and how near that counts as there; and how far to the ball's
/// side it keeps as it goes round it to get behind it.
const BOT_BEHIND: f32 = 0.9;
const BOT_LINED: f32 = 0.45;
const BOT_CLEAR: f32 = 1.25;

/// How near the ball the bot faces it, rather than the way it goes, while the ball is ahead of it
/// toward where it aims.
const BOT_CLOSE: f32 = 2.0;

/// Where the bot aims, as [`aim`] chooses: how far to either side of the middle of a goal's mouth
/// it aims at its sides, inside the posts by more than the ball's radius; how far from its way the
/// other foxes must stand for it to shoot, more than a fox's radius and the ball's; and how far to
/// the side of a fox in its way, where every way is, it kicks the ball past it.
const BOT_POST: f32 = 1.3;
const BOT_OPEN: f32 = 0.9;
const BOT_PAST: f32 = 2.2;

/// How far inside the field's walls the bot heads, so that its capsule fits, but into a goal's
/// mouth, where it heads as far as a unit past the field's end, through the ball.
const BOT_MARGIN: f32 = 0.5;

/// The view behind a run's own fox: how far back it is, how high above the fox's middle it looks
/// at, and how far it looks down, as it starts, and the least and most, in radians.
const VIEW_DISTANCE: f32 = 5.0;
const VIEW_ABOVE: f32 = 1.5;
const VIEW_PITCH: f32 = 0.35;
const VIEW_PITCHES: (f32, f32) = (-0.25, 1.2);

/// How quickly the view catches up with its fox, which moves at the ticks, a share of the way left
/// each second as `1 - e^(-rate)`: smoothing the ticks' steps between frames.
const VIEW_FOLLOW: f32 = 14.0;

/// The texts of `ui/menu.ui` that say each player's goals and matches won, by its index.
const SCORES: [(&str, &str); 2] = [("blue_score", "blue_wins"), ("orange_score", "orange_wins")];

/// The sliders of `ui/menu.ui`'s options that set each bus's volume, and the texts that say it.
const VOLUMES: [(&str, &str, Bus); 3] = [
    ("main_volume", "main_volume_value", Bus::Main),
    ("music_volume", "music_volume_value", Bus::Music),
    ("sound_volume", "sound_volume_value", Bus::Sounds),
];

/// How the view is antialiased by each choice of `ui/menu.ui`'s options' drop-down of it, in its
/// order: TAA, SMAA and none.
const ANTIALIASING: [Antialiasing; 3] = [Antialiasing::Taa, Antialiasing::Smaa, Antialiasing::None];

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

/// The game's bot, on the fox it plays, beside the [`NavAgent`] that walks the fox, as
/// [`play_bots`] has it play: from the world as each tick finds it alone, as determinism needs.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Bot {
    /// The ticks until it next sees where the ball is, and chooses where it aims and where its
    /// agent heads.
    wait: u32,
    /// Where it means to kick the ball, as [`aim`] chose last, along the ground.
    aim: Vec3,
}

/// The ball, which the foxes push and kick, and whose emitter bursts as it's kicked.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Ball;

/// A goal's mouth, whose emitter bursts as the ball goes in: the player that scores by it.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Goal {
    player: u8,
}

/// The match: each player's goals in it, and matches won; and how well the game's bot plays, in a
/// run alone, which a new match keeps.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Match {
    scores: [u32; 2],
    wins: [u32; 2],
    difficulty: Difficulty,
}

/// What the players' keys, pads, mice and menus do, and what a run alone sets up, queued as
/// commands for the next tick, each naming its player, which a recording and a lockstep peer keep.
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
    /// Starts a match afresh, for both players, as the player's menu's New match chooses.
    NewMatch { player: u8 },
    /// Has the game's bot play as well as `level` says, as the player's menu's options choose: a
    /// run alone's, as a session has no bot, but which travels as the rest do all the same.
    Difficulty { player: u8, level: Difficulty },
    /// Has the game's bot play the player's fox from then on, as a run alone queues for
    /// [`BOT_PLAYER`] as it attaches, so that its recording keeps it and a session's runs, whose
    /// players are all people, have none.
    Bot { player: u8 },
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
    /// Start, which opens and closes the menu, as Escape does with the cursor free.
    Menu,
}

/// The game's sounds, from its `sounds` folder: a kick, a goal and a match won.
#[derive(Clone, Copy)]
struct Sounds {
    kick: Sound,
    goal: Sound,
    win: Sound,
}

/// What the game loads: its sounds, the fox, and its UI, the scores, the keys' hint and the menu.
struct Assets {
    sounds: Sounds,
    fox: Model,
    ui: GameUi,
}

/// The game's layer: it builds the match and the players' foxes as it attaches, in the arena its
/// start scene holds, and run alone, queues player 1's for its bot; applies each player's commands
/// at each tick, walking, turning and kicking, has the bots play, steps navigation, sets each fox's
/// state machine's speed, steps animation and physics, and counts the goals; and as it updates, has
/// the view follow this run's fox and draws its UI, the scores, the hint and the menu while it's
/// open. An asset of its own, or its start scene, that doesn't load ends the run with its error,
/// through [`Context::fail`].
struct Foxball {
    /// The player this run's keys steer: 0, unless it joined a lockstep session or a server gave
    /// it another.
    player: u8,
    actions: Actions<Act>,
    commands: Vec<Command>,
    /// Once loaded.
    assets: Option<Assets>,
    /// Whether this run wants the cursor grabbed, for the mouse to turn its fox: from a click in
    /// the view until Escape, or the menu opening.
    grab: bool,
    /// Whether the menu is open: this run's own, as a lockstep peer's is its own; and whether it
    /// shows its options, in place of its first page.
    menu: bool,
    options: bool,
    /// How far this run's view looks down, which the mouse moves: this run's own, as it's no part
    /// of the world.
    pitch: f32,
    /// What this run's view looks at, as it catches up with its fox.
    seen: Option<Vec3>,
    /// The scores' text, written again in its memory each frame.
    text: String,
    /// Each fox's player and place at a tick, gathered before the bots see them, the players whose
    /// bots kick, before they kick, and each fox's speed, before its state machine and dust are
    /// given it: kept, so that ticks reuse their room.
    foxes: Vec<(u8, Vec3)>,
    kicks: Vec<u8>,
    speeds: Vec<(Entity, f32)>,
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
        let pad = |button| Binding::Button(Source::PadButton(button));
        let bound = [
            (Act::Walk, Binding::WASD),
            (Act::Walk, arrows),
            (Act::Walk, Binding::stick(Stick::Left)),
            (Act::Spin, spin_keys),
            (Act::Spin, spin_stick),
            (Act::Kick, Binding::Button(Source::Key(Key::Space))),
            (Act::Kick, pad(PadButton::South)),
            (Act::Menu, pad(PadButton::Start)),
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
            menu: false,
            options: false,
            pitch: VIEW_PITCH,
            seen: None,
            text: String::new(),
            foxes: Vec::new(),
            kicks: Vec::new(),
            speeds: Vec::new(),
        }
    }

    /// Loads the game's sounds, fox and UI, which takes no input until the menu opens, and
    /// registers its components. Fails with the first that doesn't load.
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
        let fox = cx.add_model("Fox", root.join("models/fox/Fox.gltf"))?;
        let ui = cx.add_ui("menu", root.join("ui/menu.ui"))?;
        ui.set_input(cx, false);
        register(cx.world_mut());
        self.assets = Some(Assets { sounds, fox, ui });
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

    /// Opens or closes the menu, as `open` says: its UI shown and taking input while it's open, at
    /// its first page, the focus on Resume, the cursor let go as it opens, and this run's fox
    /// standing, its walk and turn queued at rest, then queued as the keys and sticks are held as it
    /// closes.
    fn open_menu(&mut self, cx: &mut Context, open: bool) {
        self.show_options(cx, false);
        self.menu = open;
        self.grab &= !open;
        let player = self.player;
        let (heading, rate) = if open {
            (Vec2::ZERO, 0.0)
        } else {
            (self.actions.axis2(Act::Walk), self.actions.axis(Act::Spin))
        };
        self.commands.extend([
            Command::Walk { player, heading },
            Command::Spin { player, rate },
        ]);
        if let Some(assets) = &self.assets {
            // A file edited to drop the menu shows none.
            let _ = assets.ui.set_visible(cx, "menu", open);
            assets.ui.set_input(cx, open);
            if open {
                let _ = assets.ui.set_focus(cx, "resume");
            }
        }
    }

    /// Shows the menu's options in place of its first page, or its first page again, as `shown`
    /// says, the focus on the bot's difficulty or on Options: the options taking what the bot plays
    /// at, as the world has it, each bus's volume, as the run has it, and how the view is
    /// antialiased, as the run chose, or else as the world's environment says. A node a file edited
    /// to drop it goes without, as the scores do.
    fn show_options(&mut self, cx: &mut Context, shown: bool) {
        let Some(assets) = &self.assets else {
            return;
        };
        let ui = assets.ui;
        if shown {
            let level = cx
                .world()
                .query::<&Match>()
                .next()
                .map(|found| found.difficulty);
            let choice = Difficulty::ALL.iter().position(|&of| Some(of) == level);
            let _ = ui.set_value(cx, "difficulty", UiValue::Choice(choice.unwrap_or(1)));
            for (slider, _, bus) in VOLUMES {
                let volume = UiValue::Number(cx.volume(bus));
                let _ = ui.set_value(cx, slider, volume);
            }
            // A world without an environment isn't antialiased.
            let seen = cx.world().query::<&Environment>().next();
            let seen = seen.map_or(Antialiasing::None, |seen| seen.antialiasing);
            let antialiasing = cx.antialiasing().unwrap_or(seen);
            let choice = ANTIALIASING.iter().position(|&of| of == antialiasing);
            let _ = ui.set_value(cx, "antialiasing", UiValue::Choice(choice.unwrap_or(0)));
        }
        let _ = ui.set_visible(cx, "menu", !shown);
        let _ = ui.set_visible(cx, "settings", shown);
        if shown != self.options {
            let _ = ui.set_focus(cx, if shown { "difficulty" } else { "options" });
        }
        self.options = shown;
    }

    /// Has the menu's options choose the bot's difficulty in a run alone, and disables it in a
    /// session, which has no bot.
    fn offer_difficulty(&self, cx: &mut Context) {
        if let Some(assets) = &self.assets {
            let networked = cx.networked();
            let _ = assets.ui.set_disabled(cx, "difficulty", networked);
        }
    }

    /// What the menu's options do as widget `name` changes to `value`: the bot's difficulty queues
    /// its command, which the next tick applies, as New match's does; a volume sets its bus's, at
    /// once, this run's own; and the antialiasing sets the view's, in place of the world's
    /// environment's, from the next frame, this run's own too, so that a peer's player chooses
    /// their own, which no tick, hash or recording sees.
    fn change(&mut self, cx: &mut Context, name: &str, value: &UiValue) {
        match (name, value) {
            ("difficulty", &UiValue::Choice(choice)) => {
                if let Some(&level) = Difficulty::ALL.get(choice) {
                    let player = self.player;
                    self.commands.push(Command::Difficulty { player, level });
                }
            }
            ("antialiasing", &UiValue::Choice(choice)) => {
                if let Some(&antialiasing) = ANTIALIASING.get(choice) {
                    cx.set_antialiasing(Some(antialiasing));
                }
            }
            (slider, &UiValue::Number(volume)) => {
                let bus = VOLUMES.iter().find(|&&(of, ..)| of == slider);
                if let Some(&(_, _, bus)) = bus {
                    cx.set_volume(bus, volume, Duration::ZERO);
                }
            }
            _ => {}
        }
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
        self.offer_difficulty(cx);
        if !cx.networked() {
            let player = BOT_PLAYER;
            self.commands.push(Command::Bot { player });
        }
    }

    /// Takes back which player this run steers, which a joiner's world, the host's, and a client's,
    /// the server's, don't say, and lets go of what may have been held as the world was taken; and,
    /// run alone, queues the bot again for a world without one, as the world as edited is, which
    /// Stop brings back. A run started again from a snapshot picks up here, without attaching, so
    /// its options take whether it has a bot here too.
    fn restored(&mut self, cx: &mut Context) {
        self.offer_difficulty(cx);
        self.player = cx.player().unwrap_or(0);
        let player = self.player;
        self.commands.extend([
            Command::Walk {
                player,
                heading: Vec2::ZERO,
            },
            Command::Spin { player, rate: 0.0 },
        ]);
        if !cx.networked() && cx.world().query::<&Bot>().next().is_none() {
            let player = BOT_PLAYER;
            self.commands.push(Command::Bot { player });
        }
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        let _zone = gg_engine::zone!();
        let sounds = self.assets.as_ref().map(|assets| assets.sounds);
        for command in self.commands.drain(..) {
            apply(world, &command, sounds);
        }
        play_bots(world, sounds, &mut self.foxes, &mut self.kicks);
        walk(world, Tick::DT);
        roll(world, Tick::DT);
        step_navigation(world);
        animate(world, &mut self.speeds);
        step_animation(world);
        step_physics(world);
        score(world, sounds);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    /// Grabs the cursor while this run wants it, has the view follow this run's fox, and draws the
    /// UI: the scores, the hint for the cursor as it's grabbed or not, and the menu while it's open.
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
        let ui = assets.ui;
        if let Some(&found) = cx.world().query::<&Match>().next() {
            write_scores(cx, ui, &found, &mut self.text);
        }
        // Nodes a file edited to drop them goes without.
        let _ = ui.set_visible(cx, "hint", !self.grab && !self.menu);
        let _ = ui.set_visible(cx, "hint_grabbed", self.grab && !self.menu);
        if self.options {
            write_volumes(cx, ui, &mut self.text);
        }
        ui.draw(cx);
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        let open = self.menu;
        // What the menu makes of the event while it's open: a button pressed, an option changed,
        // or back, which goes back from the options to the menu's first page.
        let (mut chosen, mut toggled, mut changed) = (None, false, None);
        if open && let Some(assets) = &self.assets {
            match assets.ui.event(cx, event) {
                Some(UiEvent::Pressed(button)) => chosen = Some(button),
                Some(UiEvent::Changed(name, value)) => changed = Some((name, value)),
                Some(UiEvent::Back) if self.options => chosen = Some("back".into()),
                Some(UiEvent::Back) => toggled = true,
                Some(UiEvent::Focused(_)) | None => {}
            }
        }
        // Queued now, rather than at the next update, so that the next tick applies them; none
        // while the menu's open, the fox standing.
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
            Change::Pressed(Act::Menu) => toggled = true,
            _ if open => {}
            Change::Axis2(Act::Walk, heading) => commands.push(Command::Walk { player, heading }),
            Change::Axis(Act::Spin, rate) => commands.push(Command::Spin { player, rate }),
            Change::Pressed(Act::Kick) => commands.push(Command::Kick { player }),
            _ => {}
        });
        match *event {
            // The menu's, while it's open.
            _ if open => {}
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
            // Escape lets go of the cursor, if it's grabbed, or else opens the menu.
            Event::Key {
                key: Key::Escape,
                pressed: true,
                repeat,
            } => {
                if *grab {
                    *grab = false;
                } else if !repeat {
                    toggled = true;
                }
            }
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
        match chosen.as_deref() {
            Some("resume") => toggled = true,
            Some("new_match") => {
                self.commands.push(Command::NewMatch { player });
                toggled = true;
            }
            Some("options") => self.show_options(cx, true),
            Some("back") => self.show_options(cx, false),
            Some("quit") => cx.exit(),
            _ => {}
        }
        if let Some((name, value)) = changed {
            self.change(cx, &name, &value);
        }
        if toggled {
            self.open_menu(cx, !open);
        }
        ControlFlow::Continue(())
    }
}

/// Registers the game's components with `world`, so that its scene and the tool API know them
/// before any entity has one.
fn register(world: &mut World) {
    let registered = world.register::<Player>().and_then(|()| {
        world.register::<Bot>()?;
        world.register::<Ball>()?;
        world.register::<Goal>()?;
        world.register::<Match>()
    });
    registered.expect("the game's components describe themselves");
}

/// Builds what the arena's scene doesn't hold: the match, and each player's fox at its start, a
/// character's capsule with `fox`'s model under it, its feet at the capsule's bottom, its clips
/// played by an animator, which its state machine drives, as [`states`] makes it, and its dust
/// behind them.
fn build(world: &mut World, fox: Model) {
    world.spawn((
        Match {
            scores: [0; 2],
            wins: [0; 2],
            difficulty: Difficulty::default(),
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
        let driven = played.and_then(|()| world.insert(model, states()));
        let parented = driven.and_then(|()| world.set_parent(model, Some(body)));
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

/// A fox's state machine: `Idle`, Survey alone, while it goes slower than [`MOVING`], and `Move`,
/// Walk at [`WALK_SPEED`] blended with Run at [`RUN_SPEED`] along its [`SPEED`], as fast or faster,
/// each fading into the other over [`STATE_FADE`] seconds. Survey stays out of the blend, whose
/// clips keep in step, as a pose that stands would be sped up or slowed down with them.
fn states() -> AnimationStates {
    let state = |name: &str, motion| AnimationState {
        name: name.to_owned(),
        motion,
        speed: 1.0,
        playback: Playback::Loop,
    };
    let point = |clip: &str, at| BlendPoint {
        clip: clip.to_owned(),
        at,
    };
    let moving = StateMotion::Blend {
        param: SPEED.to_owned(),
        points: vec![point("Walk", WALK_SPEED), point("Run", RUN_SPEED)],
    };
    let transition = |from: &str, to: &str, test| StateTransition {
        from: Some(from.to_owned()),
        to: to.to_owned(),
        conditions: vec![ParamCondition {
            param: SPEED.to_owned(),
            test,
        }],
        exit_time: None,
        fade: STATE_FADE,
    };
    AnimationStates {
        params: vec![AnimationParam {
            name: SPEED.to_owned(),
            value: ParamValue::Float(0.0),
        }],
        states: vec![
            state("Idle", StateMotion::Clip("Survey".to_owned())),
            state("Move", moving),
        ],
        transitions: vec![
            transition("Idle", "Move", ParamTest::Above(MOVING)),
            transition("Move", "Idle", ParamTest::Below(MOVING)),
        ],
        state: String::new(),
        time: 0.0,
    }
}

/// Applies `command` to `world`: to its player's fox, which way it walks, how fast it turns, a turn
/// or a kick, as [`kick`] has it; a new match, as [`new_match`] has it; the bot, as [`give_bot`]
/// has it; or how well the bot plays, on the match, which [`play_bots`] reads.
fn apply(world: &mut World, command: &Command, sounds: Option<Sounds>) {
    let player = match *command {
        Command::Kick { player } => return kick(world, player, sounds),
        Command::NewMatch { .. } => return new_match(world),
        Command::Bot { player } => return give_bot(world, player),
        Command::Difficulty { level, .. } => {
            if let Some(found) = world.query_mut::<&mut Match>().next() {
                found.difficulty = level;
            }
            return;
        }
        Command::Walk { player, .. }
        | Command::Spin { player, .. }
        | Command::Turn { player, .. } => player,
    };
    let players = world.query_mut::<(&mut Player, &mut Transform)>();
    for (fox, at) in players.filter(|(fox, _)| fox.index == player) {
        match *command {
            Command::Walk { heading, .. } => fox.heading = heading.clamp_length_max(1.0),
            Command::Spin { rate, .. } => fox.spin = rate.clamp(-1.0, 1.0),
            Command::Turn { by, .. } => {
                fox.yaw = wrapped(fox.yaw - by);
                at.rotation = Quat::from_rotation_y(fox.yaw);
            }
            Command::Kick { .. }
            | Command::NewMatch { .. }
            | Command::Bot { .. }
            | Command::Difficulty { .. } => {}
        }
    }
}

/// Has player `player`'s fox kick, if its cooldown has passed, which it then waits again: the ball,
/// if the fox [`reaches`] it, goes forward and up, its emitter bursting and `sounds`' kick playing
/// from it.
fn kick(world: &mut World, player: u8, sounds: Option<Sounds>) {
    let mut kicker = None;
    let players = world.query_mut::<(&mut Player, &Transform)>();
    for (fox, at) in players.filter(|(fox, _)| fox.index == player && fox.cooldown == 0) {
        kicker = Some((at.translation, fox.yaw));
        fox.cooldown = KICK_EVERY;
    }
    let Some((from, yaw)) = kicker else {
        return;
    };
    let forward = Quat::from_rotation_y(yaw) * Vec3::Z;
    let balls = world.query_mut::<(Entity, &Ball, &Transform, &mut RigidBody3D)>();
    let mut kicked = None;
    for (ball, _, at, body) in balls {
        if reaches(from, yaw, at.translation) {
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

/// Whether a fox whose middle is at `from`, facing `yaw`, reaches the ball at `ball` with a kick:
/// the ball's middle within [`KICK_REACH`] across of just in front of the fox, and within a unit of
/// the fox's middle up or down.
fn reaches(from: Vec3, yaw: f32, ball: Vec3) -> bool {
    let forward = Quat::from_rotation_y(yaw) * Vec3::Z;
    let front = from + forward * CAPSULE.1;
    let flat = Vec3::new(ball.x, front.y, ball.z);
    flat.distance(front) < KICK_REACH && (ball.y - from.y).abs() < 1.0
}

/// Starts a match afresh, as a menu's New match chooses: no goals and no matches won for either
/// player, and the ball and the foxes back at their starts, the bot playing as well as it did.
fn new_match(world: &mut World) {
    if let Some(found) = world.query_mut::<&mut Match>().next() {
        found.scores = [0; 2];
        found.wins = [0; 2];
    }
    to_starts(world);
}

/// Has the game's bot play player `player`'s fox from then on, unless one does already: a [`Bot`]
/// on its capsule, which sees the ball at its first tick, and a [`NavAgent`] as wide as the
/// capsule, which runs as fast as the match's difficulty says.
fn give_bot(world: &mut World, player: u8) {
    let found = (world.query::<(Entity, &Player)>()).find(|(_, fox)| fox.index == player);
    let Some(fox) = found.map(|(fox, _)| fox) else {
        return;
    };
    if world.get::<Bot>(fox).is_some() {
        return;
    }
    let level = world.query::<&Match>().next().map(|found| found.difficulty);
    let agent = NavAgent {
        speed: level.unwrap_or_default().speed(),
        radius: CAPSULE.1,
        stopping_distance: 0.15,
        ..NavAgent::default()
    };
    let bot = Bot {
        wait: 0,
        aim: Vec3::ZERO,
    };
    let given = world
        .insert(fox, bot)
        .and_then(|()| world.insert(fox, agent));
    given.expect("the fox, just found");
}

/// `v` along the ground: its height dropped.
fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// Plays each fox a [`Bot`] is on, as well as the match's [`Difficulty`] says: every reaction's
/// ticks it sees where the ball and the foxes are, chooses where it aims, as [`aim`] says, and sets
/// where its [`NavAgent`] heads, as [`bot_destination`] says, the agent steering its character
/// there, at the difficulty's speed; at every tick it turns its fox, at up to [`BOT_TURN_SPEED`],
/// toward the ball while it's near and ahead toward where it aims, or else the way the agent goes,
/// or else the ball; and it kicks, as a player's kick does, gathering its player in `kicks` first,
/// while the fox faces as near where it aims as the difficulty's aim says and [`reaches`] the ball.
/// It decides from the world alone, in `f32` arithmetic, libm's arctangent and comparisons, as
/// peers and replays must agree. `foxes` is where it gathers each fox's player and place first.
fn play_bots(
    world: &mut World,
    sounds: Option<Sounds>,
    foxes: &mut Vec<(u8, Vec3)>,
    kicks: &mut Vec<u8>,
) {
    let ball = world.query::<(&Ball, &Transform)>().next();
    let Some(ball) = ball.map(|(_, at)| at.translation) else {
        return;
    };
    let level = world.query::<&Match>().next().map(|found| found.difficulty);
    let level = level.unwrap_or_default();
    foxes.clear();
    let placed = world.query::<(&Player, &Transform)>();
    foxes.extend(placed.map(|(fox, at)| (fox.index, at.translation)));
    let most = BOT_TURN_SPEED * Tick::DT.as_secs_f32();
    let bots = world.query_mut::<(&mut Bot, &mut Player, &mut Transform, &mut NavAgent)>();
    for (bot, fox, transform, agent) in bots {
        let at = transform.translation;
        // Player 0 attacks the far goal, at +Z, and player 1 the near one.
        let toward = if fox.index == 0 { 1.0 } else { -1.0 };
        if bot.wait == 0 {
            bot.aim = aim(ball, toward * FIELD.y, foxes, fox.index);
        }
        let attack = flat(bot.aim - ball).normalize_or(Vec3::Z * toward);
        if bot.wait == 0 {
            agent.destination = Some(bot_destination(at, ball, attack));
            bot.wait = level.reaction() - 1;
        } else {
            bot.wait -= 1;
        }
        agent.speed = level.speed();
        let to_ball = flat(ball - at);
        let near = to_ball.length() < BOT_CLOSE && to_ball.dot(attack) > 0.0;
        let way = if near || agent.velocity.length() < 0.5 {
            to_ball
        } else {
            agent.velocity
        };
        if way.length_squared() > 1e-4 {
            let turn = wrapped(atan2f(way.x, way.z) - fox.yaw).clamp(-most, most);
            if turn != 0.0 {
                fox.yaw = wrapped(fox.yaw + turn);
                transform.rotation = Quat::from_rotation_y(fox.yaw);
            }
        }
        let forward = Quat::from_rotation_y(fox.yaw) * Vec3::Z;
        if fox.cooldown == 0 && forward.dot(attack) > level.aim() && reaches(at, fox.yaw, ball) {
            kicks.push(fox.index);
        }
    }
    for player in kicks.drain(..) {
        kick(world, player, sounds);
    }
}

/// Where a bot of player `own` aims the ball at `ball`, the goal it attacks at `end` along Z, the
/// other foxes as `foxes` places them: the middle of the goal's mouth or [`BOT_POST`] to either
/// side, whichever way from the ball the other foxes ahead of it stand farthest from, the first of
/// those as far; or, where each stands nearer one than [`BOT_OPEN`], [`BOT_PAST`] to the side of
/// the fox nearest that way, the side the ball is on, so that it shoots again from past it. All
/// along the ground.
fn aim(ball: Vec3, end: f32, foxes: &[(u8, Vec3)], own: u8) -> Vec3 {
    let from = flat(ball);
    // How far the nearest other fox ahead of the ball stands from its way to `to`, and that fox.
    let room = |to: Vec3| {
        let way = to - from;
        let mut nearest = (f32::INFINITY, None);
        for &(_, at) in foxes.iter().filter(|&&(index, _)| index != own) {
            let along = flat(at - from).dot(way) / way.length_squared();
            if along.is_nan() || along <= 0.0 {
                continue;
            }
            let along = if along < 1.0 { along } else { 1.0 };
            let off = flat(at - (from + way * along)).length();
            if off < nearest.0 {
                nearest = (off, Some(at));
            }
        }
        nearest
    };
    let mut best = (f32::NEG_INFINITY, Vec3::ZERO, None);
    for x in [0.0, -BOT_POST, BOT_POST] {
        let to = Vec3::new(x, 0.0, end);
        let (off, nearest) = room(to);
        if off > best.0 {
            best = (off, to, nearest);
        }
    }
    match best {
        (off, _, Some(blocker)) if off < BOT_OPEN => {
            let side = if ball.x < blocker.x { -1.0 } else { 1.0 };
            let inside = FIELD.x - 1.0;
            let x = (blocker.x + side * BOT_PAST).clamp(-inside, inside);
            Vec3::new(x, 0.0, blocker.z)
        }
        (_, to, _) => to,
    }
}

/// Where a bot whose fox is at `at` heads, the ball at `ball` and `attack` the way from it toward
/// where the bot aims, along the ground, of unit length: on that side of the ball, or beside it,
/// out of the ball's way, [`BOT_CLEAR`] to its side, then round it to as far behind it as
/// [`BOT_BEHIND`]; behind it, to that far behind it, lined up with its aim; and lined up, on
/// through the ball as far again. It keeps [`BOT_MARGIN`] inside the field, but in front of a
/// goal's mouth, where it goes as far as a unit past the field's end; at the fox's height, as an
/// agent's middle stands above its floor.
fn bot_destination(at: Vec3, ball: Vec3, attack: Vec3) -> Vec3 {
    let from_ball = flat(at - ball);
    let along = from_ball.dot(attack);
    let aside = from_ball - attack * along;
    let off = aside.length();
    // Its own side of the ball, or the right of the way to the goal, facing it, where it's on it.
    let out = if off > 1e-3 {
        aside / off
    } else {
        Vec3::new(-attack.z, 0.0, attack.x)
    };
    let behind = ball - attack * BOT_BEHIND;
    let to = if along > -BOT_BEHIND / 2.0 {
        if off < BOT_CLEAR {
            ball + out * BOT_CLEAR + attack * along.clamp(-BOT_BEHIND, BOT_BEHIND)
        } else {
            ball + out * BOT_CLEAR - attack * BOT_BEHIND
        }
    } else if flat(at - behind).length() > BOT_LINED {
        behind
    } else {
        ball + attack * BOT_BEHIND
    };
    let x = to.x.clamp(BOT_MARGIN - FIELD.x, FIELD.x - BOT_MARGIN);
    let long = if x.abs() < GOAL - BOT_MARGIN {
        FIELD.y + 1.0
    } else {
        FIELD.y - BOT_MARGIN
    };
    Vec3::new(x, at.y, to.z.clamp(-long, long))
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
/// counting its kick's cooldown down. A bot's fox stands here, its agent steering it after.
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

/// Sets each fox's state machine's [`SPEED`] to how fast its character walks across, which takes
/// it from `Idle` to `Move` and back and blends Walk and Run in `Move`; and its dust's rate by how
/// far from walking to running it goes, to [`DUST_RATE`] at its running speed. `speeds` is where it
/// gathers each fox's speed first.
fn animate(world: &mut World, speeds: &mut Vec<(Entity, f32)>) {
    speeds.clear();
    let foxes = world.query::<(Entity, &Player, &CharacterController3D)>();
    speeds
        .extend(foxes.map(|(body, _, character)| (body, character.velocity.with_y(0.0).length())));
    let speed_of = |body: Entity| {
        let found = speeds.iter().find(|(fox, _)| *fox == body);
        found.map(|&(_, speed)| speed)
    };
    let running = |speed: f32| ((speed - WALK_SPEED) / (RUN_SPEED - WALK_SPEED)).clamp(0.0, 1.0);
    for (&Parent(body), dust) in world.query_mut::<(&Parent, &mut ParticleEmitter)>() {
        if let Some(speed) = speed_of(body) {
            dust.rate = DUST_RATE * running(speed);
        }
    }
    for (&Parent(body), machine) in world.query_mut::<(&Parent, &mut AnimationStates)>() {
        // A machine edited to hold no speed, as through the tool API, goes without.
        if let Some(speed) = speed_of(body) {
            let _ = machine.set_param(SPEED, speed);
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

/// Has the UI `ui`'s options say each bus's volume, as the run has it, as a percentage, writing
/// each text through `text`. A text the UI's file no longer names goes unset, as the scores do.
fn write_volumes(cx: &mut Context, ui: GameUi, text: &mut String) {
    for (_, said, bus) in VOLUMES {
        text.clear();
        let written = write!(text, "{:.0}%", cx.volume(bus) * 100.0);
        written.expect("a string takes any text");
        let _ = ui.set_text(cx, said, text);
    }
}

/// Has the UI `ui` say the match's scores, as `found` has them, writing each text through `text`:
/// each player's goals, and its matches won below them, in the texts [`SCORES`] names. A text the
/// UI's file no longer names, as an edit of it may take one away, goes unset: the file says what
/// shows.
fn write_scores(cx: &mut Context, ui: GameUi, found: &Match, text: &mut String) {
    for (index, (goals, matches)) in SCORES.into_iter().enumerate() {
        text.clear();
        write!(text, "{}", found.scores[index]).expect("a string takes any text");
        let _ = ui.set_text(cx, goals, text);
        let won = found.wins[index];
        text.clear();
        let wins = if won == 1 { "WIN" } else { "WINS" };
        write!(text, "{won} {wins}").expect("a string takes any text");
        let _ = ui.set_text(cx, matches, text);
    }
}
