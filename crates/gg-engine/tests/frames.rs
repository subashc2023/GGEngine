//! Steady-state frames of an app whose layers draw no development UI (egui's allocates) allocate
//! nothing, on any thread.
//!
//! The app draws what games draw: sprites, circles, meshes, glTF models and a skinned Fox; a
//! turning hierarchy and wrapping drifters whose ticks despawn and spawn entities; colliders'
//! outlines; text, also on the canvas; a UI from its file, laid out again each frame, with input
//! that changes nothing, and a second one with a list scrolled and a drop-down open; particles;
//! and an outline. A camera that a pivot carries round sees it under an environment with every
//! effect on: temporal antialiasing, bloom, grade, ambient occlusion, fog, reflections, adapting
//! exposure, depth of field, motion blur, lens, and an atmosphere whose sun the ticks raise, its
//! light prefiltered again each degree. Then the environment switches to SMAA, and frames are
//! counted again once the switch has settled.
//!
//! The run is the engine's main loop headless, without the tool API (development runs' watchers of
//! the assets' files allocate as they look), three fifths of a tick per frame so frames draw
//! between ticks. Counting starts after the first frames, by which every image the attach added has
//! decoded off the main thread and streamed to the GPU.
//!
//! Every frame picks at the view's middle, as the frame before had it, and outside the view.
//! Headless, each answer comes back in the frame it's due: four frames after the ask, two for the
//! pick outside, which finds nothing.
//!
//! It runs in one process with a main of its own, so no other test's threads run beside it. It
//! counts every allocation through Rust's allocator, on any thread; the Vulkan loader, driver,
//! validation layer and VMA go uncounted. It needs a GPU like the GPU tests; `GG_SKIP_GPU_TESTS`
//! skips it. `--windowed` runs the app in a window, for hand checks of frames the mouse moves and
//! clicks over: `cargo test -p gg-engine --test frames -- --windowed`, without `dev` (its watcher
//! of the assets' files allocates) and with no gamepad connected (gilrs allocates).

// For the allocator that counts, in `common`, and for setting the environment; see the SAFETY
// comments.
#![allow(unsafe_code)]

mod common;

use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::fmt::Write as _;
use std::fs;
use std::io::BufWriter;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gg_engine::math::{self, DVec3, Mat4, Quat, Vec2, Vec3, Vec4};
use gg_engine::{
    Actions, Adaptation, Align, Animator, Antialiasing, App, Atmosphere, Binding, Blend,
    BoxCollider2D, Camera, Change, Circle, CircleCollider2D, Component, Context, DepthOfField,
    Entity, Environment, Event, Fog, Font, GameUi, Grade, Key, Layer, Lens, Light, Mesh, MeshDraw,
    Model, MotionBlur, MouseButton, ParticleBlend, ParticleEmitter, Pick, Play, Projection,
    Reflections, Shader, Sky, Source, Sprite, SpriteSheet, Text, TextStyle, Texture, Tick,
    Transform, World, step_animation,
};
use serde_json::json;

use crate::common::Counted;

/// Seeds the world's IDs, so every run draws its entities in one order.
const SEED: u64 = 0x5eed_f2a3;
/// Frames run before counting: a run's first frames size what the engine and the app keep. Three
/// periods of what the ticks move, and many turns of every pool.
const WARM_UP: u64 = 320;
/// Frames counted: more than a period of every motion.
const COUNTED: u64 = 160;
/// Frames run after the switch to SMAA before counting again, as its first frame makes its passes
/// and images; then those counted.
const SMAA_WARM_UP: u64 = 32;
const SMAA_COUNTED: u64 = 64;
/// Every frame the app runs, the last after the last counted.
const FRAMES: u64 = WARM_UP + COUNTED + SMAA_WARM_UP + SMAA_COUNTED + 1;
/// Ticks a drifter takes to return to its start, the parent to turn round, and the camera to go
/// round.
const PERIOD: u64 = 64;
const DRIFTERS: usize = 2_000;
/// The drifters' field, around the origin.
const FIELD: Vec2 = Vec2::new(16.0, 9.0);
/// A pool of sprites with colliders and their children; each tick replaces the oldest `REPLACED`,
/// as a game's bullets and particles come and go.
const SUBTREES: usize = 256;
const REPLACED: usize = 16;
/// A pool of model copies; each tick replaces the oldest.
const COPIES: usize = 8;
/// Frames whose picks the app keeps: more than an answer takes to come back.
const ASKED: usize = 8;
/// Ticks the sun takes to rise a degree, past which its light is prefiltered again: a few times in
/// the frames counted, as prefiltering is slow on lavapipe.
const SUN_TICKS: u64 = 16;
/// A headless run's view, in pixels: small, as allocations are alike at any size, and lavapipe
/// drew 1280 by 720 on CI's two CPUs in about 820 ms a frame, which made this the longest test.
const SIZE: [u32; 2] = [320, 180];

