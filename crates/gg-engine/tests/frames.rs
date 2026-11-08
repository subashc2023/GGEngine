//! Steady-state frames of an app whose layers draw no development UI, egui's, which allocates,
//! allocate nothing, on any thread: frames
//! of what games draw, sprites flat, textured and of a sprite sheet's cells, circles filled and
//! rings, games' meshes with parameters, opaque and blended, a glTF model and a mirrored copy of
//! it, a turning hierarchy and drifters that wrap, whose ticks despawn and spawn entities, with
//! colliders, whose outlines draw over the rest, as a layer's own lines and square's outline do,
//! text, written and shaped again each frame, a sprite and that text on the view's canvas, a UI
//! from its file, laid out again each frame as code changes its text, drawn on the canvas and fed
//! input that changes nothing,
//! emitters' particles, of one the turning parent carries round and one its ticks burst, and an
//! outline round the turning parent, as the editor outlines what's picked, seen through a
//! camera of the world's that a pivot the ticks turn carries round, under the default environment,
//! which antialiases the view against the frames before, run through the engine's main
//! loop headless, without the tool API, whose development runs' watchers of the assets' files
//! allocate as they look, three fifths of a tick apart, so that they draw between ticks, once its
//! first frames have run, by which every image its attach added has decoded off the main thread
//! and everything has streamed to the GPU. Every frame picks what the view shows at its middle, as the frame before had
//! it, and outside it, from the first on, and headless, each answer comes back in the frame it's
//! due: four frames after the one that asked, and two for the pick outside the view, which finds
//! nothing.
//!
//! It runs in one process with a main of its own, so that no other test's threads run beside it,
//! and counts every allocation the process makes through Rust's allocator while the frames run,
//! whichever thread makes it; the Vulkan loader, the driver, the validation layer and VMA, beneath
//! the engine, go uncounted. It needs a GPU, as the GPU tests do, and `GG_SKIP_GPU_TESTS` skips
//! it. With `--windowed`, it runs the app in a window instead, for checks by hand of frames that
//! the mouse moves and clicks over: without `dev`, whose watcher of the assets' files allocates as
//! it looks at them, as `cargo test -p gg-engine --test frames -- --windowed`, and with no gamepad
//! connected, since gilrs allocates as it reads one.

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

use gg_engine::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use gg_engine::{
    Actions, Align, App, Binding, Blend, BoxCollider2D, Camera, Change, Circle, CircleCollider2D,
    Component, Context, Entity, Environment, Event, Font, GameUi, Key, Layer, Mesh, MeshDraw,
    Model, MouseButton, ParticleBlend, ParticleEmitter, Pick, Projection, Shader, Source, Sprite,
    SpriteSheet, Text, TextStyle, Texture, Tick, Transform, World,
};
use serde_json::json;

use crate::common::Counted;

/// What the world's IDs are drawn from, so that every run draws its entities in one order.
const SEED: u64 = 0x5eed_f2a3;
/// The frames run before counting, as a run's first frames size what the engine and the app keep:
/// three periods of what the ticks move, and many turns of every pool.
const WARM_UP: u64 = 320;
/// The frames counted: more than a period of every motion.
const COUNTED: u64 = 160;
/// The ticks a drifter takes to come back to where it started, the parent to turn round, and the
/// camera to go round.
const PERIOD: u64 = 64;
const DRIFTERS: usize = 2_000;
/// Where the drifters drift, across and up, around the origin.
const FIELD: Vec2 = Vec2::new(16.0, 9.0);
/// The pool of sprites' parents, with colliders, and their children, sprites, or circles with
/// colliders of their own, of which each tick replaces the oldest `REPLACED`, as a game's bullets
/// and particles come and go.
const SUBTREES: usize = 256;
const REPLACED: usize = 16;
/// The pool of copies of a model, of which each tick replaces the oldest.
const COPIES: usize = 8;
/// The frames whose picks the app keeps, more than an answer takes to come back.
const ASKED: usize = 8;

