//! The showcase: a scene per feature the engine draws or simulates, in `scenes/`.
//!
//! The editor's Scenes panel opens each one. While the game plays, Page Down and Page Up step
//! through them in name order, through `World::change_scene`. Each has its own camera, and play
//! starts in `welcome.ron`.
//!
//! # 2D
//!
//! - `sprites.ron`: tints, textures, a sheet's cells, tiling, crisp and smooth sampling, flips and
//!   blends.
//! - `flipbooks.ron`: sheet animation looping, back and forth and once, at its own rates, flipped.
//!   Its sheet, `textures/flipbook.png`, comes from a script outside the repository.
//! - `tweens.ron`: tweens along each curve, of a size, a turn, a color and an alpha. A script pops
//!   a crate.
//! - `shapes.ron`: circles, discs and rings, crisp and faded.
//! - `text.ron`: the engine's font and the project's, aligned, sized, colored and turned.
//! - `hierarchy.ron`: children carried by their parents.
//! - `physics.ron`: 2D boxes and circles, dynamic, kinematic and static.
//! - `shader.ron`: the game's own shader, `shaders/plasma.slang`, reloading as it's saved.
//!
//! # UI
//!
//! - `ui.ron`: a UI drawn by code on the view's canvas: labels at its edges and corners, a tick
//!   count, and a button that lights under the cursor and counts clicks.
//! - `menu.ron`: a UI file, `ui/menu.ui`, which reloads as it's saved. Keys, a pad and the mouse
//!   move the focus over its buttons, to `sounds/tick.wav` and `sounds/click.wav`. Its HUD and
//!   corner panels show the layer's state, and presses reach the world as commands.
//! - `widgets.ron`: `ui/widgets.ui`'s slider, toggle, drop-down, text field and scrolling list,
//!   each change reaching the world as a command.
//!
//! # Sound
//!
//! - `sounds.ron`: sounds from where their entities are: bells left, middle and right
//!   (`sounds/bell.wav`), and a pluck sweeping past the view's edges (`sounds/pluck.wav`).
//! - `music.ron`: `music/loop.ogg`, looping while the scene is open, its volume a field.
//! - `listen.ron`: chimes heard in 3D by a camera the mouse turns. A click grabs the cursor, and
//!   Escape lets it go.
//!
//! Scripts outside the repository synthesized the bell, the pluck, the music, the tick and the
//! click.
//!
//! # Models and animation
//!
//! - `models.ron`: glTF models, unlit, textured and of several nodes.
//! - `skeletal.ron`: Khronos's Fox, skinned, playing Survey, Walk and Run alone, blended and
//!   cross-faded. `models/fox` is CC0 by PixelMannen, rigged and animated by tomkranis, and made
//!   glTF by AsoboStudio and scurest under CC BY 4.0, as its README says.
//! - `states.ron`: the Fox on a state machine, blending by speed round a circle, with a trigger
//!   once a round.
//! - `prefabs.ron`: `prefabs/lantern.prefab` placed four times, three overriding a field, and
//!   `prefabs/lamppost.prefab` linking two. All follow their files as they're saved.
//!
//! # Light and post-processing
//!
//! - `hdr.ron`: colors from a quarter of white's brightness to 16 times, tonemapped.
//! - `bloom.ron`: neon tubes, sparks and a glowing sphere at night.
//! - `grading.ron`: a golden hour's grade: warmer, more contrast and saturation, teal shadows.
//! - `lights.ron`: metallic-roughness materials under a sun, a point light and a spot.
//! - `sky.ron`: the same under a sky alone: Poly Haven's Kloofendal 48d Partly Cloudy Puresky at
//!   1K, CC0.
//! - `atmosphere.ron`: Hillaire's atmosphere through a two-minute day, haze deepening on towers
//!   300 m to 12 km off.
//! - `exposure.ron`: exposure adapting from noon to two lamps at night, faster to light than to
//!   dark.
//! - `shadows.ron`: a sun's shadows.
//! - `ambient_occlusion.ron`: shapes piled in a corner, lit by the Puresky alone.
//! - `reflections.ron`: screen-space reflections in steel, stone and bronze (`models/gloss.gltf`),
//!   blurring with roughness.
//! - `antialiasing.ron`: SMAA on thin slats, rails and a pole, without ghosts.
//! - `depth_of_field.ron`: a focus at 7 m, as through a lens of f/1.4.
//! - `motion_blur.ron`: a carousel, a spinning crate and a tumbling torus, through a 180° shutter.
//! - `lens.ron`: vignette, chromatic aberration and grain.
//! - `particles.ron`: a fountain blended by alpha, a fire adding light over its smoke, a burster's
//!   sparks, and a comet's trail.
//!
//! # Physics, navigation and worlds
//!
//! - `physics3d.ron`: boxes, balls and capsules, dynamic, kinematic and static, down a ramp.
//! - `joints.ron`: revolute, spherical and prismatic joints, motors and limits, a rope, and
//!   continuous collision against a thin turning plate.
//! - `navigation.ron`: a navmesh baked from a maze, a dozen agents walking it, shown as lines.
//! - `cells.ron`: an 8 by 8 grid of 64 m cells in `scenes/cells`, streaming in and out round a
//!   walking character.
//! - `world.ron`: 1,024 m cells reaching 1,000 km out, which [`world_cell`] makes. Two fliers
//!   1,000 km apart cross it in 32 s, each a viewer. A gate of the cells' models at the east one's
//!   start keeps them loaded, as a server's session loads nothing as it plays.
//!
//! # Sponza and Bistro
//!
//! These need `cargo gg fetch` first.
//!
//! - `sponza.ron`: Sponza in a low fog under its sun's sky, with AO, bloom, grading, reflections,
//!   adapting exposure and a light lens.
//! - `fog.ron`: Sponza in a height fog that the Puresky and a sun light.
//! - `walk.ron`: a character walking Sponza, colliding with its triangles and pushing crates.
//! - `bistro.ron`: Amazon Lumberyard Bistro: specular-glossiness materials, BC7 DDS images, its sun
//!   and 96 point lights.
//! - `bistro_grid.ron`: 16 Bistros, 4 by 4, each with its lights, exposed 4 stops down.
//!
//! The Luau scripts in `scripts/` each say at their top what they do.
//!
//! The reference player runs the package without this code. Animation, navigation and physics still
//! step, but nothing the code registers or makes is there: the walking, the components this file
//! defines, and the world's cells.

