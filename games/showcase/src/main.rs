//! The showcase: a scene for each of what the engine draws and simulates so far, in its scenes
//! folder, which the editor's Scenes panel opens, and Page Down and Page Up step through as it
//! plays, in the order of their names, the game putting each in place of the last through
//! `World::change_scene`, each with a camera of its own: `welcome.ron`,
//! where it starts; sprites, tinted, textured, cut from a sheet, tiled, sampled crisp or smooth,
//! flipped and blended (`sprites.ron`); sprites played through a sheet's cells, a coin spinning and
//! numbered tiles, looping, back and forth and once, at their own rates and flipped
//! (`flipbooks.ron`, its sheet `textures/flipbook.png`, written by a script outside the
//! repository); fields eased by tweens, a dot along a track for each curve, a size, a turn, a color
//! and an alpha, and a crate a script pops (`tweens.ron`); circles, discs and rings, crisp and
//! faded (`shapes.ron`);
//! text in the engine's font and the project's, aligned, sized, colored and turned (`text.ron`); a
//! game's UI on the view's canvas, labels kept to its edges and corners, the ticks counted and a
//! button that lights under the cursor and counts its clicks, over the world (`ui.ron`); a game's
//! UI from a file, laid out by the engine, a menu in the view's middle, four buttons, one disabled,
//! which keys, a pad and the mouse move the focus over and press, a soft tick as the focus moves and
//! a click at a press (`sounds/tick.wav` and `sounds/click.wav`), a HUD along the top whose clock
//! the layer sets each second, and panels in its corners saying what was pressed and what has the
//! focus, each press reaching the world as a command (`menu.ron`, its UI `ui/menu.ui`, which
//! reloads as it's saved); a UI's widgets, a slider of the volume whose label follows it, a
//! toggle, a drop-down of difficulties, a text field for a name and a list of saves that scrolls
//! and clips them, each change reaching the world as a command, which the panel's last line tells
//! of, ticking and clicking as the menu does (`widgets.ron`, its UI `ui/widgets.ui`); a
//! hierarchy of transforms, each child carried by its parent (`hierarchy.ron`); 2D physics, bodies
//! dynamic, kinematic and static, boxes and circles, bouncing and sliding (`physics.ron`); sounds
//! played from where their entities are, bells to the left, the middle and the right in turn and a
//! pluck sweeping across and out past the view's edges (`sounds.ron`, its sounds `sounds/bell.wav`
//! and `sounds/pluck.wav`); music looping while the scene is open, its volume a field, bars
//! bouncing at its tempo (`music.ron`, its music `music/loop.ogg`; those three, and the UI's tick
//! and click, synthesized by scripts outside the repository); squares drawn by the game's own
//! shader, which reloads as it's saved (`shader.ron`,
//! its shader `shaders/plasma.slang`); glTF
//! models, unlit, textured and of more than one node, through a camera with perspective
//! (`models.ron`); Khronos's Fox, skinned, playing its clips, Survey, Walk and Run, each alone,
//! walking and running blended, and cross-faded from one to the next (`skeletal.ron`, the model in
//! `models/fox`, CC0 by PixelMannen, rigged and animated by tomkranis and made glTF by
//! AsoboStudio and scurest under CC BY 4.0, as its README there says); chimes heard in 3D, by
//! their direction and distance from a camera that the mouse looks about with, the cursor grabbed
//! from a click in the view until Escape, a bell to the left, a pluck behind, and farther ones
//! ahead (`listen.ron`); colors from a quarter as
//! bright as white to 16 times, which the world's
//! environment tonemaps (`hdr.ron`); light spreading round neon tubes, sparks and a glowing sphere
//! at night, as the world's environment blooms them (`bloom.ron`); the light of models under a sky
//! graded as a golden hour's, warmer, of more contrast and saturation, its shadows teal
//! (`grading.ron`); glTF's metallic-roughness materials, plastic, metal,
//! glowing and tiled with every map, lit by a sun, a point light and a spot (`lights.ron`); the
//! same lit by a sky alone, Poly Haven's Kloofendal 48d Partly Cloudy Puresky at 1K, CC0, which
//! shows behind them (`sky.ron`); a sky made from its sun, Hillaire's atmosphere, under a sun that
//! `scripts/spin.luau` turns through a day two minutes long, with its night, the haze deepening on
//! towers from 300 m to 12 km off (`atmosphere.ron`); under that sky turned through a day a minute
//! long, an exposure that adapts to what the view sees, as eyes do, from the noon sun, through
//! dusk, to two lamps at night, faster to the light than to the dark (`exposure.ron`); models
//! casting a sun's shadows
//! (`shadows.ron`); crates, balls and shapes piled in a corner under that sky alone, its light
//! darkened where they hide it from each other and the walls, by ambient occlusion
//! (`ambient_occlusion.ron`); spheres, a cube and a torus mirrored in a floor of polished steel,
//! black stone and brushed bronze under that sky, the bronze's reflections blurred as its roughness
//! spreads them, by screen-space reflections (`reflections.ron`, its floor `models/gloss.gltf`); a
//! fence's thin slats and rails, a leaning pole and models, a crate and a torus turning, in a sun's
//! light, antialiased by SMAA from each frame alone, sharp and without ghosts (`antialiasing.ron`);
//! crates, balls and spheres, a glowing one among them, receding from a camera of a narrow field of
//! view, those before and past its focus at 7 m blurred, as through a lens of f/1.4, by depth of
//! field (`depth_of_field.ron`); balls and marbles on a carousel, a crate spinning and a torus
//! tumbling, blurred along their ways by motion blur through a 180° shutter (`motion_blur.ron`);
//! spheres, a crate and a torus seen through a lens's vignette, chromatic aberration and grain
//! (`lens.ron`); 3D physics,
//! boxes, balls and capsules, dynamic, kinematic and static, tumbling down a ramp, bouncing and
//! swept about within walls (`physics3d.ron`); particles, a fountain's drops going over what's
//! behind them by their alpha, a fire and its embers adding their light over its smoke, the floor
//! lit by a light beside it, sparks that a burster bursts every so often, and a comet's trail
//! behind an emitter a spinning pivot carries round (`particles.ron`); linked prefabs, a lantern
//! placed four times, three of them changing a field of their own, a color, a caption or where
//! their flame is, and a lamppost that links two lanterns, its own instances inside it, every
//! lantern taking what its prefab's file changes as it's saved (`prefabs.ron`, of
//! `prefabs/lantern.prefab` and `prefabs/lamppost.prefab`); the Fox driven round a circle by a state
//! machine, surveying as it stands, walking, jogging and running blended by its speed, and looking
//! about once a round at a trigger, a line of text saying its state and speed (`states.ron`);
//! joints, a bridge of planks on revolute joints, a pendulum on a spherical one, a door its motor
//! swings to either side within its limits, a piston its motor drives along a prismatic joint
//! between its limits and a ball on a rope, and continuous collision, balls fired at a thin plate
//! that turns, those that have it stopping at it where some of the others pass through
//! (`joints.ron`); a navmesh baked from a small maze's colliders, a step and a ramp up to a
//! platform among them, and a dozen agents walking its paths to stops in turn, round each other,
//! the navmesh and their paths in lines (`navigation.ron`); a world of cells 64 m on a side, an 8
//! by 8 grid of scene files, each a crate, a torus, a ball and a lamp, which come in around a
//! character the keys walk, its camera following, and go out as it walks away, kept as they were
//! (`cells.ron`, its cells in `scenes/cells`); and, once `cargo
//! gg fetch` has fetched them, Sponza under a sky made from its sun, its arches darkened by ambient
//! occlusion, what's bright blooming, graded and in a low fog, its gilding mirroring the arcades in
//! place of the sky, exposed as it adapts to the sunlit atrium and the shaded arcades, through a
//! light lens (`sponza.ron`),
//! under the Puresky and a sun in a low height fog that they light, thickest along its floor and
//! thinning up its walls (`fog.ron`), and walked
//! through by a character that the keys or a pad move, a camera following it, Sponza's triangles
//! what it collides with, among crates and balls it pushes (`walk.ron`), and Amazon
//! Lumberyard Bistro, its materials of specular and glossiness, its images DDS files of BC7, lit by
//! its own sun and 96 point lights under that sky (`bistro.ron`), and 16 of it, 4 by 4, each with
//! its own lights, its 16 suns' light exposed 4 stops down, from the same street of one of the
//! four in the middle (`bistro_grid.ron`). Its
//! Luau scripts, `scripts/spin.luau` and `scripts/bob.luau`,
//! turn and bob what they're on at each tick, `scripts/walk.luau` walks a character the way the
//! keys or a pad say, W and S or the arrows or the left stick forward and back, turning by A and D,
//! jumping by Space or South, `scripts/follow.luau` follows it with a camera, casting back
//! from it so that walls don't come between them, and `scripts/pulse.luau` pops what it's on every
//! so often, a tween easing it back, `scripts/moods.luau` cross-fades its fox to its next clip
//! every few seconds through `gg.play`, `scripts/pace.luau` walks its fox round, setting its state
//! machine's speed and pulling its trigger through `gg.set_param`, `scripts/swing.luau` turns its
//! joint's motor around every few seconds through the joint's fields, and `scripts/tour.luau`
//! sends its agent to its stops in turn by its destination; and animation, navigation and physics
//! step, so that each scene moves as it plays, in the editor from its Play, and in the reference
//! player, which runs the game's package without its code, and so without the keys' walking and
//! its own components: the chimes, the bursters, the jukebox, the HUD, the menu, the widgets, the
//! plasma, the look, the state labels, the tether and the launches, which its code registers and
//! runs.