/// The answers that came back: to the picks at the view's middle, those that found an entity and
/// those that found none; to those outside it, which find none; and those that came back in
/// another frame than they were due, or found an entity outside the view.
static FOUND: AtomicU64 = AtomicU64::new(0);
static MISSED: AtomicU64 = AtomicU64::new(0);
static OUTSIDE: AtomicU64 = AtomicU64::new(0);
static ASTRAY: AtomicU64 = AtomicU64::new(0);

/// What the counted frames allocated, once the app has run them all.
static ALLOCATED: Mutex<Option<Counted>> = Mutex::new(None);

/// A drifter: where it starts, and how far it goes a tick, a whole number of fields across and up
/// in `PERIOD` ticks, so that it's back where it started each period; and its z.
#[derive(Clone, Component)]
struct Drifter {
    start: Vec2,
    velocity: Vec2,
    z: f32,
}

impl Drifter {
    /// Drifter `i`, which starts somewhere in the field and drifts one way of sixteen.
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

    /// Where it is at tick `tick`.
    fn at(&self, tick: u64) -> Transform {
        let moved = self.start + self.velocity * (tick % PERIOD) as f32;
        let at = moved.rem_euclid(FIELD) - FIELD / 2.0;
        Transform {
            translation: at.extend(self.z),
            scale: Vec3::splat(0.2),
            ..Transform::IDENTITY
        }
    }
}

/// What the app's input does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Spin,
    Jump,
}

/// What the app draws with, which its attach adds.
struct Assets {
    quad: Mesh<[f32; 3]>,
    opaque: Shader<[f32; 3], Vec4>,
    blended: Shader<[f32; 3], Vec4>,
    texture: Texture,
    sheet: SpriteSheet,
    /// Opaque, so that its copies draw in runs that the order of their IDs doesn't split.
    plain: Model,
    /// The engine's own font, Inter.
    font: Font,
    /// A panel of a text and two buttons, the second disabled, in the view's bottom left corner.
    ui: GameUi,
}

/// The app: what it spawns as it attaches, and what its ticks move, spawn and despawn.
#[derive(Default)]
struct Scene {
    dir: PathBuf,
    assets: Option<Assets>,
    turning: Option<Entity>,
    /// What carries the camera round.
    pivot: Option<Entity>,
    /// The emitter the ticks burst.
    sparks: Option<Entity>,
    subtrees: VecDeque<Entity>,
    copies: VecDeque<Entity>,
    /// The drifters that wrapped at a tick, kept for its memory, with its lock: empty between
    /// ticks.
    wrapped: Mutex<Vec<Entity>>,
    actions: Actions<Act>,
    /// What the actions did since the last tick, as a game's commands: plain data, which the
    /// tick applies, here by forgetting.
    commands: Vec<Change<Act>>,
    frames: u64,
    /// The tickets of the picks of each of the last frames, by its number modulo `ASKED`: at the
    /// view's middle, then outside it.
    asked: [Option<[Pick; 2]>; ASKED],
    /// The text each frame draws, written again in its memory.
    text: String,
}

impl Scene {
    fn assets(&self) -> &Assets {
        self.assets.as_ref().expect("attached")
    }