// A build without `dev`, as it ships, shows no console on Windows. Its JSON log still records the
// run, and output piped to another process still arrives.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::fmt::Write as _;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use bytemuck::NoUninit;
use gg_engine::math::IVec3;
use gg_engine::prelude::*;
use gg_engine::reflect::Value;
use gg_engine::{
    AnimationStates, Cell, Curve, Ease, GameUi, Joint3D, MeshDraw, Music, ParamValue,
    ParticleEmitter, RigidBody3D, Text, Tween, UiEvent, UiValue,
};

#[cfg(test)]
mod tests;

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, as `main` runs it and its recordings replay it.
fn app() -> App {
    let app = App::new("Showcase")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
    let app = app.layer(Showcase::new()).overlay(gg_engine::PerfPanel);
    app.overlay(gg_engine::Inspector::default())
}

/// The game's scripts, by their paths from its directory. Each declares the component it is.
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
    Walk,
    Jump,
}

/// What the keys, a pad and the mouse do, queued for the next tick. Recordings keep them.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Sets every walker's heading until the next: across turns, up walks forward.
    Walk { heading: Vec2 },
    /// Has every walker jump, if it stands.
    Jump,
    /// Counts a click on the HUD's button, on each [`Hud`].
    Click,
    /// Turns each [`Look`] by the mouse's raw motion, right and down, while the cursor is grabbed.
    Look { by: Vec2 },
    /// Records a press of the menu's button named `button`, or `back` for back, on each [`Menu`].
    Press { button: String },
    /// Records on each [`Widgets`] that the widget named `widget` changed to `value`, or was
    /// pressed.
    Change { widget: String, value: String },
    /// Replaces the world's entities with the scene at `path`, from the game's directory.
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