// A build without `dev`, as it ships, shows no console on Windows; its JSON log still records the
// run, and output piped to another process still arrives.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::fmt::Write as _;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use bytemuck::NoUninit;
use gg_engine::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use gg_engine::reflect::Value;
use gg_engine::{
    Actions, Align, AnimationStates, App, Binding, Blend, Bus, Change, Commands, Component,
    Context, Curve, Ease, Entity, Error, Event, Font, GameUi, Joint3D, Key, Layer, Mesh, MeshDraw,
    MouseButton, Music, PadButton, ParamValue, ParticleEmitter, Place, Reflect, RigidBody3D,
    Scripts, Shader, Sound, Source, Sprite, Stick, Text, TextStyle, Tick, Transform, Tween,
    UiEvent, UiValue, World, step_animation, step_navigation, step_physics,
};

#[cfg(test)]
mod tests;

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's Performance and Inspector panels: what `main` runs, and what its
/// recordings replay.
fn app() -> App {
    let app = App::new("Showcase").project(env!("CARGO_MANIFEST_DIR"));
    let app = app.layer(Showcase::new()).overlay(gg_engine::PerfPanel);
    app.overlay(gg_engine::Inspector::default())
}

/// The game's scripts, by their paths from its directory: each declares the component it is.
const SCRIPTS: [&str; 9] = [
    "scripts/bob.luau",
    "scripts/follow.luau",
    "scripts/moods.luau",
    "scripts/pace.luau",
    "scripts/pulse.luau",
    "scripts/spin.luau",
    "scripts/swing.luau",
    "scripts/tour.luau",
    "scripts/walk.luau",
];

/// What the keys and a pad do: walk the walkers, `walk.luau`'s, and have them jump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// W, A, S and D, the arrows and the left stick.
    Walk,
    /// Space and South.
    Jump,
}