/// Answers that came back: to picks at the view's middle, those that found an entity and those
/// that found none; to picks outside the view, which find none; and those in the wrong frame or
/// finding an entity outside the view.
static FOUND: AtomicU64 = AtomicU64::new(0);
static MISSED: AtomicU64 = AtomicU64::new(0);
static OUTSIDE: AtomicU64 = AtomicU64::new(0);
static ASTRAY: AtomicU64 = AtomicU64::new(0);

/// What the counted frames allocated: antialiased temporally, then by SMAA.
static ALLOCATED: Mutex<[Option<Counted>; 2]> = Mutex::new([None, None]);

/// A drifter: where it starts, how far it goes a tick (a whole number of fields in `PERIOD` ticks,
/// so it's back each period), and its z.
#[derive(Clone, Component)]
struct Drifter {
    start: Vec2,
    velocity: Vec2,
    z: f32,
}

impl Drifter {
    fn new(i: usize) -> Self {
        let mix = |salt: u64| {
            let mut z = (i as u64 ^ salt).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            z = (z ^ (z >> 31)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            (z >> 40) as f32 / (1u64 << 24) as f32
        };
        let ways = [-2.0, -1.0, 1.0, 2.0];
        let way = Vec2::new(ways[i % 4], ways[i / 4 % 4]);
        Self {
            start: Vec2::new(mix(1), mix(2)) * FIELD,
            velocity: way * FIELD / PERIOD as f32,
            z: (i % 4) as f32 * 0.1,
        }
    }

    fn at(&self, tick: u64) -> Transform {
        let moved = self.start + self.velocity * (tick % PERIOD) as f32;
        let at = moved.rem_euclid(FIELD) - FIELD / 2.0;
        Transform {
            translation: at.extend(self.z).as_dvec3(),
            scale: Vec3::splat(0.2),
            ..Transform::IDENTITY
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Spin,
    Jump,
}

/// What the app draws with, added by its attach.
struct Assets {
    quad: Mesh<[f32; 3]>,
    opaque: Shader<[f32; 3], Vec4>,
    blended: Shader<[f32; 3], Vec4>,
    texture: Texture,
    sheet: SpriteSheet,
    /// Opaque, so its copies draw in runs that the order of their IDs doesn't split.
    plain: Model,
    font: Font,
    /// In the view's bottom left corner.
    ui: GameUi,
    /// Another UI of the same file, over it, its list scrolled to its last button and its
    /// drop-down's options open.
    open: GameUi,
}

/// The app: what it spawns as it attaches, and what its ticks move, spawn and despawn.
#[derive(Default)]
struct Scene {
    dir: PathBuf,
    assets: Option<Assets>,
    turning: Option<Entity>,
    /// Carries the camera round.
    pivot: Option<Entity>,
    /// The emitter the ticks burst.
    sparks: Option<Entity>,
    sun: Option<Entity>,
    /// The next tick switches it to SMAA when `smaa` asks.
    environment: Option<Entity>,
    smaa: bool,
    subtrees: VecDeque<Entity>,
    copies: VecDeque<Entity>,
    /// The drifters that wrapped at a tick: empty between ticks.
    wrapped: Mutex<Vec<Entity>>,
    actions: Actions<Act>,
    /// What the actions did since the last tick, as a game's commands: plain data the tick
    /// applies, here by forgetting.
    commands: Vec<Change<Act>>,
    frames: u64,
    /// Tickets of the last frames' picks, by frame number modulo `ASKED`: at the view's middle,
    /// then outside it.
    asked: [Option<[Pick; 2]>; ASKED],
    /// Written again in its memory each frame.
    text: String,
}

/// The atmosphere's sun at tick `tick`: 30° up, a degree higher every [`SUN_TICKS`] ticks.
fn sun_at(tick: u64) -> Transform {
    let up = math::to_radians(30.0 + tick as f32 / SUN_TICKS as f32);
    Transform {
        rotation: Quat::from_rotation_x(-up),
        ..Transform::IDENTITY
    }
}

impl Scene {
    fn assets(&self) -> &Assets {
        self.assets.as_ref().expect("attached")
    }

    /// Spawns the pool's newest subtree: a sprite with a box collider and a sprite child, or in
    /// every eighth `slot` a circle child with a circle collider, disc and ring by turns.
    fn spawn_subtree(&mut self, world: &mut World, slot: usize) {
        let assets = self.assets.as_ref().expect("attached");
        let at = Transform {
            translation: DVec3::new(f64::from(slot as f32 * 0.05), -3.0, 0.5),
            scale: Vec3::splat(0.3),
            ..Transform::IDENTITY
        };
        let sprite = assets.sheet.sprite([1, 1], [1, 1]);
        let root = world.spawn((at, sprite, BoxCollider2D::default()));
        let below = Transform {
            translation: DVec3::new(0.0, -1.0, 0.0),
            ..Transform::IDENTITY
        };
        let child = if slot.is_multiple_of(8) {
            let thickness = if slot.is_multiple_of(16) { 1.0 } else { 0.25 };
            let circle = Circle {
                thickness,
                ..Circle::default()
            };
            world.spawn((below, circle, CircleCollider2D::default()))
        } else {
            let textured = Sprite {
                texture: Some(assets.texture),
                ..Sprite::default()
            };
            world.spawn((below, textured))
        };
        world.set_parent(child, Some(root)).expect("both alive");
        self.subtrees.push_back(root);
    }

    fn spawn_copy(&mut self, world: &mut World, x: f32) {
        let at = Transform {
            translation: DVec3::new(f64::from(x), 3.0, -1.0),
            ..Transform::IDENTITY
        };
        let copy = self.assets().plain.spawn(world, at);
        self.copies.push_back(copy);
    }
}

impl Layer for Scene {
    fn attach(&mut self, cx: &mut Context) {
        let shader = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/flat_color.slang");
        let opaque = cx.add_shader("flat color", &shader, Blend::Opaque);
        let blended = cx.add_shader("flat color, blended", &shader, Blend::Alpha);
        let corners = [
            [-0.5, -0.5, 0.0],
            [0.5, -0.5, 0.0],
            [0.5, 0.5, 0.0],
            [-0.5, 0.5, 0.0],
        ];
        let quad = cx.add_mesh("quad", &corners, &[0, 1, 2, 0, 2, 3]);
        let texture = cx.add_texture("checker", self.dir.join("checker.png"));
        let sheet = cx.add_sprite_sheet("cells", self.dir.join("cells.png"), [8, 8]);
        let pair = cx.add_model("pair", self.dir.join("pair.gltf"));
        let plain = cx.add_model("plain", self.dir.join("plain.gltf"));
        let fox = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models/fox/Fox.gltf");
        let fox = cx.add_model("fox", fox);
        let inter =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/Inter/Inter-Regular.ttf");
        let font = cx.add_font("inter", inter);
        let ui = cx.add_ui("corner", self.dir.join("corner.ui"));
        let open = cx.add_ui("open", self.dir.join("corner.ui"));
        let open = open.expect("the UI reads");
        // Scroll the list by focusing its last button, then open the drop-down.
        open.set_focus(cx, "three").expect("a button");
        open.set_focus(cx, "mode").expect("a drop-down");
        let enter = Event::Key {
            key: Key::Enter,
            pressed: true,
            repeat: false,
        };
        assert_eq!(open.event(cx, &enter), None, "opened");
        // The open options take the down arrow, lighting the second; the focus stays.
        let down = Event::Key {
            key: Key::ArrowDown,
            pressed: true,
            repeat: false,
        };
        assert_eq!(open.event(cx, &down), None);
        assert_eq!(open.focused(cx).as_deref(), Some("mode"), "open");
        let assets = Assets {
            quad: quad.expect("the quad is whole triangles"),
            opaque: opaque.expect("the shader compiles"),
            blended: blended.expect("the shader compiles"),
            texture: texture.expect("the checker loads"),
            sheet: sheet.expect("the cells load"),
            plain: plain.expect("the plain model loads"),
            font: font.expect("Inter bakes"),
            ui: ui.expect("the UI reads"),
            open,
        };
        let pair = pair.expect("the pair loads");
        let fox = fox.expect("the Fox loads");

        let world = cx.world_mut();
        for i in 0..DRIFTERS {
            let drifter = Drifter::new(i);
            let sprite = match i % 4 {
                0 => Sprite {
                    color: Vec4::new(0.9, 0.3, 0.2, 1.0),
                    ..Sprite::default()
                },
                1 => Sprite {
                    color: Vec4::new(0.2, 0.5, 0.9, 0.5),
                    ..Sprite::default()
                },
                2 => Sprite {
                    texture: Some(assets.texture),
                    ..Sprite::default()
                },
                _ => assets.sheet.sprite([(i / 4 % 2) as u32, 0], [1, 1]),
            };
            world.spawn((drifter.at(0), drifter, sprite));
        }
        let at = |x: f32, y: f32, z: f32| Transform {
            translation: Vec3::new(x, y, z).as_dvec3(),
            ..Transform::IDENTITY
        };
        let red = MeshDraw::new(assets.quad, assets.opaque, Vec4::new(0.8, 0.1, 0.1, 1.0));
        let parent = world.spawn((at(0.0, 0.0, 1.0), red));
        let glass = MeshDraw::new(assets.quad, assets.blended, Vec4::new(0.1, 0.8, 0.1, 0.5));
        let glass = world.spawn((at(1.5, 0.0, 0.0), glass));
        let sprite = world.spawn((at(-1.5, 0.0, 0.0), assets.sheet.sprite([0, 1], [1, 1])));
        let grandchild = world.spawn((at(0.0, 1.0, 0.0), Sprite::default()));
        // The font bakes as the first frame draws it.
        let label = Text {
            text: "Turning".to_owned(),
            size: 0.4,
            ..Text::default()
        };
        let label = world.spawn((at(0.0, -1.0, 0.0), label));
        let fountain = ParticleEmitter {
            rate: 300.0,
            ..ParticleEmitter::default()
        };
        let fountain = world.spawn((at(0.0, 2.0, 0.0), fountain));
        let sparks = ParticleEmitter {
            rate: 0.0,
            burst: 200,
            blend: ParticleBlend::Additive,
            ..ParticleEmitter::default()
        };
        self.sparks = Some(world.spawn((at(3.0, -3.0, 0.0), sparks)));
        let children = [(glass, parent), (sprite, parent), (grandchild, sprite)];
        let more = [(label, parent), (fountain, parent)];
        for (child, parent) in children.into_iter().chain(more) {
            world.set_parent(child, Some(parent)).expect("both alive");
        }
        self.turning = Some(parent);
        let pivot = world.spawn(Transform::IDENTITY);
        let tilt = Quat::from_rotation_x(-0.25);
        let looking = Transform {
            translation: (tilt * Vec3::new(0.0, 0.0, 16.0)).as_dvec3(),
            rotation: tilt,
            ..Transform::IDENTITY
        };
        let projection = Projection::Perspective {
            fov: 1.0,
            near: 0.1,
        };
        let camera = world.spawn((
            looking,
            Camera {
                projection,
                primary: true,
            },
        ));
        world.set_parent(camera, Some(pivot)).expect("both alive");
        self.pivot = Some(pivot);
        // Every effect on, each in passes of its own: the grade bakes its table each frame, the
        // occlusion needs the atmosphere as a sky, reflections trace the view's depth against the
        // colors each frame keeps for the next, and exposure is metered and read back each frame.
        self.environment = Some(world.spawn(Environment {
            adaptation: Some(Adaptation::default()),
            sky: Sky::Atmosphere(Atmosphere::default()),
            grade: Grade {
                saturation: 1.2,
                ..Grade::IDENTITY
            },
            fog: Some(Fog::default()),
            reflections: Some(Reflections::default()),
            depth_of_field: Some(DepthOfField::default()),
            motion_blur: Some(MotionBlur::default()),
            lens: Lens {
                vignette: 0.5,
                chromatic_aberration: 1.0,
                grain: 0.1,
            },
            ..Environment::default()
        }));
        self.sun = Some(world.spawn((sun_at(0), Light::default())));
        // A mirrored copy culls the other way.
        pair.spawn(world, at(-4.0, 0.0, 2.0));
        let mirrored = Transform {
            scale: Vec3::new(-1.0, 1.0, 1.0),
            ..at(4.0, 0.0, 2.0)
        };
        pair.spawn(world, mirrored);
        // The Fox's units are centimeters.
        let below = Transform {
            scale: Vec3::splat(0.04),
            ..at(-4.0, -3.0, 2.0)
        };
        let fox = fox.spawn(world, below);
        let running = Animator(vec![Play::new("Run")]);
        world.insert(fox, running).expect("just spawned");
        self.assets = Some(assets);
        for slot in 0..SUBTREES {
            self.spawn_subtree(world, slot);
        }
        for i in 0..COPIES {
            self.spawn_copy(world, i as f32);
        }
        // Room for every drifter to wrap at once, so the list never grows at a tick.
        self.wrapped.get_mut().expect("unshared").reserve(DRIFTERS);
        cx.set_colliders_shown(true);
        let click = Binding::Button(Source::Button(MouseButton::Left));
        self.actions.bind(Act::Spin, click).expect("a button");
        let space = Binding::Button(Source::Key(Key::Space));
        self.actions.bind(Act::Jump, space).expect("a button");
    }

    fn tick(&mut self, world: &mut World, tick: Tick) {
        self.commands.clear();
        let found = &self.wrapped;
        world
            .query_mut::<(Entity, &Drifter, &mut Transform)>()
            .par_for_each(|(entity, drifter, transform)| {
                let to = drifter.at(tick.number + 1);
                let moved = (to.translation - transform.translation).truncate().abs();
                if moved.cmpgt((FIELD / 2.0).as_dvec2()).any() {
                    found.lock().expect("no job panics holding it").push(entity);
                }
                *transform = to;
            });
        let wrapped = self.wrapped.get_mut().expect("no job panicked holding it");
        for &entity in wrapped.iter() {
            let reset = world.reset_interpolation(entity);
            reset.expect("a drifter is alive");
        }
        wrapped.clear();
        let turn = TAU * ((tick.number + 1) % PERIOD) as f32 / PERIOD as f32;
        let turning = self.turning.expect("attached");
        let transform = world.get_mut::<Transform>(turning).expect("alive");
        transform.rotation = Quat::from_rotation_z(turn);
        let pivot = self.pivot.expect("attached");
        let transform = world.get_mut::<Transform>(pivot).expect("alive");
        transform.rotation = Quat::from_rotation_y(turn);
        let sun = self.sun.expect("attached");
        *world.get_mut::<Transform>(sun).expect("alive") = sun_at(tick.number + 1);
        if tick.number.is_multiple_of(16) {
            let sparks = self.sparks.expect("attached");
            let emitter = world.get_mut::<ParticleEmitter>(sparks).expect("alive");
            emitter.bursts += 1;
        }
        for i in 0..REPLACED {
            let oldest = self.subtrees.pop_front().expect("a full pool");
            world.despawn(oldest).expect("alive");
            let slot = (tick.number as usize * REPLACED + i) % SUBTREES;
            self.spawn_subtree(world, slot);
        }
        let oldest = self.copies.pop_front().expect("a full pool");
        world.despawn(oldest).expect("alive");
        self.spawn_copy(world, (tick.number % COPIES as u64) as f32);
        if std::mem::take(&mut self.smaa) {
            let environment = self.environment.expect("attached");
            let environment = world.get_mut::<Environment>(environment).expect("alive");
            environment.antialiasing = Antialiasing::Smaa;
        }
        step_animation(world);
    }

    fn update(&mut self, cx: &mut Context, _: Duration) {
        let assets = self.assets();
        let (quad, opaque) = (assets.quad, assets.opaque);
        let cell = assets.sheet.sprite([1, 0], [1, 1]);
        // Away from the view's middle, where these entity-less draws would hide what picks find.
        let below = Mat4::from_translation(Vec3::new(0.0, -5.0, 0.0));
        cx.draw(quad, opaque, below, Vec4::new(0.5, 0.5, 0.5, 1.0));
        let (left, right) = (Vec3::new(-2.0, -5.0, 0.0), Vec3::new(2.0, -5.0, 0.0));
        cx.draw_line(left, right, Vec4::new(0.9, 0.9, 0.2, 1.0), 2.0);
        let above = Mat4::from_translation(Vec3::new(0.0, 5.0, 0.0));
        cx.draw_sprite(above, cell);
        cx.draw_rect(above, Vec4::new(0.2, 0.9, 0.9, 1.0), 1.0);
        let ring = Circle {
            thickness: 0.25,
            ..Circle::default()
        };
        cx.draw_circle(Mat4::from_translation(Vec3::new(1.5, 5.0, 0.0)), ring);
        // Same-length text shaped again each frame, kerned, over two lines.
        self.text.clear();
        let written = write!(self.text, "Frame {:06}\nAVA Æ€ ‘f’ \u{7}漢", self.frames);
        written.expect("a string takes any text");
        let style = TextStyle {
            font: Some(self.assets().font),
            size: 0.5,
            color: Vec4::new(0.9, 0.9, 0.9, 1.0),
            align: [Align::Start, Align::End],
        };
        let corner = Mat4::from_translation(Vec3::new(-6.0, -6.0, 0.0));
        cx.draw_text(corner, &self.text, &style);
        // Away from the view's middle; the cursor is found too, as a menu finds what it's over.
        let mut canvas = cx.canvas([Align::End, Align::Start]);
        let backing = Mat4::from_translation(Vec3::new(-200.0, -80.0, 0.0))
            * Mat4::from_scale(Vec3::new(360.0, 120.0, 1.0));
        let shade = Sprite {
            color: Vec4::new(0.0, 0.0, 0.0, 0.5),
            ..Sprite::default()
        };
        canvas.draw_sprite(backing, shade);
        let on_canvas = TextStyle {
            size: 40.0,
            align: [Align::End, Align::Center],
            ..style
        };
        let inset = Mat4::from_translation(Vec3::new(-40.0, -80.0, 0.0));
        canvas.draw_text(inset, &self.text, &on_canvas);
        let _ = canvas.cursor();
        // Shape and lay out the UI's text again: a title set to what it says, and keys that change
        // nothing (a letter the text field takes no text for, a step down from the last field).
        let ui = self.assets().ui;
        ui.set_text(cx, "frame", &self.text).expect("a text");
        ui.set_text(cx, "title", "CORNER").expect("a text");
        for key in [Key::KeyA, Key::ArrowDown] {
            let pressed = Event::Key {
                key,
                pressed: true,
                repeat: false,
            };
            let _ = ui.event(cx, &pressed);
        }
        ui.draw(cx);
        self.assets().open.draw(cx);
        let turning = self.turning.expect("attached");
        let turning = cx.world().id(turning).expect("alive");
        cx.outline(turning, Vec4::new(1.0, 0.6, 0.1, 1.0), 1.0);
        self.frames += 1;
        let frame = self.frames;
        let asked = |back: u64| {
            let frame = frame.checked_sub(back)?;
            self.asked[frame as usize % ASKED]
        };
        for &(pick, entity) in cx.picked() {
            let inside = asked(4).is_some_and(|[inside, _]| inside == pick);
            let outside = asked(2).is_some_and(|[_, outside]| outside == pick);
            let counted = match (inside, outside, entity) {
                (true, _, Some(_)) => &FOUND,
                (true, _, None) => &MISSED,
                (_, true, None) => &OUTSIDE,
                _ => &ASTRAY,
            };
            counted.fetch_add(1, Ordering::Relaxed);
        }
        // The view's middle as the last frame had it (a headless run's at first), and a point left
        // of the window.
        let middle = cx
            .view()
            .map_or(SIZE.map(|side| f64::from(side) / 2.0), |view| {
                let [x, y] = view.origin.map(f64::from);
                let [width, height] = view.size.map(f64::from);
                [x + width / 2.0, y + height / 2.0]
            });
        let picks = [middle, [-1.0, middle[1]]].map(|at| cx.pick(at).expect("two a frame"));
        self.asked[frame as usize % ASKED] = Some(picks);
        if self.frames == WARM_UP {
            // The attach's assets must be decoded and streamed, so the counted frames draw all.
            let loading = cx.streaming();
            assert!(loading.is_empty(), "still on its way: {loading:?}");
            common::start();
        } else if self.frames == WARM_UP + COUNTED {
            let counted = common::stop();
            ALLOCATED.lock().expect("no thread panicked holding it")[0] = Some(counted);
            self.smaa = true;
        } else if self.frames == WARM_UP + COUNTED + SMAA_WARM_UP {
            common::start();
        } else if self.frames == FRAMES - 1 {
            let counted = common::stop();
            ALLOCATED.lock().expect("no thread panicked holding it")[1] = Some(counted);
        }
    }

    fn event(&mut self, _: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        let Self {
            actions, commands, ..
        } = self;
        actions.event(event, |change| commands.push(change));
        ControlFlow::Continue(())
    }
}

fn write_png(path: &Path, size: [u32; 2], pixel: impl Fn(u32, u32) -> [u8; 4]) {
    let file = fs::File::create(path).expect("the PNG is made");
    let mut encoder = png::Encoder::new(BufWriter::new(file), size[0], size[1]);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let pixels: Vec<u8> = (0..size[1])
        .flat_map(|y| (0..size[0]).map(move |x| (x, y)))
        .flat_map(|(x, y)| pixel(x, y))
        .collect();
    let mut writer = encoder.write_header().expect("the PNG's header is written");
    let written = writer.write_image_data(&pixels);
    written.expect("the PNG's pixels are written");
}

/// Writes the app's assets to `dir`: a checker, a sheet of 2 by 2 cells, and two quad models
/// sharing a buffer and the checker. The pair is an opaque, double-sided, textured quad and a
/// blended one; the plain model is opaque.
fn write_assets(dir: &Path) {
    fs::create_dir_all(dir).expect("the test's directory is made");
    let checker = |x: u32, y: u32| {
        let light = (x / 4 + y / 4).is_multiple_of(2);
        let shade = if light { 230 } else { 40 };
        [shade, shade, shade, 255]
    };
    write_png(&dir.join("checker.png"), [16, 16], checker);
    let cells = |x: u32, y: u32| [(x / 8 * 200) as u8, (y / 8 * 200) as u8, 120, 255];
    write_png(&dir.join("cells.png"), [16, 16], cells);

    let corners = [
        -1.0f32, -1.0, 0.0, 1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0,
    ];
    let uvs = [0.0f32, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0];
    let floats = corners.into_iter().chain(uvs).flat_map(f32::to_le_bytes);
    let indices = [0u16, 1, 2, 0, 2, 3].into_iter().flat_map(u16::to_le_bytes);
    let bin: Vec<u8> = floats.chain(indices).collect();
    fs::write(dir.join("quad.bin"), &bin).expect("the buffer is written");
    let quad = |material: u32| {
        json!({
            "attributes": {"POSITION": 0, "TEXCOORD_0": 1},
            "indices": 2,
            "material": material,
        })
    };
    let model = |primitives: serde_json::Value| {
        json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": primitives}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3",
                 "min": [-1, -1, 0], "max": [1, 1, 0]},
                {"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC2"},
                {"bufferView": 2, "componentType": 5123, "count": 6, "type": "SCALAR"},
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 48},
                {"buffer": 0, "byteOffset": 48, "byteLength": 32},
                {"buffer": 0, "byteOffset": 80, "byteLength": 12},
            ],
            "buffers": [{"uri": "quad.bin", "byteLength": bin.len()}],
            "materials": [
                {"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}, "doubleSided": true},
                {
                    "pbrMetallicRoughness": {"baseColorFactor": [0.2, 0.4, 0.9, 0.5]},
                    "alphaMode": "BLEND",
                },
                {"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}},
            ],
            "textures": [{"source": 0}],
            "images": [{"uri": "checker.png"}],
        })
    };
    let pair = model(json!([quad(0), quad(1)]));
    fs::write(dir.join("pair.gltf"), pair.to_string()).expect("the pair is written");
    let plain = model(json!([quad(2)]));
    fs::write(dir.join("plain.gltf"), plain.to_string()).expect("the plain model is written");
    fs::write(dir.join("corner.ui"), CORNER).expect("the UI is written");
}

