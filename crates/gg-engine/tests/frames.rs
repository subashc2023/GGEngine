//! Steady-state frames of an app whose layers draw no UI allocate nothing, on any thread: frames
//! of what games draw, sprites flat, textured and of a sprite sheet's cells, games' meshes with
//! parameters, opaque and blended, a glTF model and a mirrored copy of it, a turning hierarchy and
//! drifters that wrap, whose ticks despawn and spawn entities, run through the engine's main loop
//! headless, three fifths of a tick apart, so that they draw between ticks, once its first frames
//! have run.
//!
//! It runs in one process with a main of its own, so that no other test's threads run beside it,
//! and counts every allocation the process makes through Rust's allocator while the frames run,
//! whichever thread makes it; the Vulkan loader, the driver, the validation layer and VMA, beneath
//! the engine, go uncounted. It needs a GPU, as the GPU tests do, and `GG_SKIP_GPU_TESTS` skips
//! it. With `--windowed`, it runs the app in a window instead, for checks by hand of frames that
//! the mouse moves and clicks over: without `dev`, whose shader watcher allocates as it looks at
//! files, as `cargo test -p gg-engine --test frames -- --windowed`.

// For the allocator that counts; see the SAFETY comments.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::backtrace::Backtrace;
use std::cell::Cell;
use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::fs;
use std::io::BufWriter;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use gg_engine::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use gg_engine::{
    Actions, App, Binding, Blend, Camera, Change, Component, Context, Entity, Event, Key, Layer,
    Mesh, MeshDraw, Model, MouseButton, Projection, Shader, Source, Sprite, SpriteSheet, Texture,
    Tick, Transform, World,
};
use serde_json::json;

/// What the world's IDs are drawn from, so that every run draws its entities in one order.
const SEED: u64 = 0x5eed_f2a3;
/// The frames run before counting, as a run's first frames size what the engine and the app keep:
/// three periods of what the ticks move, more than three of the camera's turns, and many turns of
/// every pool.
const WARM_UP: u64 = 320;
/// The frames counted: more than a period of every motion.
const COUNTED: u64 = 160;
/// The ticks a drifter takes to come back to where it started, and the parent to turn round.
const PERIOD: u64 = 64;
/// The frames the camera takes to go round.
const ORBIT: u64 = 96;
const DRIFTERS: usize = 2_000;
/// Where the drifters drift, across and up, around the origin.
const FIELD: Vec2 = Vec2::new(16.0, 9.0);
/// The pool of sprites' parents and children, of which each tick replaces the oldest `REPLACED`,
/// as a game's bullets and particles come and go.
const SUBTREES: usize = 256;
const REPLACED: usize = 16;
/// The pool of copies of a model, of which each tick replaces the oldest.
const COPIES: usize = 8;

/// System's allocator, which counts the allocations made while `COUNTING` is set, and keeps where
/// the first was made.
struct Counting;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
/// The backtrace of the first allocation counted, whose frames say which thread made it.
static FIRST: Mutex<Option<Backtrace>> = Mutex::new(None);
/// Set once the app has run all of its counted frames.
static COUNTED_ALL: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Set while the thread counts an allocation, since capturing a backtrace allocates too.
    static IN_COUNT: Cell<bool> = const { Cell::new(false) };
}

fn count() {
    if !COUNTING.load(Ordering::Relaxed) || IN_COUNT.with(|in_count| in_count.replace(true)) {
        return;
    }
    // Neither while panicking, since the panic hook captures a backtrace too, nor through
    // `thread::current`, which std aborts for as it runs inside itself, as it would if its own
    // first allocation came here: the backtrace's frames say which thread.
    if ALLOCATIONS.fetch_add(1, Ordering::Relaxed) == 0 && !thread::panicking() {
        let first = Backtrace::force_capture();
        if let Ok(mut kept) = FIRST.lock() {
            kept.get_or_insert(first);
        }
    }
    IN_COUNT.with(|in_count| in_count.set(false));
}

// SAFETY: each method passes what it's given to System's, so it keeps System's contract; counting
// touches atomics and a thread-local flag, and allocates only through this allocator again, which
// the flag keeps from counting.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: the caller keeps `alloc`'s contract, which is System's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: the caller keeps `alloc_zeroed`'s contract, which is System's.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count();
        // SAFETY: the caller keeps `realloc`'s contract: `ptr` is System's, from `layout`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller keeps `dealloc`'s contract: `ptr` is System's, from `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

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
}

/// The app: what it spawns as it attaches, and what its ticks move, spawn and despawn.
#[derive(Default)]
struct Scene {
    dir: PathBuf,
    assets: Option<Assets>,
    turning: Option<Entity>,
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
}

impl Scene {
    fn assets(&self) -> &Assets {
        self.assets.as_ref().expect("attached")
    }