/// What the keys, a pad and the mouse do, queued for the next tick, which a recording keeps.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Has every walker go this way, until the next: across to turn, and up to go forward.
    Walk { heading: Vec2 },
    /// Has every walker jump, if it stands.
    Jump,
    /// Counts a click on the HUD's button, on each [`Hud`].
    Click,
    /// Turns each [`Look`] by the mouse's raw motion, `by`, right and down, as it moved with the
    /// cursor grabbed.
    Look { by: Vec2 },
    /// Records a press of the menu's button named `button`, or `back` for back, on each [`Menu`].
    Press { button: String },
    /// Records a change of the widget named `widget` to what `value` says, or a press of a button
    /// among the widgets, on each [`Widgets`].
    Change { widget: String, value: String },
    /// Puts the scene at `path`, from the game's directory, in place of the world's entities, as
    /// Page Up and Page Down step through the scenes.
    Scene { path: String },
}

/// A sound of the showcase's, which a [`Chime`] rings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
enum Tone {
    /// `sounds/bell.wav`, a struck bell.
    #[default]
    Bell,
    /// `sounds/pluck.wav`, a plucked string.
    Pluck,
}

/// Rings its tone every `every` seconds of the world's ticks, from its entity, so that it's panned
/// and faded as the view hears it there, as [`Place`] says: by where a 2D view shows it, and by its
/// direction and distance from a camera with perspective; a `Circle` on it flashes full as it
/// rings, thinning back to [`RING`]: `sounds.ron`'s chimes, and `listen.ron`'s.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Chime {
    /// What it rings.
    tone: Tone,
    /// The seconds between its rings.
    #[reflect(min = 0.05, default = 1.0)]
    every: f32,
    /// The seconds since it last rang; negative, a wait before its first.
    time: f32,
}

/// Bursts its entity's [`ParticleEmitter`] every `every` seconds of the world's ticks, raising its
/// count of bursts at the tick: `particles.ron`'s sparks.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Burster {
    /// The seconds between its bursts.
    #[reflect(min = 0.05, default = 1.0)]
    every: f32,
    /// The seconds since it last burst; negative, a wait before its first.
    time: f32,
}

/// Has the showcase's music, `music/loop.ogg`, play while its entity is in the world, looping, its
/// bus at `volume`, fading in as it starts and out once no entity has one, as another scene opens:
/// `music.ron`'s. It plays whether the world ticks or not, as music plays on through a pause.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Jukebox {
    /// How loud the music's bus plays it: 1 as loud as the music is, 0 silent.
    #[reflect(min = 0.0, max = 1.0, default = 1.0)]
    volume: f32,
}

/// Has the showcase draw its HUD on the view's canvas while its entity is in the world: a label at
/// each of the canvas's edges and corners, the ticks it has counted, and a button that lights as
/// the cursor is over it and counts the clicks on it, which reach it as commands: `ui.ron`'s.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Hud {
    /// The ticks it has been in the world for.
    ticks: u32,
    /// The clicks on its button.
    clicks: u32,
}

/// Has the showcase drive its menu, `ui/menu.ui`, while its entity is in the world: draws it on the
/// view's canvas, its clock the seconds the menu has been in the world, and has it take input,
/// whose presses reach the world as commands, which record here what was pressed: `menu.ron`'s.
#[derive(Clone, Debug, PartialEq, Component)]
struct Menu {
    /// The ticks it has been in the world for.
    ticks: u32,
    /// The presses of its buttons, and of back, and the name of the last pressed.
    presses: u32,
    pressed: String,
}

/// Has the showcase drive its widgets, `ui/widgets.ui`, while its entity is in the world: draws
/// them on the view's canvas, the volume's label saying the slider's value, and has them take
/// input, whose changes reach the world as commands, which record here what changed last:
/// `widgets.ron`'s.
#[derive(Clone, Debug, PartialEq, Component)]
struct Widgets {
    /// The changes of its widgets and the presses of its buttons, and the last, the widget's name
    /// and what it changed to.
    changes: u32,
    changed: String,
}

/// The widgets' drop-down's options, as `ui/widgets.ui` names them.
const DIFFICULTIES: [&str; 4] = ["EASY", "NORMAL", "HARD", "NIGHTMARE"];

/// Has its entity drawn by the showcase's own shader, `shaders/plasma.slang`, once the world
/// ticks: a unit square that its transform places, colored by waves drifting with the frame's time,
/// `cells` of them across, at `speed`, tinted by `tint`: `shader.ron`'s.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Plasma {
    /// What the waves' colors are multiplied by, in linear RGB; its alpha is unused.
    #[reflect(default = Vec4::ONE)]
    tint: Vec4,
    /// How many waves cross the square.
    #[reflect(min = 0.0, default = 1.0)]
    cells: f32,
    /// How fast the waves drift, 1 a radian a second.
    #[reflect(default = 1.0)]
    speed: f32,
}

/// Turns its entity, a camera, as the mouse moves with the cursor grabbed, which a click in the view
/// grabs and Escape lets go of: about the world's up by `yaw`, and up or down by `pitch`, no further
/// than [`MAX_PITCH`], level, each of the mouse's units turning it `sensitivity` radians:
/// `listen.ron`'s.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Look {
    /// Its turn to its left about the world's up, in radians, from facing -Z.
    yaw: f32,
    /// Its turn up, in radians, no further than [`MAX_PITCH`] either way.
    #[reflect(min = -1.553_343, max = 1.553_343)]
    pitch: f32,
    /// How far a unit of the mouse's motion turns it, in radians.
    #[reflect(min = 0.0, default = 0.0025)]
    sensitivity: f32,
}

/// The furthest a [`Look`] turns up or down: 89°, short of straight, where its yaw would spin.
const MAX_PITCH: f32 = 1.553_343;

/// Has its entity's [`Text`] say the state that `of`'s state machine is in and its `speed`
/// parameter, as each tick leaves them, as `Move  ·  speed 3.3`: `states.ron`'s.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct StateLabel {
    /// The entity whose [`AnimationStates`] it tells of.
    of: Option<Entity>,
}