/// Rings its tone from its entity every `every` seconds of the world's ticks: `sounds.ron`'s and
/// `listen.ron`'s chimes. A `Circle` on the entity flashes as it rings.
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

/// Bursts its entity's [`ParticleEmitter`] every `every` seconds of the world's ticks, by raising
/// the emitter's count of bursts: `particles.ron`'s sparks.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Burster {
    /// The seconds between its bursts.
    #[reflect(min = 0.05, default = 1.0)]
    every: f32,
    /// The seconds since it last burst; negative, a wait before its first.
    time: f32,
}

/// Plays `music/loop.ogg` while its entity is in the world: `music.ron`'s. The music fades in,
/// fades out once no jukebox is left, and plays on through a pause.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Jukebox {
    /// How loud the music's bus plays it: 1 as loud as the music is, 0 silent.
    #[reflect(min = 0.0, max = 1.0, default = 1.0)]
    volume: f32,
}

/// Has the showcase draw its HUD on the view's canvas while its entity is in the world: `ui.ron`'s.
/// Clicks on its button reach the world as commands.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Hud {
    /// The ticks it has been in the world for.
    ticks: u32,
    /// The clicks on its button.
    clicks: u32,
}

/// Has the showcase draw `ui/menu.ui` and give it input while its entity is in the world:
/// `menu.ron`'s. Its presses reach the world as commands, which record them here.
#[derive(Clone, Debug, PartialEq, Component)]
struct Menu {
    /// The ticks it has been in the world for.
    ticks: u32,
    /// The presses of its buttons, and of back, and the name of the last pressed.
    presses: u32,
    pressed: String,
}

/// Has the showcase draw `ui/widgets.ui` and give it input while its entity is in the world:
/// `widgets.ron`'s. Their changes reach the world as commands, which record the last here.
#[derive(Clone, Debug, PartialEq, Component)]
struct Widgets {
    /// How many changes and presses there have been, then the last: a widget's name and value.
    changes: u32,
    changed: String,
}

/// The widgets' drop-down's options, as `ui/widgets.ui` names them.
const DIFFICULTIES: [&str; 4] = ["EASY", "NORMAL", "HARD", "NIGHTMARE"];

/// Has the game's shader, `shaders/plasma.slang`, draw its entity as a unit square of drifting
/// waves, once the world ticks: `shader.ron`'s.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Plasma {
    /// What the waves' colors are multiplied by, in linear RGB. Its alpha is unused.
    #[reflect(default = Vec4::ONE)]
    tint: Vec4,
    /// How many waves cross the square.
    #[reflect(min = 0.0, default = 1.0)]
    cells: f32,
    /// How fast the waves drift, 1 a radian a second.
    #[reflect(default = 1.0)]
    speed: f32,
}

/// Turns its entity, a camera, by the mouse while the cursor is grabbed: `listen.ron`'s. A click in
/// the view grabs the cursor, and Escape lets it go.
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

/// Has its entity's [`Text`] give `of`'s state machine's state and `speed`, as `Move  ·  speed 3.3`:
/// `states.ron`'s.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct StateLabel {
    /// The entity whose [`AnimationStates`] it tells of.
    of: Option<Entity>,
}

/// Stretches its entity, a model a unit tall along Y, between the anchors of the [`Joint3D`] on it:
/// `joints.ron`'s rope.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Tether {
    /// How wide and deep it is.
    #[reflect(min = 0.0, default = 0.03)]
    thickness: f32,
}

/// Fires its entity, a body, from `from` every `every` seconds of the world's ticks: `joints.ron`'s
/// balls, fired at a thin plate.
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

/// A corner of the plasma's square, laid out as `shaders/plasma.slang` reads it.
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

/// The thickness a chime's ring thins back to after its flash.
const RING: f32 = 0.12;

