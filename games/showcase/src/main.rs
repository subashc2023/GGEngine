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
//! - `models.ron`: glTF models, lit, flat and smooth, textured, and unlit of several nodes.
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

mod components;
mod systems;
mod ui;

use std::fmt::Write as _;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use bytemuck::NoUninit;
use gg_engine::math::IVec3;
use gg_engine::prelude::*;
use gg_engine::{Cell, GameUi, MeshDraw, Music, UiEvent};

#[cfg(doc)]
use components::Plasma;
use components::{Hud, Jukebox, Look, Menu, Tone, Widgets, register};
use systems::apply;
use ui::{draw_hud, drive_menu, drive_widgets, on_button, said, write_changed};

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
    app.layer(Showcase::new()).debug_overlays()
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

/// How long music takes to fade in as it starts and out as it stops.
const MUSIC_FADE: Duration = Duration::from_millis(1500);

/// How long the music's bus takes to reach a jukebox's volume.
const VOLUME_FADE: Duration = Duration::from_millis(100);

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

/// The game's layer. A script, an asset or the start scene that doesn't load ends the run.
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
        let bound = actions.bind_all([
            (Act::Walk, Binding::WASD),
            (Act::Walk, Binding::ARROWS),
            (Act::Walk, Binding::stick(Stick::Left)),
            (Act::Jump, Binding::Button(Source::Key(Key::Space))),
            (
                Act::Jump,
                Binding::Button(Source::PadButton(PadButton::South)),
            ),
        ]);
        bound.expect("the game's bindings are each of one kind, of other sources");
        // What `gg actions` says each control does.
        let walk = "Walks every entity with a `Walk` component, in `walk.ron` and `cells.ron`: \
                    [x, y], right and up. Up walks forward along where it faces at `Walk.speed` \
                    units a second; right turns it right at `Walk.turn_speed` radians a second";
        actions.describe(Act::Walk, walk);
        let jump = "Jumps every `Walk` entity that stands, leaving the ground at `Walk.jump_speed` \
                    units a second; a press in the air does nothing";
        actions.describe(Act::Jump, jump);
        let look = "In `listen.ron`, turns the camera while the cursor is grabbed, \
                    `Look.sensitivity` radians a unit: right turns right, lowering `Look.yaw`, and \
                    down looks down, lowering `Look.pitch`. The pitch stops at 89° either way";
        actions.describe_control(Control::Motion, look);
        let click = "In `listen.ron`, a press in the view, off its UI, grabs the cursor. In \
                     `ui.ron`, a press with the cursor on the HUD's button counts a click. In \
                     `menu.ron` and `widgets.ron`, a press chooses the button or widget under the \
                     cursor";
        let left = Control::Source(Source::Button(MouseButton::Left));
        actions.describe_control(left, click);
        let cursor =
            "In `menu.ron` and `widgets.ron`, the cursor over a button or widget focuses it";
        actions.describe_control(Control::Cursor, cursor);
        let escape = Control::Source(Source::Key(Key::Escape));
        let back = "Lets go of the grabbed cursor, so the mouse stops looking. In `menu.ron` it \
                    also presses back";
        actions.describe_control(escape, back);
        let next = "Changes to the next scene in name order, wrapping from the last to the first";
        actions.describe_control(Control::Source(Source::Key(Key::PageDown)), next);
        let before = "Changes to the previous scene in name order, wrapping from the first to the \
                      last";
        actions.describe_control(Control::Source(Source::Key(Key::PageUp)), before);
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
}

impl Layer for Showcase {
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        // Development builds watch each one and reload it as it's saved.
        for script in SCRIPTS {
            self.scripts.load(cx, root.join(script))?;
        }
        register(cx.world_mut())?;
        let [from, to] = WORLD_CELLS.map(|cell| Cell(IVec3::from_array(cell)));
        cx.make_cells(WORLD_FOLDER, from, to, world_cell);
        self.scenes = scenes(cx, root);
        // Only a scene with a navmesh volume, `navigation.ron`, has any to show.
        cx.set_navmesh_shown(true);
        self.assets = Some(Assets::load(cx, root)?);
        Ok(())
    }

    fn attach(&mut self, cx: &mut Context) {
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

    /// What `gg actions` lists, and `gg input` presses by name.
    fn actions(&self) -> Controls {
        self.actions.controls()
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
            drive_widgets(cx, widgets_ui, &self.text);
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