/// Places its entity, a model a unit tall along its Y, from one anchor of the [`Joint3D`] on it to
/// the other, where the last tick left the joint's bodies, `thickness` across: `joints.ron`'s rope.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Tether {
    /// How wide and deep it is.
    #[reflect(min = 0.0, default = 0.03)]
    thickness: f32,
}

/// Fires its entity, a body, every `every` seconds of the world's ticks: puts it at `from`,
/// unturned, going at `velocity`: `joints.ron`'s balls, fired at a thin plate.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Launch {
    /// Where it's fired from.
    from: Vec3,
    /// How fast it's fired, in units a second.
    velocity: Vec3,
    /// The seconds between its firings.
    #[reflect(min = 0.05, default = 1.0)]
    every: f32,
    /// The seconds since it was last fired; negative, a wait before its first.
    time: f32,
}

/// A corner of the plasma's square, as `shaders/plasma.slang` reads it: where it is in the square's
/// own space, and where it is across the square, from 0 to 1.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Corner {
    position: Vec3,
    uv: Vec2,
}

/// What `shaders/plasma.slang` draws a square in, as it reads it: a [`Plasma`]'s fields.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Waves {
    tint: Vec4,
    cells: f32,
    speed: f32,
    pad: Vec2,
}

/// The thickness `sounds.ron` draws its chimes' rings at, which a ring flashes back to.
const RING: f32 = 0.12;

/// How long music takes to fade in as it starts and out as it stops.
const MUSIC_FADE: Duration = Duration::from_millis(1500);

/// How long the music's bus takes to reach a jukebox's volume.
const VOLUME_FADE: Duration = Duration::from_millis(100);

/// How far the HUD's labels are from the canvas's edges, in its units.
const HUD_INSET: f32 = 36.0;

/// The HUD's labels at the canvas's edges and corners, each where it's anchored, across then down,
/// which its text aligns to.
const ANCHORED: [([Align; 2], &str); 8] = [
    ([Align::Start, Align::Start], "TOP LEFT"),
    ([Align::Center, Align::Start], "TOP"),
    ([Align::End, Align::Start], "TOP RIGHT"),
    ([Align::Start, Align::Center], "LEFT"),
    ([Align::End, Align::Center], "RIGHT"),
    ([Align::Start, Align::End], "BOTTOM LEFT"),
    ([Align::Center, Align::End], "BOTTOM"),
    ([Align::End, Align::End], "BOTTOM RIGHT"),
];

/// The HUD's button: its middle and its half size, from the canvas's middle.
const BUTTON: (Vec2, Vec2) = (Vec2::new(0.0, -40.0), Vec2::new(190.0, 56.0));

/// The scene the game starts in, as its project's manifest names it.
const START: &str = "scenes/welcome.ron";

/// The game's scenes, by their paths from its directory, `root`, `/` between their parts, in order:
/// those its package holds, in a run from one, as a build without `dev` runs, and else those in its
/// scenes folder.
fn scenes(cx: &Context, root: &Path) -> Vec<String> {
    let mut paths = cx.package_files();
    if paths.is_empty() {
        let listed = std::fs::read_dir(root.join("scenes")).into_iter().flatten();
        paths = listed.flatten().map(|entry| entry.path()).collect();
    }
    let scene = |path: &PathBuf| {
        let within = path.strip_prefix(root).ok()?;
        let parts: Option<Vec<&str>> = within.iter().map(|part| part.to_str()).collect();
        let parts = parts?;
        let ron = within.extension().is_some_and(|ext| ext == "ron");
        (ron && parts.len() == 2 && parts[0] == "scenes").then(|| parts.join("/"))
    };
    let mut scenes: Vec<String> = paths.iter().filter_map(scene).collect();
    scenes.sort();
    scenes
}

/// What the showcase loads beside its scripts, for its scenes' components.
struct Assets {
    /// The sounds, by [`Tone`].
    tones: [Sound; 2],
    /// The menu's and the widgets' cues, quiet and short beside the chimes' tones: `focus` as the
    /// focus moves, as the cursor comes over a button, and `press` at a press of a button.
    focus: Sound,
    press: Sound,
    music: Music,
    /// Its font, Inter Bold, which the HUD writes in.
    font: Font,
    /// The plasma's square and shader.
    square: Mesh<Corner>,
    plasma: Shader<Corner, Waves>,
    /// The menu, `ui/menu.ui`, as [`Menu`] drives it.
    menu: GameUi,
    /// The widgets, `ui/widgets.ui`, as [`Widgets`] drives them.
    widgets: GameUi,
}

impl Assets {
    /// Loads them from the game's directory, `root`. Fails with the first that doesn't load, as a
    /// shader that doesn't compile.
    fn load(cx: &mut Context, root: &Path) -> Result<Self, Error> {
        let sound = |cx: &mut Context, name: &str| {
            cx.add_sound(name, root.join(format!("sounds/{name}.wav")))
        };
        let tones = [sound(cx, "bell")?, sound(cx, "pluck")?];
        let (focus, press) = (sound(cx, "tick")?, sound(cx, "click")?);
        let music = cx.add_music("loop", root.join("music/loop.ogg"))?;
        let font = cx.add_font("Inter Bold", root.join("fonts/Inter-Bold.ttf"))?;
        let plasma = cx.add_shader("plasma", root.join("shaders/plasma.slang"), Blend::Opaque)?;
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|[u, v]| Corner {
            position: Vec3::new(u - 0.5, v - 0.5, 0.0),
            uv: Vec2::new(u, v),
        });
        let square = cx.add_mesh("plasma", &corners, &[0, 1, 2, 0, 2, 3]);
        let square = square.expect("the square is two whole triangles");
        let menu = cx.add_ui("menu", root.join("ui/menu.ui"))?;
        let widgets = cx.add_ui("widgets", root.join("ui/widgets.ui"))?;
        Ok(Self {
            tones,
            focus,
            press,
            music,
            font,
            square,
            plasma,
            menu,
            widgets,
        })
    }
}