/// How long music takes to fade in as it starts and out as it stops.
const MUSIC_FADE: Duration = Duration::from_millis(1500);

/// How long the music's bus takes to reach a jukebox's volume.
const VOLUME_FADE: Duration = Duration::from_millis(100);

/// How far the HUD's labels are from the canvas's edges, in its units.
const HUD_INSET: f32 = 36.0;

/// The HUD's edge and corner labels, by anchor, across then down.
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

/// The start scene, as the project's manifest names it.
const START: &str = "scenes/welcome.ron";

/// The folder of `world.ron`'s cells. It holds no file: [`world_cell`] makes each cell.
const WORLD_FOLDER: &str = "scenes/world";

/// The world's first and last cells, each 1,024 m on a side: 977 along x and z, reaching 1,000 km
/// from the origin, at y 0.
const WORLD_CELLS: [[i32; 3]; 2] = [[0, 0, 0], [976, 0, 976]];

/// The props a cell of the world holds, by kind: its name, its model, and its middle's height over
/// the ground by its size.
const PROPS: [(&str, &str, f64); 4] = [
    ("Crate", "11d32c53785146e8", 0.5),
    ("Torus", "25ea9ac40f5d8623", 1.0),
    ("Ball", "fbca002a5a685a47", 0.5),
    ("Crystal", "48899f3b379ae447", 0.5),
];

/// Cell `cell` of the world, as a canonical scene file would hold it: a tile of ground to stand on
/// and four props, which a hash of the cell picks.
///
/// Each entity's ID is the cell's place and its number, so every run, peer and platform makes the
/// cell the same, and no other entity shares an ID. Each entity stands within the cell, as a
/// streamed root belongs to the cell it stands in, so the cell goes out keeping nothing.
fn world_cell(cell: Cell) -> String {
    let [x, _, z] = cell.0.to_array();
    let place = u64::from(x.unsigned_abs()) << 28 | u64::from(z.unsigned_abs()) << 8;
    let id = |n: u64| format!("{:016x}", 0x5ce2_0000_0000_0000 | place | n);
    let [x, z] = [x, z].map(|n| f64::from(n) * 1024.0);
    let mut text = String::from("(\n    entities: {\n");
    write!(
        text,
        r#"        "{id}": (
            BoxCollider3D: (
                offset: (0.0, 0.0, 0.0),
                size: (1.0, 1.0, 1.0),
                density: 1.0,
                friction: 0.5,
                restitution: 0.0,
                sensor: false,
                membership: 4294967295,
                filter: 4294967295,
            ),
            ModelRoot: ("f1335e6995be98d7"),
            Name: ("Ground"),
            RigidBody3D: (
                body_type: Static,
                fixed_rotation: false,
                continuous_collision: false,
                velocity: (0.0, 0.0, 0.0),
                angular_velocity: (0.0, 0.0, 0.0),
            ),
            Transform: (
                translation: ({x:?}, 0.0, {z:?}),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (1024.0, 1.0, 1024.0),
            ),
        ),
"#,
        id = id(0),
        x = x + 512.0,
        z = z + 512.0,
    )
    .expect("a string takes any text");
    for n in 1..=4 {
        let hash = mix(place | n);
        let (name, model, up) = PROPS[(hash % 4) as usize];
        let [east, south] = [8, 24].map(|at| ((hash >> at) % 896 + 64) as f64);
        let size = ((hash >> 40) % 17 + 8) as f64;
        write!(
            text,
            r#"        "{id}": (
            ModelRoot: ("{model}"),
            Name: ("{name}"),
            Transform: (
                translation: ({x:?}, {y:?}, {z:?}),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: ({size:?}, {size:?}, {size:?}),
            ),
        ),
"#,
            id = id(n),
            x = x + east,
            // On the ground, whose top is half a unit up.
            y = 0.5 + size * up,
            z = z + south,
        )
        .expect("a string takes any text");
    }
    text + "    },\n)\n"
}