    /// Spawns the subtree of slot `slot` of the pool, the newest: a sprite with a box collider,
    /// whose child is another sprite, or in every eighth slot a circle with a circle collider, a
    /// disc and a ring by turns.
    fn spawn_subtree(&mut self, world: &mut World, slot: usize) {
        let assets = self.assets.as_ref().expect("attached");
        let at = Transform {
            translation: Vec3::new(slot as f32 * 0.05, -3.0, 0.5),
            scale: Vec3::splat(0.3),
            ..Transform::IDENTITY
        };
        let sprite = assets.sheet.sprite([1, 1], [1, 1]);
        let root = world.spawn((at, sprite, BoxCollider2D::default()));
        let below = Transform {
            translation: Vec3::new(0.0, -1.0, 0.0),
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

    /// Spawns a copy of the plain model at `x`, the newest of the pool.
    fn spawn_copy(&mut self, world: &mut World, x: f32) {
        let at = Transform {
            translation: Vec3::new(x, 3.0, -1.0),
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
        let inter =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/Inter/Inter-Regular.ttf");
        let font = cx.add_font("inter", inter);
        let ui = cx.add_ui("corner", self.dir.join("corner.ui"));
        let assets = Assets {
            quad: quad.expect("the quad is whole triangles"),
            opaque: opaque.expect("the shader compiles"),
            blended: blended.expect("the shader compiles"),
            texture: texture.expect("the checker loads"),
            sheet: sheet.expect("the cells load"),
            plain: plain.expect("the plain model loads"),
            font: font.expect("Inter bakes"),
            ui: ui.expect("the UI reads"),
        };
        let pair = pair.expect("the pair loads");

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
        // A parent that turns, carrying a child of each kind, and a grandchild.
        let at = |x: f32, y: f32, z: f32| Transform {
            translation: Vec3::new(x, y, z),
            ..Transform::IDENTITY
        };
        let red = MeshDraw::new(assets.quad, assets.opaque, Vec4::new(0.8, 0.1, 0.1, 1.0));
        let parent = world.spawn((at(0.0, 0.0, 1.0), red));
        let glass = MeshDraw::new(assets.quad, assets.blended, Vec4::new(0.1, 0.8, 0.1, 0.5));
        let glass = world.spawn((at(1.5, 0.0, 0.0), glass));
        let sprite = world.spawn((at(-1.5, 0.0, 0.0), assets.sheet.sprite([0, 1], [1, 1])));
        let grandchild = world.spawn((at(0.0, 1.0, 0.0), Sprite::default()));
        // Text in the engine's own font, which bakes as the first frame draws it.
        let label = Text {
            text: "Turning".to_owned(),
            size: 0.4,
            ..Text::default()
        };
        let label = world.spawn((at(0.0, -1.0, 0.0), label));
        // Particles from a fountain the parent carries round, which leaves a trail, and sparks
        // that the ticks burst.
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
        // The camera, looking down a little at the origin from 16 away, which a pivot there
        // carries round, as the ticks turn it.
        let pivot = world.spawn(Transform::IDENTITY);
        let tilt = Quat::from_rotation_x(-0.25);
        let looking = Transform {
            translation: tilt * Vec3::new(0.0, 0.0, 16.0),
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
        // The default environment's, which antialiases the view, resolving it against the frames
        // before.
        world.spawn(Environment::default());
        // The pair, and a copy mirrored, which culls the other way.
        pair.spawn(world, at(-4.0, 0.0, 2.0));
        let mirrored = Transform {
            scale: Vec3::new(-1.0, 1.0, 1.0),
            ..at(4.0, 0.0, 2.0)
        };
        pair.spawn(world, mirrored);
        self.assets = Some(assets);
        for slot in 0..SUBTREES {
            self.spawn_subtree(world, slot);
        }
        for i in 0..COPIES {
            self.spawn_copy(world, i as f32);
        }
        // Room for every drifter to wrap at once, so that the list never grows at a tick.
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
                if moved.cmpgt(FIELD / 2.0).any() {
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
    }

    fn update(&mut self, cx: &mut Context, _: Duration) {
        let assets = self.assets();
        let (quad, opaque) = (assets.quad, assets.opaque);
        let cell = assets.sheet.sprite([1, 0], [1, 1]);
        // Away from the view's middle, where these, of no entity, would hide what the picks find.
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
        // Text shaped again each frame, of the same length, and kerned, over two lines.
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
        // On the canvas, from the view's top right, away from its middle, where the cursor is
        // found too, as a menu finds what it's over.
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
        // The UI's text shaped and laid out again, its title set to what it says already, and keys
        // it takes nothing from: a letter, and a step down from its last button.
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
        let turning = self.turning.expect("attached");
        let turning = cx.world().id(turning).expect("alive");
        cx.outline(turning, Vec4::new(1.0, 0.6, 0.1, 1.0), 1.0);
        self.frames += 1;
        let frame = self.frames;
        // The picks of the frame `back` before this one.
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
        // The view's middle, where the last frame had it, or a headless run's at first; and left of
        // the window.
        let middle = cx.view().map_or([640.0, 360.0], |view| {
            let [x, y] = view.origin.map(f64::from);
            let [width, height] = view.size.map(f64::from);
            [x + width / 2.0, y + height / 2.0]
        });
        let picks = [middle, [-1.0, middle[1]]].map(|at| cx.pick(at).expect("two a frame"));
        self.asked[frame as usize % ASKED] = Some(picks);
        if self.frames == WARM_UP {
            // What the attach added decoded and streamed to the GPU long before, so that the
            // frames counted draw it all.
            let loading = cx.streaming();
            assert!(loading.is_empty(), "still on its way: {loading:?}");
            common::start();
        } else if self.frames == WARM_UP + COUNTED {
            let counted = common::stop();
            *ALLOCATED.lock().expect("no thread panicked holding it") = Some(counted);
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

/// Writes a PNG of `size` pixels, each `pixel` of its column and row, to `path`.
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

/// Writes what the app loads to `dir`: a checker texture, a sprite sheet of 2 by 2 cells, and two
/// models of quads, which share a buffer and the checker: the pair, one opaque, double-sided and
/// textured, one blended; and the plain one, opaque.
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

    // A quad's corners, the corners' texture coordinates, and its two triangles.
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

/// The UI in the view's bottom left corner, away from its middle, where the picks look: a rounded
/// panel of a title, a text and two buttons, the first focused and the second disabled.
const CORNER: &str = r#"(
    focus: "go",
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
    // CI runners without a GPU or a Vulkan driver, Windows' and macOS's, set this. Linux CI runs
    // the test on lavapipe instead.
    if std::env::var_os("GG_SKIP_GPU_TESTS").is_some_and(|value| !value.is_empty()) {
        println!("the frames test skipped: GG_SKIP_GPU_TESTS is set");
        return;
    }
    // Overlays such as Steam's and OBS's load into every instance as implicit layers, and can
    // add validation errors of their own. The loader warns of each layer it disables as each
    // instance starts, a general message, which `RUST_LOG` quiets by naming that target alone, so
    // that the rest keeps the default filter: one of the environment's could log, and so allocate,
    // as the frames run, as one at trace level would for every event.
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
    // Four threads, so that the job system has threads of its own on a machine of two CPUs too.
    let frames = (WARM_UP + COUNTED + 1).to_string();
    let mut args = vec!["--frames", &frames, "--threads", "4"];
    if !windowed {
        // Without the tool API, whose runs in a development build have threads watch the assets'
        // files, which allocate as they look at them, as docs/PLAN.md's Deferred says.
        args.extend(["--headless", "--no-api"]);
    }
    assert_eq!(
        gg_engine::run_with(app, args),
        ExitCode::SUCCESS,
        "the run failed, as its log says"
    );
    let Some(counted) = ALLOCATED
        .lock()
        .expect("no thread panicked holding it")
        .take()
    else {
        panic!("the run ended before its {COUNTED} counted frames");
    };
    if counted.allocations > 0 {
        panic!("{COUNTED} frames allocated {counted}");
    }
    let how = if windowed { "in a window" } else { "headless" };
    println!("{COUNTED} frames {how} allocated nothing");
    // Headless, every frame renders, so each answer comes back in the frame it's due: all but
    // those of the last four frames' picks at the middle, and of the last two's outside the view.
    // A window's frames may not, as one that's minimized doesn't.
    if !windowed {
        let frames = WARM_UP + COUNTED + 1;
        let [found, missed, outside, astray] =
            [&FOUND, &MISSED, &OUTSIDE, &ASTRAY].map(|count| count.load(Ordering::SeqCst));
        assert_eq!(astray, 0, "answers came back in frames they weren't due");
        assert_eq!(found + missed, frames - 4, "picks at the middle answered");
        assert!(found > 0, "no pick at the middle found an entity");
        assert_eq!(outside, frames - 2, "the picks outside the view answered");
        println!("{found} picks of {} found an entity", found + missed);
    }
}