/// Registers the game's own components with `world`, so that its scenes, the Inspector's Add
/// Component and the tool API know them before any entity has one.
fn register(world: &mut World) {
    let registered = world.register::<Chime>().and_then(|()| {
        world.register::<Burster>()?;
        world.register::<Jukebox>()?;
        world.register::<Hud>()?;
        world.register::<Menu>()?;
        world.register::<Widgets>()?;
        world.register::<Plasma>()?;
        world.register::<Look>()?;
        world.register::<StateLabel>()?;
        world.register::<Tether>()?;
        world.register::<Launch>()
    });
    registered.expect("the game's components describe themselves");
}

/// The game's layer: it loads the scripts as it loads, which declares their components, and
/// registers its own, so that its scenes, the Inspector's Add Component and the tool API know them
/// before any entity has one, loads its sounds, music, font and shader, and has the navmesh and
/// agents' paths show, of the scenes that have them; loads the start scene as it attaches; and at
/// each tick applies what the keys, the mouse and the HUD's button did to its scenes' walkers,
/// looks and HUDs, and the scene Page Up or Page Down asked for, then runs the scripts, rings the
/// chimes, bursts the bursters' emitters, counts the HUDs' and menus' ticks, fires the launches'
/// bodies, steps animation, navigation and then physics, places the tethers, gives the plasmas
/// their draws and writes the state labels' text. As it updates, it grabs the cursor while it
/// wants it for a look, plays the music while a jukebox is in the world, and draws the HUD while a
/// HUD is. A script, an asset or the start scene that doesn't load ends the run with its error,
/// through [`Context::fail`].
struct Showcase {
    scripts: Scripts,
    actions: Actions<Act>,
    commands: Vec<Command>,
    /// Once loaded.
    assets: Option<Assets>,
    /// The chimes that rang at a tick, gathered before they play, the plasmas' draws, before
    /// they're given, and the tethers' places, before they're placed: kept, so that ticks reuse
    /// their room.
    rang: Vec<(Entity, Tone)>,
    drawn: Vec<(Entity, MeshDraw)>,
    tethered: Vec<(Entity, Transform)>,
    /// The HUD's numbers' text, kept so that frames reuse its room.
    text: String,
    /// The state labels found at a tick, each with the entity it tells of, and the text it says,
    /// kept so that ticks reuse their room.
    labeled: Vec<(Entity, Option<Entity>)>,
    label: String,
    /// Whether it wants the cursor grabbed, for the looks: from a click in the view while the world
    /// has one until Escape, or until it has none.
    grab: bool,
    /// The scenes, by their paths from the game's directory, in order, which Page Up and Page Down
    /// step through from the one the world's entities came from, as the last change put it in, or
    /// else the start scene.
    scenes: Vec<String>,
}

impl Showcase {
    fn new() -> Self {
        let mut actions = Actions::default();
        let arrows = Binding::Axis2 {
            x: [Key::ArrowLeft, Key::ArrowRight].map(Source::Key),
            y: [Key::ArrowDown, Key::ArrowUp].map(Source::Key),
        };
        let bound = [
            (Act::Walk, Binding::WASD),
            (Act::Walk, arrows),
            (Act::Walk, Binding::stick(Stick::Left)),
            (Act::Jump, Binding::Button(Source::Key(Key::Space))),
            (
                Act::Jump,
                Binding::Button(Source::PadButton(PadButton::South)),
            ),
        ]
        .into_iter()
        .try_for_each(|(action, binding)| actions.bind(action, binding));
        bound.expect("the game's bindings are each of one kind, of other sources");
        Self {
            scripts: Scripts::new().expect("Luau starts"),
            actions,
            commands: Vec::new(),
            assets: None,
            rang: Vec::new(),
            drawn: Vec::new(),
            tethered: Vec::new(),
            text: String::new(),
            labeled: Vec::new(),
            label: String::new(),
            grab: false,
            scenes: Vec::new(),
        }
    }

    /// Rings each of `world`'s chimes whose time has come, from its entity, flashing its circle.
    fn ring(&mut self, world: &mut World) {
        let dt = Tick::DT.as_secs_f32();
        for (entity, chime) in world.query_mut::<(Entity, &mut Chime)>() {
            chime.time += dt;
            if chime.time >= chime.every {
                chime.time -= chime.every;
                self.rang.push((entity, chime.tone));
            }
        }
        for (entity, tone) in self.rang.drain(..) {
            if let Some(assets) = &self.assets {
                world.play_sound(assets.tones[tone as usize], Place::On(entity));
            }
            let flash = Tween {
                curve: Curve::Quad,
                ease: Ease::Out,
                ..Tween::new("Circle", "thickness", 1.0, RING, 0.6)
            };
            // A chime without a circle rings without one.
            let _ = world.tween(entity, flash);
        }
    }

    /// Bursts each of `world`'s bursters' emitters whose time has come.
    fn burst(world: &mut World) {
        let dt = Tick::DT.as_secs_f32();
        for (burster, emitter) in world.query_mut::<(&mut Burster, &mut ParticleEmitter)>() {
            burster.time += dt;
            if burster.time >= burster.every {
                burster.time -= burster.every;
                emitter.bursts = emitter.bursts.wrapping_add(1);
            }
        }
    }

    /// Fires each of `world`'s launches' bodies whose time has come.
    fn launch(world: &mut World) {
        let dt = Tick::DT.as_secs_f32();
        let launches = world.query_mut::<(&mut Launch, &mut Transform, &mut RigidBody3D)>();
        for (launch, transform, body) in launches {
            launch.time += dt;
            if launch.time >= launch.every {
                launch.time -= launch.every;
                transform.translation = launch.from;
                transform.rotation = Quat::IDENTITY;
                body.velocity = launch.velocity;
                body.angular_velocity = Vec3::ZERO;
            }
        }
    }