/// SplitMix64's step of `seed`: a hash, the same on every platform.
fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The game's scenes, by their paths from `root`, in name order: the package's in a run from one
/// (a build without `dev`), else the scenes folder's.
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
    /// The menu's and widgets' cues, quiet and short: `focus` as the focus moves, `press` as a
    /// button is pressed.
    focus: Sound,
    press: Sound,
    music: Music,
    /// Inter Bold, for the HUD.
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

/// The game's layer. A script, an asset or the start scene that doesn't load ends the run, through
/// [`Context::fail`].
struct Showcase {
    scripts: Scripts,
    actions: Actions<Act>,
    commands: Vec<Command>,
    /// Once loaded.
    assets: Option<Assets>,
    /// Each tick's scratch, kept so ticks reuse its room.
    rang: Vec<(Entity, Tone)>,
    drawn: Vec<(Entity, MeshDraw)>,
    tethered: Vec<(Entity, Transform)>,
    /// The HUD's numbers' text, kept so that frames reuse its room.
    text: String,
    /// Each tick's scratch for the state labels, kept so ticks reuse its room.
    labeled: Vec<(Entity, Option<Entity>)>,
    label: String,
    /// Whether a look wants the cursor grabbed: from a click in the view until Escape, or until no
    /// look is left.
    grab: bool,
    /// The scenes, in name order, for Page Up and Page Down.
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
                transform.translation = launch.from.as_dvec3();
                transform.rotation = Quat::IDENTITY;
                body.velocity = launch.velocity;
                body.angular_velocity = Vec3::ZERO;
            }
        }
    }

    /// Places each of `world`'s tethers between its joint's anchors, as its bodies are now. A
    /// tether already in place isn't written, and one whose joint lacks a body stays put.
    fn tether(&mut self, world: &mut World) {
        let anchored = |body: Option<Entity>, anchor: Vec3| {
            let transform = world.get::<Transform>(body?)?;
            Some(transform.matrix().transform_point3(anchor.as_dvec3()))
        };
        for (entity, tether, joint) in world.query::<(Entity, &Tether, &Joint3D)>() {
            let (Some(from), Some(to)) = (
                anchored(joint.body_a, joint.anchor_a),
                anchored(joint.body_b, joint.anchor_b),
            ) else {
                continue;
            };
            let along = (to - from).as_vec3();
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

/// Applies `command` to `world`. Walkers are the entities with `walk.luau`'s `Walk`.
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
        // Development builds watch each one and reload it as it's saved.
        for script in SCRIPTS {
            if let Err(error) = self.scripts.load(cx, root.join(script)) {
                cx.fail(error);
                return;
            }
        }
        register(cx.world_mut());
        let [from, to] = WORLD_CELLS.map(|cell| Cell(IVec3::from_array(cell)));
        cx.make_cells(WORLD_FOLDER, from, to, world_cell);
        self.scenes = scenes(cx, root);
        // Only a scene with a navmesh volume, `navigation.ron`, has any to show.
        cx.set_navmesh_shown(true);
        match Assets::load(cx, root) {
            Ok(assets) => self.assets = Some(assets),
            Err(error) => cx.fail(error),
        }
    }

    fn attach(&mut self, cx: &mut Context) {
        // A run ending as it loads loads no scene. The scripts declare the scene's components.
        if cx.exiting() {
            return;
        }
        // Development builds watch the scene and apply its saves.
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
            // The menu is this run's own, as a lockstep peer's is. A press reaches the world as a
            // command.
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
                // From the world's scene: the last change's, which a refused change leaves.
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

/// Draws the menu's UI as `menu` has it, writing its texts through `text`. A node the UI's file no
/// longer names goes unset: the file says what shows.
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

/// Draws the widgets' UI, its last line `text` as [`write_changed`] wrote it. A node the UI's file
/// no longer names goes unset, as the menu's do.
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

/// Writes the widgets' UI's last line into `text`: what changed last, and how many changes there
/// have been.
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

/// A widget's `value`, as [`Widgets`] records it.
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

/// Draws `hud` on the view's canvas in `font`, writing its numbers through `text`.
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