    /// Spawns a sprite whose child is another, at `x`, the newest of the pool.
    fn spawn_subtree(&mut self, world: &mut World, x: f32) {
        let assets = self.assets.as_ref().expect("attached");
        let at = Transform {
            translation: Vec3::new(x, -3.0, 0.5),
            scale: Vec3::splat(0.3),
            ..Transform::IDENTITY
        };
        let root = world.spawn((at, assets.sheet.sprite([1, 1], [1, 1])));
        let below = Transform {
            translation: Vec3::new(0.0, -1.0, 0.0),
            ..Transform::IDENTITY
        };
        let textured = Sprite {
            texture: assets.texture,
            ..Sprite::default()
        };
        let child = world.spawn((below, textured));
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
        let assets = Assets {
            quad: quad.expect("the quad is whole triangles"),
            opaque: opaque.expect("the shader compiles"),
            blended: blended.expect("the shader compiles"),
            texture: texture.expect("the checker loads"),
            sheet: sheet.expect("the cells load"),
            plain: plain.expect("the plain model loads"),
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
                    texture: assets.texture,
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
        for (child, parent) in [(glass, parent), (sprite, parent), (grandchild, sprite)] {
            world.set_parent(child, Some(parent)).expect("both alive");
        }
        self.turning = Some(parent);
        // The pair, and a copy mirrored, which culls the other way.
        pair.spawn(world, at(-4.0, 0.0, 2.0));
        let mirrored = Transform {
            scale: Vec3::new(-1.0, 1.0, 1.0),
            ..at(4.0, 0.0, 2.0)
        };
        pair.spawn(world, mirrored);
        self.assets = Some(assets);
        for i in 0..SUBTREES {
            self.spawn_subtree(world, i as f32 * 0.05);
        }
        for i in 0..COPIES {
            self.spawn_copy(world, i as f32);
        }
        // Room for every drifter to wrap at once, so that the list never grows at a tick.
        self.wrapped.get_mut().expect("unshared").reserve(DRIFTERS);
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
        for i in 0..REPLACED {
            let oldest = self.subtrees.pop_front().expect("a full pool");
            world.despawn(oldest).expect("alive");
            let x = ((tick.number as usize * REPLACED + i) % SUBTREES) as f32 * 0.05;
            self.spawn_subtree(world, x);
        }
        let oldest = self.copies.pop_front().expect("a full pool");
        world.despawn(oldest).expect("alive");
        self.spawn_copy(world, (tick.number % COPIES as u64) as f32);
    }

    fn update(&mut self, cx: &mut Context, _: Duration) {
        let turn = TAU * (self.frames % ORBIT) as f32 / ORBIT as f32;
        let rotation = Quat::from_rotation_y(turn) * Quat::from_rotation_x(-0.25);
        cx.set_camera(Camera {
            position: rotation * Vec3::new(0.0, 0.0, 16.0),
            rotation,
            projection: Projection::Perspective {
                fov: 1.0,
                near: 0.1,
            },
        });
        let assets = self.assets();
        let (quad, opaque) = (assets.quad, assets.opaque);
        let cell = assets.sheet.sprite([1, 0], [1, 1]);
        let below = Mat4::from_translation(Vec3::new(0.0, -5.0, 0.0));
        cx.draw(quad, opaque, below, Vec4::new(0.5, 0.5, 0.5, 1.0));
        cx.draw_sprite(Mat4::from_translation(Vec3::new(0.0, 5.0, 0.0)), cell);
        self.frames += 1;
        if self.frames == WARM_UP {
            COUNTING.store(true, Ordering::SeqCst);
        } else if self.frames == WARM_UP + COUNTED {
            COUNTING.store(false, Ordering::SeqCst);
            COUNTED_ALL.store(true, Ordering::SeqCst);
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
}

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
            "the shader watcher, which `dev` adds, allocates as it looks at files: run the \
             windowed check without it, as `cargo test -p gg-engine --test frames -- --windowed`"
        );
    }
    // CI runners without a GPU or a Vulkan driver, Windows' and macOS's, set this. Linux CI runs
    // the test on lavapipe instead.
    if std::env::var_os("GG_SKIP_GPU_TESTS").is_some_and(|value| !value.is_empty()) {
        println!("the frames test skipped: GG_SKIP_GPU_TESTS is set");
        return;
    }
    // Overlays such as Steam's and OBS's load into every instance as implicit layers, and can
    // add validation errors of their own; and a `RUST_LOG` of the environment's could log, and
    // so allocate, as the frames run, as one at trace level would for every event.
    // SAFETY: no other thread exists yet to read the environment.
    unsafe {
        std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~");
        std::env::remove_var("RUST_LOG");
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
        args.push("--headless");
    }
    assert_eq!(
        gg_engine::run_with(app, args),
        ExitCode::SUCCESS,
        "the run failed, as its log says"
    );
    assert!(
        COUNTED_ALL.load(Ordering::SeqCst),
        "the run ended before its {COUNTED} counted frames"
    );
    let counted = ALLOCATIONS.load(Ordering::SeqCst);
    if counted > 0 {
        let first = FIRST.lock().expect("no thread panicked holding it").take();
        let first = first.map_or_else(|| "none kept".to_owned(), |trace| trace.to_string());
        panic!("{COUNTED} frames allocated {counted} times; the first, from\n{first}");
    }
    let how = if windowed { "in a window" } else { "headless" };
    println!("{COUNTED} frames {how} allocated nothing");
}