    /// Places each of `world`'s tethers from one anchor of its joint to the other, as its bodies
    /// are now, where that moves it: none of a joint whose bodies aren't both there.
    fn tether(&mut self, world: &mut World) {
        let anchored = |body: Option<Entity>, anchor: Vec3| {
            let transform = world.get::<Transform>(body?)?;
            Some(transform.matrix().transform_point3(anchor))
        };
        for (entity, tether, joint) in world.query::<(Entity, &Tether, &Joint3D)>() {
            let (Some(from), Some(to)) = (
                anchored(joint.body_a, joint.anchor_a),
                anchored(joint.body_b, joint.anchor_b),
            ) else {
                continue;
            };
            let along = to - from;
            let placed = Transform {
                translation: (from + to) * 0.5,
                rotation: Quat::from_rotation_arc(Vec3::Y, along.normalize_or(Vec3::Y)),
                scale: Vec3::new(tether.thickness, along.length(), tether.thickness),
            };
            if world.get::<Transform>(entity) != Some(&placed) {
                self.tethered.push((entity, placed));
            }
        }
        for (entity, placed) in self.tethered.drain(..) {
            world
                .insert(entity, placed)
                .expect("the tether's entity, found this tick");
        }
    }

    /// Gives each of `world`'s plasmas its draw with the showcase's shader, as its fields are now.
    fn shade(&mut self, world: &mut World) {
        let Some(assets) = &self.assets else {
            return;
        };
        for (entity, plasma) in world.query::<(Entity, &Plasma)>() {
            let waves = Waves {
                tint: plasma.tint,
                cells: plasma.cells,
                speed: plasma.speed,
                pad: Vec2::ZERO,
            };
            let draw = MeshDraw::new(assets.square, assets.plasma, waves);
            if world.get::<MeshDraw>(entity) != Some(&draw) {
                self.drawn.push((entity, draw));
            }
        }
        for (entity, draw) in self.drawn.drain(..) {
            world
                .insert(entity, draw)
                .expect("the plasma's entity, found this tick");
        }
    }

    /// Has each of `world`'s state labels say its state machine's state and speed, where that
    /// changed.
    fn label(&mut self, world: &mut World) {
        let labels = world.query::<(Entity, &StateLabel)>();
        self.labeled
            .extend(labels.map(|(entity, label)| (entity, label.of)));
        for (entity, of) in self.labeled.drain(..) {
            let Some(machine) = of.and_then(|of| world.get::<AnimationStates>(of)) else {
                continue;
            };
            let speed = match machine.param("speed") {
                Some(ParamValue::Float(speed)) => speed,
                _ => 0.0,
            };
            self.label.clear();
            let state = &machine.state;
            write!(self.label, "{state}  ·  speed {speed:.1}").expect("a string takes any text");
            let changed = world.get::<Text>(entity);
            if changed.is_some_and(|text| text.text != self.label)
                && let Some(text) = world.get_mut::<Text>(entity)
            {
                text.text.clone_from(&self.label);
            }
        }
    }
}

/// Applies `command` to `world`: to each of its walkers, the entities with `walk.luau`'s `Walk`, its
/// heading, with forward along -Z, or its jump; to each of its HUDs, a click; to each of its menus,
/// a press, and to each of its widgets, a change; or to each of its looks, a turn, which turns its
/// entity.
fn apply(world: &mut World, command: &Command) {
    let (field, value) = match *command {
        Command::Walk { heading } => (
            "heading",
            Value::Vec3(Vec3::new(heading.x, 0.0, -heading.y)),
        ),
        Command::Jump => ("jump", Value::Bool(true)),
        Command::Click => {
            for hud in world.query_mut::<&mut Hud>() {
                hud.clicks = hud.clicks.saturating_add(1);
            }
            return;
        }
        Command::Press { ref button } => {
            for menu in world.query_mut::<&mut Menu>() {
                menu.presses = menu.presses.saturating_add(1);
                menu.pressed.clone_from(button);
            }
            return;
        }
        Command::Change {
            ref widget,
            ref value,
        } => {
            for widgets in world.query_mut::<&mut Widgets>() {
                widgets.changes = widgets.changes.saturating_add(1);
                widgets.changed.clear();
                widgets.changed.push_str(widget);
                widgets.changed.push(' ');
                widgets.changed.push_str(value);
            }
            return;
        }
        Command::Scene { ref path } => {
            world.change_scene(path);
            return;
        }
        Command::Look { by } => {
            for (look, transform) in world.query_mut::<(&mut Look, &mut Transform)>() {
                // Right turns to the right, about the world's up, and down looks down.
                look.yaw -= by.x * look.sensitivity;
                look.pitch = (look.pitch - by.y * look.sensitivity).clamp(-MAX_PITCH, MAX_PITCH);
                transform.rotation =
                    Quat::from_rotation_y(look.yaw) * Quat::from_rotation_x(look.pitch);
            }
            return;
        }
    };
    let walkers: Vec<_> = match world.entities(Some("Walk")) {
        Ok(walkers) => walkers.collect(),
        Err(_) => return,
    };
    for walker in walkers {
        // A walker whose script holds no such field, as one edited to drop it, goes without.
        let _ = world.set_field(walker, "Walk", field, &value);
    }
}