/// A UI in the view's bottom left corner, away from its middle, where the picks look. Its list has
/// more buttons than it shows, and its text field is focused, its caret and text drawn.
const CORNER: &str = r#"(
    focus: "typed",
    nodes: [
        Panel(
            anchor: (Start, End),
            margin: (24.0, 24.0),
            padding: (16.0, 16.0),
            color: (0.0, 0.0, 0.0, 0.6),
            radius: 16.0,
            children: [
                Column(
                    gap: 8.0,
                    children: [
                        Text(name: "title", text: "CORNER", size: 32.0),
                        Text(name: "frame", size: 24.0),
                        Row(
                            gap: 8.0,
                            children: [
                                Button(name: "go", text: "GO"),
                                Button(name: "stop", text: "STOP", disabled: true),
                            ],
                        ),
                        Row(
                            gap: 8.0,
                            children: [
                                Slider(name: "volume", value: 0.5, width: Fixed(160.0), height: Fixed(32.0)),
                                Toggle(name: "sound", text: "ON", size: 20.0, on: true),
                                Dropdown(name: "mode", options: ["A", "B"], size: 20.0),
                            ],
                        ),
                        List(
                            name: "lines",
                            width: Fixed(240.0),
                            height: Fixed(60.0),
                            children: [
                                Button(name: "one", text: "ONE", size: 24.0),
                                Button(name: "two", text: "TWO", size: 24.0),
                                Button(name: "three", text: "THREE", size: 24.0),
                            ],
                        ),
                        Field(name: "typed", text: "TYPED", size: 20.0, width: Fixed(240.0)),
                    ],
                ),
            ],
        ),
    ],
)
"#;