impl Layer for Showcase {
    fn load(&mut self, cx: &mut Context) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        // Each is watched as the game runs, in development builds, which reload it as it's saved.
        for script in SCRIPTS {
            if let Err(error) = self.scripts.load(cx, root.join(script)) {
                cx.fail(error);
                return;
            }
        }
        register(cx.world_mut());
        self.scenes = scenes(cx, root);
        // Only a scene with a navmesh volume, `navigation.ron`, has any to show.
        cx.set_navmesh_shown(true);
        match Assets::load(cx, root) {
            Ok(assets) => self.assets = Some(assets),
            Err(error) => cx.fail(error),
        }
    }

    fn attach(&mut self, cx: &mut Context) {
        // A run that ends as it loads loads no scene, whose components the scripts declare.
        if cx.exiting() {
            return;
        }
        // Watched as the game runs, in development builds, which apply its saves.
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
        }
    }

    /// Lets go of what may have been held as the world was taken.
    fn restored(&mut self, _: &mut Context) {
        let heading = Vec2::ZERO;
        self.commands.push(Command::Walk { heading });
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for command in self.commands.drain(..) {
            apply(world, &command);
        }
        self.scripts.run(world);
        self.ring(world);
        Self::burst(world);
        for hud in world.query_mut::<&mut Hud>() {
            hud.ticks = hud.ticks.saturating_add(1);
        }
        for menu in world.query_mut::<&mut Menu>() {
            menu.ticks = menu.ticks.saturating_add(1);
        }
        Self::launch(world);
        step_animation(world);
        step_navigation(world);
        step_physics(world);
        self.tether(world);
        self.shade(world);
        self.label(world);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    /// Grabs the cursor while it wants it for the looks, plays the music while a jukebox is in the
    /// world, and draws the HUD while a HUD is.
    fn update(&mut self, cx: &mut Context, _: Duration) {
        self.grab &= cx.world().query::<&Look>().next().is_some();
        cx.set_cursor_grabbed(self.grab);
        let Some(assets) = &self.assets else {
            return;
        };
        let (music, font) = (assets.music, assets.font);
        let jukebox = cx.world().query::<&Jukebox>().next().copied();
        cx.play_music(jukebox.map(|_| music), MUSIC_FADE);
        if let Some(Jukebox { volume }) = jukebox
            && volume.is_finite()
        {
            cx.set_volume(Bus::Music, volume, VOLUME_FADE);
        }
        if let Some(hud) = cx.world().query::<&Hud>().next().copied() {
            draw_hud(cx, hud, font, &mut self.text);
        }
        let (ui, widgets_ui) = (assets.menu, assets.widgets);
        let menu = cx.world().query::<&Menu>().next().cloned();
        ui.set_input(cx, menu.is_some());
        if let Some(menu) = menu {
            drive_menu(cx, ui, &menu, &mut self.text);
        }
        // Read in place, its record written into the text that shows it, so that a frame copies
        // none of the world's strings.
        let widgets = cx.world().query::<&Widgets>().next();
        let shown = widgets.map(|widgets| write_changed(widgets, &mut self.text));
        widgets_ui.set_input(cx, shown.is_some());
        if shown.is_some() {
            drive_widgets(cx, widgets_ui, &mut self.text);
        }
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, rather than at the next update, so that the next tick applies them.
        let commands = &mut self.commands;
        self.actions.event(event, |change| match change {
            Change::Axis2(Act::Walk, heading) => commands.push(Command::Walk { heading }),
            Change::Pressed(Act::Jump) => commands.push(Command::Jump),
            _ => {}
        });
        let click = Event::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        };
        if *event == click
            && cx.world().query::<&Hud>().next().is_some()
            && cx
                .canvas([Align::Center; 2])
                .cursor()
                .is_some_and(on_button)
        {
            commands.push(Command::Click);
        }
        if let Some(assets) = &self.assets {
            // This run's own, as a lockstep peer's menu is: a press reaches the world as a command.
            let (menu, focus, press) = (assets.menu, assets.focus, assets.press);
            match menu.event(cx, event) {
                Some(UiEvent::Pressed(button)) => {
                    let button = button.to_string();
                    commands.push(Command::Press { button });
                    cx.play_sound(press, Place::Nowhere);
                }
                Some(UiEvent::Back) => {
                    let button = "back".to_owned();
                    commands.push(Command::Press { button });
                }
                Some(UiEvent::Focused(_)) => cx.play_sound(focus, Place::Nowhere),
                Some(UiEvent::Changed(..)) | None => {}
            }
            // As the menu's: a change or a press reaches the world as a command.
            let change = match assets.widgets.event(cx, event) {
                Some(UiEvent::Changed(widget, value)) => Some((widget, said(&value))),
                Some(UiEvent::Pressed(widget)) => {
                    cx.play_sound(press, Place::Nowhere);
                    Some((widget, "pressed".to_owned()))
                }
                Some(UiEvent::Focused(_)) => {
                    cx.play_sound(focus, Place::Nowhere);
                    None
                }
                Some(UiEvent::Back) | None => None,
            };
            if let Some((widget, value)) = change {
                let widget = widget.to_string();
                commands.push(Command::Change { widget, value });
            }
        }
        let looks = cx.world().query::<&Look>().next().is_some();
        match *event {
            Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            } if looks && !cx.pointer_over_ui() => self.grab = true,
            Event::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => self.grab = false,
            Event::Key {
                key: key @ (Key::PageUp | Key::PageDown),
                pressed: true,
                ..
            } if !self.scenes.is_empty() => {
                // From the scene a change put in, which a change refused leaves in.
                let count = self.scenes.len();
                let shown = cx.world().scene().unwrap_or(START);
                let at = self.scenes.iter().position(|scene| scene == shown);
                let by = if key == Key::PageDown { 1 } else { count - 1 };
                let at = at.map_or(0, |at| (at + by) % count);
                let path = self.scenes[at].clone();
                commands.push(Command::Scene { path });
            }
            // Each tick turns the looks once, by the motion since the tick before.
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

/// Draws the menu's UI, `ui`, as `menu` has it, writing its texts through `text`: its clock, the
/// minutes and seconds the menu has been in the world, its presses, the last pressed and the button
/// with the focus, each by its name in capitals, a space for each `_`. A node the UI's file no
/// longer names, as an edit of it may take one away, goes unset: the file says what shows.
fn drive_menu(cx: &mut Context, ui: GameUi, menu: &Menu, text: &mut String) {
    let seconds = (Tick::DT * menu.ticks).as_secs();
    text.clear();
    let written = write!(text, "{:02}:{:02}", seconds / 60, seconds % 60);
    written.expect("a string takes any text");
    let _ = ui.set_text(cx, "clock", text);
    text.clear();
    write!(text, "{}", menu.presses).expect("a string takes any text");
    let _ = ui.set_text(cx, "presses", text);
    let label = |text: &mut String, name: &str| {
        text.clear();
        let capitals = name.chars().map(|c| match c {
            '_' => ' ',
            c => c.to_ascii_uppercase(),
        });
        text.extend(capitals);
    };
    match menu.pressed.as_str() {
        "" => label(text, "nothing yet"),
        pressed => label(text, pressed),
    }
    let _ = ui.set_text(cx, "pressed", text);
    match ui.focused(cx) {
        Some(focused) => label(text, &focused),
        None => label(text, "none"),
    }
    let _ = ui.set_text(cx, "focus", text);
    ui.draw(cx);
}

/// Draws the widgets' UI, `ui`, its last line `text`, as [`write_changed`] wrote it, and the
/// volume's label the slider's value as a percentage, written through `text` after. A node the
/// UI's file no longer names goes unset, as the menu's do.
fn drive_widgets(cx: &mut Context, ui: GameUi, text: &mut String) {
    let _ = ui.set_text(cx, "changed", text);
    if let Ok(UiValue::Number(volume)) = ui.value(cx, "volume") {
        text.clear();
        let written = write!(text, "{:.0}%", volume * 100.0);
        written.expect("a string takes any text");
        let _ = ui.set_text(cx, "volume_value", text);
    }
    ui.draw(cx);
}

/// Writes what `widgets` says changed last into `text`, in capitals, a space for each `_`, and how
/// many changes there have been, as the widgets' UI's last line says it.
fn write_changed(widgets: &Widgets, text: &mut String) {
    text.clear();
    if widgets.changed.is_empty() {
        text.push_str("NOTHING CHANGED YET");
    } else {
        let capitals = widgets.changed.chars().map(|c| match c {
            '_' => ' ',
            c => c.to_ascii_uppercase(),
        });
        text.extend(capitals);
        let changes = if widgets.changes == 1 {
            "CHANGE"
        } else {
            "CHANGES"
        };
        let written = write!(text, "  ·  {} {changes}", widgets.changes);
        written.expect("a string takes any text");
    }
}

/// What a widget's `value` is, as [`Widgets`] records it: a slider's to two places, a toggle's on
/// or off, a drop-down's choice by its name, as [`DIFFICULTIES`] has them, and a text field's text.
fn said(value: &UiValue) -> String {
    match value {
        UiValue::Number(number) => format!("{number:.2}"),
        UiValue::On(on) => (if *on { "on" } else { "off" }).to_owned(),
        UiValue::Choice(choice) => DIFFICULTIES.get(*choice).unwrap_or(&"?").to_string(),
        UiValue::Text(text) => text.clone(),
    }
}

/// Whether `at`, on the canvas from its middle, is on the HUD's button.
fn on_button(at: Vec2) -> bool {
    let (middle, half) = BUTTON;
    let off = (at - middle).abs();
    off.x <= half.x && off.y <= half.y
}

/// Draws `hud` on the view's canvas, in `font`, writing its numbers through `text`: its labels at
/// the canvas's edges and corners; and in its middle, its title, the ticks it has counted, its
/// button, lit while the cursor is over it, and the clicks on it.
fn draw_hud(cx: &mut Context, hud: Hud, font: Font, text: &mut String) {
    let style = |size: f32, color: Vec4, align: [Align; 2]| TextStyle {
        font: Some(font),
        size,
        color,
        align,
    };
    let at = |x: f32, y: f32| Mat4::from_translation(Vec3::new(x, y, 0.0));
    let dim = Vec4::new(0.62, 0.66, 0.78, 1.0);
    for (anchor, label) in ANCHORED {
        // Inward from the edges it's anchored to: across from the left or the right, and down from
        // the top or up from the bottom.
        let inward = |align: Align, toward: f32| match align {
            Align::Start => toward * HUD_INSET,
            Align::Center => 0.0,
            Align::End => -toward * HUD_INSET,
        };
        let place = at(inward(anchor[0], 1.0), inward(anchor[1], -1.0));
        cx.canvas(anchor)
            .draw_text(place, label, &style(30.0, dim, anchor));
    }
    let mut canvas = cx.canvas([Align::Center; 2]);
    let middle = [Align::Center; 2];
    let title = style(52.0, Vec4::ONE, middle);
    canvas.draw_text(at(0.0, 260.0), "A GAME'S UI, ON THE VIEW'S CANVAS", &title);
    let write = |text: &mut String, label: &str, value: u32| {
        text.clear();
        write!(text, "{label} {value}").expect("a string takes any text");
    };
    write(text, "TICKS", hud.ticks);
    canvas.draw_text(at(0.0, 150.0), text, &style(40.0, dim, middle));
    let lit = canvas.cursor().is_some_and(on_button);
    let (fill, ink) = if lit {
        (Vec4::new(0.95, 0.55, 0.15, 1.0), Vec4::ONE)
    } else {
        (Vec4::new(1.0, 1.0, 1.0, 0.1), Vec4::new(1.0, 1.0, 1.0, 0.8))
    };
    let (button, half) = BUTTON;
    let rect =
        Mat4::from_translation(button.extend(0.0)) * Mat4::from_scale((half * 2.0).extend(1.0));
    let sprite = Sprite {
        color: fill,
        ..Sprite::default()
    };
    canvas.draw_sprite(rect, sprite);
    canvas.draw_text(
        at(button.x, button.y),
        "CLICK ME",
        &style(40.0, ink, middle),
    );
    write(text, "CLICKS", hud.clicks);
    canvas.draw_text(at(0.0, -170.0), text, &style(40.0, dim, middle));
    let hint =
        "PLAY COUNTS THE TICKS AND THE CLICKS: THE LABELS KEEP TO THE VIEW'S EDGES AS IT RESIZES";
    canvas.draw_text(at(0.0, -260.0), hint, &style(24.0, dim, middle));
}