fn main() {
    if gg_engine::profile::ENABLED {
        panic!(
            "Tracy's client allocates as it records; run the allocation test without the tracy \
             feature"
        );
    }
    let windowed = std::env::args().any(|arg| arg == "--windowed");
    if windowed && cfg!(feature = "dev") {
        panic!(
            "the watcher of the assets' files, which `dev` adds, allocates as it looks at them: \
             run the windowed check without it, as `cargo test -p gg-engine --test frames -- \
             --windowed`"
        );
    }
    // CI runners without a GPU or Vulkan driver (Windows, macOS) set this. Linux CI runs the test
    // on lavapipe.
    if std::env::var_os("GG_SKIP_GPU_TESTS").is_some_and(|value| !value.is_empty()) {
        println!("the frames test skipped: GG_SKIP_GPU_TESTS is set");
        return;
    }
    // Overlays such as Steam's and OBS's load into every instance as implicit layers and can add
    // validation errors. The loader warns of each layer it disables, a general message that
    // `RUST_LOG` quiets by naming that target alone, so the rest keeps the default filter: an
    // environment's could log, and so allocate, as the frames run (trace level logs every event).
    // SAFETY: no other thread exists yet to read the environment.
    unsafe {
        std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~");
        std::env::set_var("RUST_LOG", "gg_gpu::vulkan::general=error");
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("frames");
    write_assets(&dir);
    let scene = Scene {
        dir,
        ..Scene::default()
    };
    let app = App::new("Frames")
        .seeded(SEED)
        .headless_step(Tick::DT * 3 / 5)
        .layer(scene);
    // Four threads, so the job system has threads of its own on a two-CPU machine too.
    let frames = FRAMES.to_string();
    let size = format!("{}x{}", SIZE[0], SIZE[1]);
    let mut args = vec!["--frames", &frames, "--threads", "4"];
    if !windowed {
        // No tool API: in a development build its runs watch the assets' files on threads that
        // allocate as they look, as docs/PLAN.md's Deferred says.
        args.extend(["--headless", "--no-api", "--size", &size]);
    }
    assert_eq!(
        gg_engine::run_with(app, args),
        ExitCode::SUCCESS,
        "the run failed, as its log says"
    );
    let allocated = std::mem::take(&mut *ALLOCATED.lock().expect("no thread panicked holding it"));
    let how = if windowed { "in a window" } else { "headless" };
    let windows = [
        (COUNTED, "antialiased temporally"),
        (SMAA_COUNTED, "by SMAA"),
    ];
    for (counted, (frames, antialiased)) in allocated.into_iter().zip(windows) {
        let Some(counted) = counted else {
            panic!("the run ended before its {frames} frames {antialiased} were counted");
        };
        if counted.allocations > 0 {
            panic!("{frames} frames {antialiased} allocated {counted}");
        }
        println!("{frames} frames {how}, {antialiased}, allocated nothing");
    }
    // Headless, every frame renders, so each answer comes back when due: all but the last four
    // frames' picks at the middle and the last two's outside. A window's may not (minimized, it
    // doesn't render).
    if !windowed {
        let frames = FRAMES;
        let [found, missed, outside, astray] =
            [&FOUND, &MISSED, &OUTSIDE, &ASTRAY].map(|count| count.load(Ordering::SeqCst));
        assert_eq!(astray, 0, "answers came back in frames they weren't due");
        assert_eq!(found + missed, frames - 4, "picks at the middle answered");
        assert!(found > 0, "no pick at the middle found an entity");
        assert_eq!(outside, frames - 2, "the picks outside the view answered");
        println!("{found} picks of {} found an entity", found + missed);
    }
}
