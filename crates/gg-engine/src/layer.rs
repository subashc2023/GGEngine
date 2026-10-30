//! The layer stack: how an app's code, and the engine's tools, plug into the main loop.

use std::any::type_name;
use std::mem;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{
    Blend, Circle, Draws, Line, MAX_PICKS, Mesh, Meshes, Mipmaps, ModelParams, ModelVertex,
    Outline, Shader, Shaders, Sprite, SpriteSheet, Texture, Textures, Viewpoint, Viewport,
};
#[cfg(feature = "dev")]
use subsecond::HotFn;
use tracing::info;

use crate::assets::{self, Assets};
use crate::camera::{self, Camera, Tie};
use crate::draw::{self, Poses, Scratch};
use crate::gltf;
use crate::image::{self, Image};
use crate::interpolation::Past;
use crate::perf::Perf;
use crate::physics::{self, CircleCollider2D, Physics2D};
use crate::recording::Recorder;
use crate::reflect::{Reflect, Value};
use crate::render_thread::Extracted;
use crate::snapshot::Snapshot;
use crate::{
    App, BoxCollider2D, EntityId, Event, Input, MeshDraw, Model, Name, RigidBody2D, Transform, Ui,
    World,
};

/// A part of an app that the main loop drives. Every method has a default that does nothing,
/// so a layer implements only what it uses; [`ui`](Self::ui)'s also tells the engine that the
/// layer draws no UI.
///
/// The world changes only as the layers attach and detach, and at their ticks: the rest of a
/// layer reads it, and queues commands, data of its own, for its next tick to apply, so that the
/// simulation goes on at its own rate whatever the frame rate. A tick changes the world only
/// through its commands and its systems, so that a recording of the commands replays the run.
///
/// As the app starts, every layer loads, from the bottom up, then every layer attaches, building
/// the world; or, in a run restored from a snapshot (`--restore`), whose world comes from the
/// snapshot, picks up from the world in its place, so that a game that restarts, after a crash or
/// a rebuild, goes on where it was.
///
/// In a development build, the engine calls each method through subsecond's `HotFn`, so that a
/// patch of the game's code, which `gg run` builds as the game's files change and the engine
/// applies between ticks, reaches the method and what it calls directly, such as the systems and
/// scripts its tick runs; but not what it calls through a trait object or a function pointer of
/// the game's own, which waits for the next rebuild.
pub trait Layer {
    /// Called once as the app starts, before any layer attaches, and in a run restored from a
    /// snapshot too: it makes what the layer draws with, such as its textures, meshes and shaders,
    /// and registers the component types its world holds, which a restored run needs as much as a
    /// fresh one, but spawns nothing, since a restored run's world comes from the snapshot. A layer
    /// that spawns here panics.
    fn load(&mut self, _cx: &mut Context) {}

    /// Called once as the app starts, after every layer has loaded, before the first tick and
    /// update: it builds the world the simulation starts from. A run restored from a snapshot calls
    /// [`restored`](Self::restored) in its place.
    fn attach(&mut self, _cx: &mut Context) {}

    /// Called once as a run restored from a snapshot starts, in place of [`attach`](Self::attach),
    /// once every layer has loaded and the world is back as it was: the layer picks up from the
    /// world, such as the values its panels show, and queues what its ticks need, such as letting
    /// go of what was held as the snapshot was taken. The world changes only at ticks from here.
    fn restored(&mut self, _cx: &mut Context) {}

    /// The world a snapshot of the run saves in place of the context's while the layer holds aside
    /// the world the simulation goes back to, as the editor holds the world as it was edited while
    /// a copy of it plays, so that a restart comes back to that world; `None`, the default,
    /// otherwise.
    fn saved_world(&self) -> Option<&World> {
        None
    }

    /// Called once as the app ends, after the last tick.
    fn detach(&mut self, _cx: &mut Context) {}

    /// Called at each tick of the simulation, [64 a second](Tick::DT) whatever the frame rate,
    /// before the frame's updates: the layer applies the commands it queued since its last tick,
    /// in the order it queued them, then runs its systems, in the order it calls them. It gets the
    /// world alone, so that a tick goes by the world, the tick, its commands and what the layer set
    /// up as it attached, never by the frame's time, input or drawing.
    fn tick(&mut self, _world: &mut World, _tick: Tick) {}

    /// The commands the layer queued since its last tick, which its tick applies: what a recording
    /// of the run keeps of the layer, which the engine reads just before each tick, and what a
    /// replay gives it again, in place of what it queued itself. `None`, the default, for a layer
    /// that queues none. A layer whose tick goes by anything but the world, the tick, its commands
    /// and what it set up as it attached, such as a frame's time or input, or what its UI keeps,
    /// replays another way.
    fn commands(&mut self) -> Option<&mut dyn Commands> {
        None
    }

    /// Called every frame, after the frame's ticks, with the time since the last frame: zero for
    /// the first, and for the first after frames pause, as while the window is minimized or a
    /// file dialog is open, so that the pause doesn't count as one long frame. For what's
    /// presented, such as the viewpoint the view sees from, rather than the world, which changes
    /// at ticks.
    fn update(&mut self, _cx: &mut Context, _dt: Duration) {}

    /// Called for each event that the layers above didn't handle. `Break` handles it, so the
    /// layers below don't see it; but a release of a key or a button, a gamepad's stick or trigger
    /// back at rest, and the loss of focus, reach every layer, so that none is left holding what it
    /// saw pressed. A stick or a trigger that a layer handles away from rest stays, for the layers
    /// below, as they last saw it until it rests.
    fn event(&mut self, _cx: &mut Context, _event: &Event<'_>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    /// Called every frame, after the updates, to draw UI: dockable panels, and anything else egui
    /// draws. A layer that keeps this default draws none, and an app none of whose layers draw UI,
    /// whose windows the OS frames, and which has no [view panel](crate::App::view_panel), runs
    /// without egui from its second frame, so that its frames neither collect egui's input nor lay
    /// out and tessellate an empty UI.
    fn ui(&mut self, _cx: &mut Context, ui: &mut Ui<'_>) {
        ui.default_ui(type_name::<Self>());
    }
}

/// A layer's queue of commands, without their type, as [`Layer::commands`] gives it to the engine,
/// which reads it at each tick to record it, and fills it to replay it: each command as a
/// reflection [`Value`]. A `Vec` of a type that's [`Reflect`], such as an enum that derives it, is
/// one.
pub trait Commands {
    /// How many are queued.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Command `i`, the first queued first, as a value.
    fn value(&self, i: usize) -> Value;

    /// Queues the command `value` makes, after those queued.
    ///
    /// Fails as [`Reflect::from_value`] does if it makes none.
    fn push(&mut self, value: &Value) -> Result<()>;

    /// Checks that [`push`](Self::push) would queue the command `value` makes, without queuing it:
    /// a recording checks each command it records, so that it holds none that its replay would
    /// refuse. The default checks nothing, for a queue whose `push` takes every value its
    /// [`value`](Self::value) gives, as the Inspector's does, whose sets keep what's opaque.
    ///
    /// Fails as `push` would.
    fn check(&self, value: &Value) -> Result<()> {
        let _ = value;
        Ok(())
    }

    /// Empties the queue.
    fn clear(&mut self);
}

impl<C: Reflect> Commands for Vec<C> {
    fn len(&self) -> usize {
        Vec::len(self)
    }

    fn value(&self, i: usize) -> Value {
        self[i].to_value()
    }

    fn push(&mut self, value: &Value) -> Result<()> {
        Vec::push(self, C::from_value(value)?);
        Ok(())
    }

    /// Makes the command, as `push` does: it fails, saying where, for a command that holds
    /// something opaque, which no value makes.
    fn check(&self, value: &Value) -> Result<()> {
        C::from_value(value).map(drop)
    }

    fn clear(&mut self) {
        Vec::clear(self);
    }
}

/// A step of the simulation, which runs 64 of them a second whatever the frame rate: what a
/// layer's [`tick`](Layer::tick) gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tick {
    /// The ticks before it.
    pub number: u64,
}

impl Tick {
    /// How long a tick simulates: a 64th of a second, exact in `f32`, `f64` and nanoseconds.
    pub const DT: Duration = Duration::from_nanos(15_625_000);

    /// The simulation's time once this tick has run: `DT` for the first.
    pub fn time(self) -> Duration {
        Duration::from_nanos(Self::DT.as_nanos() as u64 * (self.number + 1))
    }
}

/// A ticket for a pick, which [`Context::pick`] gives, and its answer in [`Context::picked`] comes
/// back with. Each is the next of a count, so a later pick's is greater.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pick(u64);

/// What layers can see and change of the running app.
pub struct Context {
    window: Option<Window>,
    input: Input,
    clear_color: [f32; 4],
    /// What a layer set the view to see from this frame, in place of the world's camera.
    viewpoint: Option<Viewpoint>,
    /// Where the world's camera saw from as the world was last drawn, if it has one.
    seen: Option<Viewpoint>,
    /// The tie among the world's cameras last warned of, while they've tied since.
    tie: Option<Tie>,
    meshes: Meshes,
    shaders: Shaders,
    textures: Textures,
    /// What to draw this frame.
    draws: Draws,
    /// What to draw this frame over the rest, after the world: lines.
    over: Draws,
    /// Whether the world's colliders' outlines draw, over the rest.
    colliders: bool,
    /// What to outline in the view this frame, if anything.
    outline: Option<Outline>,
    /// Where the game's view was as the last frame handed it off, while it had an area.
    view: Option<Viewport>,
    /// The number of the next pick's ticket.
    tickets: u64,
    /// The picks asked this frame, at most [`MAX_PICKS`], each with where, in the main window's
    /// pixels.
    picks: Vec<(Pick, [f64; 2])>,
    /// The picks the last frame handed off asked outside the view, which no frame reads: answered
    /// `None` with the answers that come back next.
    unseen: Vec<Pick>,
    /// The answers that came back with the frame last taken back.
    picked: Vec<(Pick, Option<EntityId>)>,
    world: World,
    /// What drawing the world sorts and computes, kept between frames.
    scratch: Scratch,
    /// Where the world's entities were as the latest tick started, which drawing draws them on
    /// from.
    past: Past,
    /// Set from when the layers have attached until they detach: the world then changes only at
    /// ticks.
    attached: bool,
    /// What the files of the assets the layers add are named from, as [`App::root`] says.
    ///
    /// [`App::root`]: crate::App::root
    pub(crate) root: Option<PathBuf>,
    /// The assets the layers added: the world's, which a world that a layer puts in its place
    /// takes again, as [`share_assets`](Self::share_assets) says.
    assets: Arc<Assets>,
    /// How long frames take, which the main loop measures and
    /// [`PerfPanel`](crate::PerfPanel) shows.
    pub(crate) perf: Perf,
    /// Set once a layer has shown something that paused the frames while it was open, such as a
    /// file dialog, until the main loop takes it to restart its clock, so that no frame's time
    /// counts the pause.
    pub(crate) paused: bool,
}

impl Context {
    pub(crate) fn new(window: Option<Window>) -> Self {
        Self::with_world(window, World::default())
    }

    /// A context for `app`'s run, in `window`, if it has one, whose world draws its entities' IDs
    /// from the app's seed, or from a seed drawn at random without one.
    pub(crate) fn of_run(window: Option<Window>, app: &App) -> Self {
        let world = app.seed.map_or_else(World::default, World::seeded);
        let mut cx = Self::with_world(window, world);
        cx.root.clone_from(&app.root);
        cx
    }

    /// A context without a window, a renderer or a stack, whose world draws its entities' IDs from
    /// `seed`: for calling a layer's [`attach`](Layer::attach) and its [`tick`](Layer::tick)s
    /// directly, as a test does, so that its simulation goes the same way every run, on every
    /// machine.
    #[doc(hidden)]
    pub fn seeded(seed: u64) -> Self {
        Self::with_world(None, World::seeded(seed))
    }

    /// Names the assets the layers add from files by their paths relative to `root`, as
    /// [`App::root`](crate::App::root) does for a run: for a [`seeded`](Self::seeded) context, so
    /// that the world's hash, which reads its textures' names, doesn't depend on where the game's
    /// files are.
    #[doc(hidden)]
    #[must_use]
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    fn with_world(window: Option<Window>, mut world: World) -> Self {
        // The engine's names first, so that a game's types can't take them, listed before any
        // entity has one.
        let registered = world.register::<Transform>().and_then(|()| {
            world.register::<Sprite>()?;
            world.register::<MeshDraw>()?;
            world.register::<Camera>()?;
            world.register::<Name>()?;
            world.register::<RigidBody2D>()?;
            world.register::<BoxCollider2D>()?;
            world.register::<Circle>()?;
            world.register::<CircleCollider2D>()?;
            world.register_saved::<Physics2D>()
        });
        registered.expect("the engine's components describe themselves");
        let assets = Arc::default();
        world.share_assets(&assets);
        Self {
            window,
            input: Input::default(),
            clear_color: [0.02, 0.02, 0.02, 1.0],
            viewpoint: None,
            seen: None,
            tie: None,
            meshes: Meshes::default(),
            shaders: Shaders::default(),
            textures: Textures::default(),
            draws: Draws::default(),
            over: Draws::default(),
            colliders: false,
            outline: None,
            view: None,
            tickets: 0,
            picks: Vec::new(),
            unseen: Vec::new(),
            picked: Vec::new(),
            world,
            scratch: Scratch::default(),
            past: Past::default(),
            attached: false,
            root: None,
            assets,
            perf: Perf::default(),
            paused: false,
        }
    }

    /// The app's window; `None` in a headless run.
    pub fn window(&self) -> Option<&Window> {
        self.window.as_ref()
    }

    /// The app's window's physical pixels per logical pixel, as [`Window::scale`] says; 1 in a
    /// headless run.
    pub fn scale(&self) -> f64 {
        self.window.as_ref().map_or(1.0, Window::scale)
    }

    /// What's held down and where the cursor is, for layers that poll input rather than
    /// follow events.
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// The color each frame clears to, in linear RGBA.
    pub fn clear_color(&self) -> [f32; 4] {
        self.clear_color
    }

    /// Sets the color each frame clears to, in linear RGBA.
    pub fn set_clear_color(&mut self, color: [f32; 4]) {
        self.clear_color = color;
    }

    /// What a layer set the game's view to see the world from this frame, if one has.
    pub fn viewpoint(&self) -> Option<Viewpoint> {
        self.viewpoint
    }

    /// Sets what the game's view sees the world from this frame, in place of the world's camera,
    /// which it sees through in frames no layer sets one: of the entities with a [`Camera`] and a
    /// [`Transform`], the one that's primary; without one, [`Viewpoint::default`]. A projection
    /// that can't project panics as the frame renders, as [`Projection`](crate::Projection) says.
    pub fn set_viewpoint(&mut self, viewpoint: Viewpoint) {
        self.viewpoint = Some(viewpoint);
    }

    /// Where the game's view was in the main window as the last frame handed it to the renderer:
    /// its physical pixels from the window's top left, as [`Input::cursor`] places the cursor. The
    /// view is the main window's central area, which the panels dock around, or the panel that
    /// [`App::view_panel`](crate::App::view_panel) names. `None` while it has no area, as in a
    /// dockspace's first frame and while another panel's tab shows over the view's, and in replays,
    /// which render nothing. For what's presented, such as a camera that keeps the point under the
    /// cursor where it is, rather than the world.
    pub fn view(&self) -> Option<Viewport> {
        self.view
    }

    /// Asks which of the world's entities the game's view shows at `at`, in the main window's
    /// physical pixels from its top left, as [`Input::cursor`] places the cursor, in the frame
    /// being made: the one whose sprite, circle or mesh draws on top at that pixel, or whose
    /// collider's outline does, as [`set_colliders_shown`](Self::set_colliders_shown) shows them;
    /// and returns the ticket its answer comes back with in [`picked`](Self::picked), or `None`
    /// once the frame has asked 64, as many as a frame reads back. The answer comes four frames
    /// after the frame that asks, about 67 ms at 60 Hz: the main thread runs a frame ahead of the
    /// render thread, which reads the pixel back with a frame that the GPU finishes two frames
    /// later. It's `None` where the view shows no entity: where a layer's own draw is on top, as
    /// [`draw`](Self::draw), [`draw_sprite`](Self::draw_sprite),
    /// [`draw_circle`](Self::draw_circle), [`draw_line`](Self::draw_line) and
    /// [`draw_rect`](Self::draw_rect) draw, where the view shows its clear color, and outside the
    /// view, which is answered sooner, with the next answers to come back. Where a sprite, a
    /// circle, a line or a blended model is fully transparent, which their shaders discard, the
    /// pick finds what's behind it. A frame whose rendering had nothing to render into, as when the
    /// window's swapchain had no image to give, leaves its picks to the next that renders, which
    /// reads them by what it shows. Replays, which render nothing, answer none.
    pub fn pick(&mut self, at: [f64; 2]) -> Option<Pick> {
        if self.picks.len() == MAX_PICKS {
            return None;
        }
        let pick = Pick(self.tickets);
        self.tickets += 1;
        self.picks.push((pick, at));
        Some(pick)
    }

    /// The answers that came back with the frame the main loop last took back from the render
    /// thread: each a ticket that [`pick`](Self::pick) gave, with the entity the view showed at its
    /// pixel, or `None`. Each is here for one frame, until the answers that come back with the
    /// next take its place, after that frame's updates and UI; the entity may have gone since.
    pub fn picked(&self) -> &[(Pick, Option<EntityId>)] {
        &self.picked
    }

    /// Compiles the Slang shader at `path` for layers to draw meshes of vertex type `V` with from
    /// then on, which takes parameters of type `P` with each draw, such as a color or a texture:
    /// `()` for none. `blend` says how its color goes over what's drawn before it, such as
    /// [`Blend::Alpha`] for straight alpha, as PNGs store it; and so whether it writes depth: every
    /// shader draws only where it's as near as what's drawn there or nearer, and an opaque one
    /// writes its depth too, so that what's behind it, drawn later, doesn't show over it, while one
    /// that blends doesn't, so that what's behind it shows through it. `name` names it in errors
    /// and GPU captures. The shader keeps to the contract that [`gg_renderer::Shaders::add`]
    /// describes, which the engine checks.
    ///
    /// In a development build's windowed run, the shader compiles again as its file or a file it
    /// includes changes, and the next frame draws with it; a change that doesn't compile, or
    /// breaks the contract, warns, and the shader keeps drawing as it did.
    ///
    /// Fails with `renderer.bad_shader` and slangc's diagnostics if it doesn't compile, with
    /// `renderer.shader_mismatch` and each way it breaks the contract if it breaks it, with
    /// `renderer.shader_files` if slangc's directory in the OS's temporary directory can't be made
    /// or what it wrote there can't be read, and with `renderer.no_slangc` if slangc can't be run.
    pub fn add_shader<V: NoUninit, P: NoUninit>(
        &mut self,
        name: &str,
        path: impl AsRef<Path>,
        blend: Blend,
    ) -> Result<Shader<V, P>> {
        let shader = self.shaders.add(name, path.as_ref(), blend)?;
        let named = assets::name_of(self.root.as_deref(), path.as_ref());
        let types = format!("{} {} {blend:?}", layout::<V>(), layout::<P>());
        self.added("shader", &format!("{named} {types}"));
        Ok(shader)
    }

    /// Loads the PNG or JPEG file at `path` as a texture for sprites and layers' shaders to sample
    /// from then on, through the handle: a sprite's `texture`, or a field of a shader's parameters
    /// as it is. Any PNG loads, and any JPEG that's Huffman-coded at 8 bits, sequential or
    /// progressive, as nearly all are: its colors are taken as sRGB-encoded, whatever color profile
    /// or gamma it holds, as glTF requires, and its alpha as not multiplied into them, and shaders
    /// sample its colors decoded to linear, with texture coordinates from (0, 0) at its top left to
    /// (1, 1) at its bottom right. `name` names the texture in errors and GPU captures. The GPU has
    /// it from the next frame that renders.
    ///
    /// Fails with `engine.bad_texture` if the file can't be read, isn't a regular file, is more
    /// than 1 GiB, doesn't decode as a PNG or a JPEG, or has a side of more than 8,192 pixels, the
    /// most every Vulkan 1.4 device takes, and with `renderer.too_many_textures` past the 61,440 a
    /// renderer holds, the engine's white one among them.
    pub fn add_texture(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Texture> {
        let path = path.as_ref();
        let image = image::load_image(name, path)?;
        let texture = self
            .textures
            .add(name, image.size, image.pixels, Mipmaps::None)?;
        let named = assets::name_of(self.root.as_deref(), path);
        self.add_assets(|assets| assets.add_texture(texture, named.clone()));
        self.added("texture", &named);
        Ok(texture)
    }

    /// Loads the PNG or JPEG file at `path` as a sprite sheet: a texture, as
    /// [`add_texture`](Self::add_texture) loads one, cut into cells `cell` pixels across and down,
    /// packed edge to edge from its top left, which [`SpriteSheet::sprite`] draws.
    ///
    /// Fails as `add_texture` does, and with `engine.bad_sprite_sheet` if a side of `cell` is 0
    /// or more than the image's.
    pub fn add_sprite_sheet(
        &mut self,
        name: &str,
        path: impl AsRef<Path>,
        cell: [u32; 2],
    ) -> Result<SpriteSheet> {
        let path = path.as_ref();
        let image = image::load_image(name, path)?;
        let sheet = add_sheet(&mut self.textures, name, path, image, cell)?;
        let named = assets::name_of(self.root.as_deref(), path);
        self.add_assets(|assets| assets.add_texture(sheet.texture, named.clone()));
        self.added("texture", &named);
        Ok(sheet)
    }

    /// Adds a mesh for layers to draw from then on: triangles, three of `indices` each, which
    /// index `vertices`. A vertex is any `#[repr(C)]` type without padding, which derives
    /// bytemuck's `NoUninit`. `name` names the mesh in errors and GPU captures. The GPU has it
    /// from the next frame that renders.
    ///
    /// Fails with `renderer.bad_mesh` if it has no triangles, indices left over from whole
    /// triangles, or an index past its vertices.
    pub fn add_mesh<V: NoUninit>(
        &mut self,
        name: &str,
        vertices: &[V],
        indices: &[u32],
    ) -> Result<Mesh<V>> {
        let mesh = self.meshes.add(name, vertices, indices)?;
        self.added("mesh", &format!("{name} {}", layout::<V>()));
        Ok(mesh)
    }

    /// Loads the glTF 2.0 model at `path`, a `.gltf` or `.glb` file, and the files it names: what
    /// its default scene draws, `scene` or else the first, which [`Model::spawn`] places in worlds
    /// as entities, as many times as layers like. Its meshes and images are added once, as
    /// [`add_mesh`](Self::add_mesh) and [`add_texture`](Self::add_texture) add them, but each image
    /// with mip levels, made as a frame copies it to the GPU; the GPU has them from the next frame
    /// that renders. `name` names the model in errors and GPU captures.
    ///
    /// Of each node it takes the transform, a matrix or a translation, rotation and scale; of each
    /// primitive its mesh draws, the positions, the texture coordinates its base color texture
    /// names and its vertex colors, COLOR_0, as floats or normalized integers, and its indices, of
    /// 8, 16 or 32 bits; and of its material, the base color factor and texture, the texture's
    /// wrapping and whether it magnifies to the nearest texel, the alpha mode and cutoff, and
    /// whether it's double-sided. Models draw unlit, their base color alone, as KHR_materials_unlit
    /// has it, and a material that isn't double-sided draws its triangles' fronts alone. Buffers
    /// are in files in the model's folder, named by URIs relative to the model's, read no further
    /// than their byteLength, in base64 data URIs or in a GLB's BIN chunk, and images there or in
    /// buffer views: PNGs or JPEGs, as `add_texture` takes them, told apart by their bytes rather
    /// than by the types the model declares.
    ///
    /// It ignores animations, cameras and the extensions the model uses without requiring them,
    /// and logs which it has at info level, as `engine.model_ignored`: the model draws as it is at
    /// rest. It ignores without a word normals, tangents, other texture coordinates, the material's
    /// other parameters and textures, the textures' minification filters, since it samples them
    /// trilinearly and anisotropically unless they magnify to the nearest texel, morph targets
    /// while their weights are 0, the other scenes, and whatever the scene doesn't reach, which it
    /// doesn't read.
    ///
    /// Fails, adding nothing, with `engine.unsupported_model` for glTF the engine doesn't draw:
    /// glTF 1, a model without a scene, primitives that aren't triangle lists, sparse accessors and
    /// accessors without a buffer view, skinned nodes, morph weights other than 0, a sampler that
    /// wraps across and down differently, a texture without an image, any required extension but
    /// KHR_materials_unlit, and a model that takes more than 4 GiB to read, of its buffers, decoded
    /// images, vertices and nodes; with `engine.bad_model` for a file that can't be read, isn't a
    /// regular file or isn't glTF, as far as reading it checks, such as an accessor past its buffer
    /// view, an index past its vertices, a node with two parents, a matrix that isn't a
    /// translation, rotation and scale, a URI that's neither a data URI nor a relative path, or
    /// that has a `..`, or an image that doesn't decode, and, though glTF allows them, integers
    /// written as floats, such as `1.0`; and with `renderer.too_many_textures` if the renderer
    /// hasn't room for its images.
    pub fn add_model(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Model> {
        let zone = gg_core::zone!();
        let path = path.as_ref();
        zone.text(path.display());
        let imported = gltf::read(name, path, crate::dirs::read_file, gltf::MAX_MEMORY)?;
        if !imported.ignored.is_empty() {
            info!(
                code = "engine.model_ignored",
                model = name,
                path = %path.display(),
                "Model {name:?} draws without what the engine ignores of it: {}",
                imported.ignored.join(", ")
            );
        }
        let before = self.textures.count();
        let (meshes, shaders, textures) = (&mut self.meshes, &mut self.shaders, &mut self.textures);
        let model = Model::add(name, imported, meshes, shaders, textures)?;
        let named = assets::name_of(self.root.as_deref(), path);
        let count = self.textures.count();
        self.add_assets(|assets| assets.add_images(&named, before, count));
        let types = format!("{} {}", layout::<ModelVertex>(), layout::<ModelParams>());
        self.added("model", &format!("{named} {types}"));
        Ok(model)
    }

    /// Records that a layer added an asset of `kind`, as `what` says it: its name, and the types
    /// that a mesh's, a shader's or a model's draws hold the bytes of, as [`Assets`] keeps it.
    fn added(&mut self, kind: &str, what: &str) {
        let (meshes, shaders) = (self.meshes.count(), self.shaders.count());
        let added = format!("{kind} {meshes} {shaders} {} {what}", self.textures.count());
        self.add_assets(|assets| assets.note(added));
    }

    /// Has `add` change the assets the layers added, which the world shares from then on: the
    /// context's, so that a world that took the world's place doesn't lose them, and copied only
    /// where a copy of the world shares them.
    fn add_assets(&mut self, add: impl FnOnce(&mut Assets)) {
        // The world lets go of its share for the change.
        self.world.share_assets(&Arc::default());
        add(Arc::make_mut(&mut self.assets));
        self.world.share_assets(&self.assets);
    }

    /// Has the world share the assets the layers added again, if a layer put another world in its
    /// place, as it loaded, attached or ticked, as [`World::share_assets`] says.
    pub(crate) fn share_assets(&mut self) {
        self.world.share_assets(&self.assets);
    }

    /// Draws `mesh` with `shader` this frame, where `transform` places it in the world, over the
    /// meshes drawn before it but where an opaque one is nearer, in the game's view as the camera
    /// sees it; the shader gets `params`. The view is the main window's central area, which the
    /// panels dock around, or the panel that [`App::view_panel`](crate::App::view_panel) names.
    /// Consecutive draws of one mesh with one shader render as one instanced draw, whatever their
    /// transforms and parameters, so a frame of many takes few draws on the GPU. It's of no
    /// entity, so a [pick](Self::pick) where it's on top finds none.
    pub fn draw<V, P: NoUninit>(
        &mut self,
        mesh: Mesh<V>,
        shader: Shader<V, P>,
        transform: Mat4,
        params: P,
    ) {
        self.draws.push(mesh, shader, transform, params);
    }

    /// Draws `sprite` this frame, where `transform` places it in the world, over the meshes drawn
    /// before it but where an opaque one is nearer, in the game's view as the camera sees it.
    /// Unscaled, a sprite is a unit square around the transform's translation, as [`Sprite`] says.
    /// It's of no entity, so a [pick](Self::pick) where it's on top finds none.
    pub fn draw_sprite(&mut self, transform: Mat4, sprite: Sprite) {
        self.draws.push_sprite(transform, sprite);
    }

    /// Draws `circle` this frame, where `transform` places it in the world, over the meshes drawn
    /// before it but where an opaque one is nearer, in the game's view as the camera sees it.
    /// Unscaled, a circle is a unit across, around the transform's translation, as [`Circle`]
    /// says. It's of no entity, so a [pick](Self::pick) where it's on top finds none.
    pub fn draw_circle(&mut self, transform: Mat4, circle: Circle) {
        self.draws.push_circle(&mut self.shaders, transform, circle);
    }

    /// Draws a line from `from` to `to` in the world this frame, `width` pixels of the view wide
    /// however near or far it is, or, thinner than a pixel, a pixel wide and as much fainter as
    /// it's thinner, its ends squared off half its width past its points, in `color`, linear RGBA
    /// with alpha not multiplied in: over everything else the view draws, the world's entities as
    /// well as what layers draw, but where an opaque mesh is nearer, so that it shows as debugging
    /// wants. It's of no entity, so a [pick](Self::pick) where it's on top finds none.
    pub fn draw_line(&mut self, from: Vec3, to: Vec3, color: Vec4, width: f32) {
        let line = Line {
            from,
            to,
            color,
            width,
        };
        self.over
            .push_line(&mut self.shaders, Mat4::IDENTITY, line, 0);
    }

    /// Draws the outline of the square that `transform` places in the world this frame, a unit
    /// across around its translation unscaled, as a sprite is: its four sides, as
    /// [`draw_line`](Self::draw_line) draws lines, which meet at its corners.
    pub fn draw_rect(&mut self, transform: Mat4, color: Vec4, width: f32) {
        self.over
            .push_rect(&mut self.shaders, transform, color, width, 0);
    }

    /// Outlines what the game's view shows of entity `id` this frame, as the editor outlines what's
    /// picked: round whatever shape the pixels make where its draws are on top, its colliders'
    /// outlines among them, even across something in front of it, but not round what's hidden;
    /// over what the view shows and under the UI, in `color`, linear RGBA with alpha not
    /// multiplied in, `width` pixels of the view wide, at most 8, its outer edge smoothed. One
    /// entity a frame: a later call replaces an earlier one's.
    pub fn outline(&mut self, id: EntityId, color: Vec4, width: f32) {
        let id = id.get();
        self.outline = Some(Outline { id, color, width });
    }

    /// What a layer has [outlined](Self::outline) this frame, if anything: the entity, the color
    /// and the width.
    pub fn outlined(&self) -> Option<(EntityId, Vec4, f32)> {
        let Outline { id, color, width } = self.outline?;
        Some((EntityId::new(id)?, color, width))
    }

    /// Whether the world's colliders' outlines draw, as
    /// [`set_colliders_shown`](Self::set_colliders_shown) says: not at first.
    pub fn colliders_shown(&self) -> bool {
        self.colliders
    }

    /// Has the world's colliders' outlines draw each frame from then on, or not: each
    /// [`BoxCollider2D`] and [`CircleCollider2D`] whose entity has a [`Transform`], as physics
    /// shapes it, 2 points wide in light green, where its entity draws, over everything else the
    /// view draws but a nearer opaque mesh, as lines are; with its entity's ID, so that a
    /// [pick](Self::pick) of an outline finds its entity. A collider's outline draws whether
    /// physics moves its entity or not, as it doesn't without a [`RigidBody2D`] or under a parent.
    pub fn set_colliders_shown(&mut self, shown: bool) {
        self.colliders = shown;
    }

    /// The world: its entities and their components, as the frame's ticks left it. After the
    /// layers' updates each frame, the engine draws those with a [`Transform`] and a [`Sprite`], a
    /// [`Circle`] or a [`MeshDraw`], after what the layers drew themselves: the meshes whose
    /// shaders are opaque first, whatever their z's, by shader, then mesh, then the rest back to
    /// front by their z's in the world, at one z sprites, then circles, under meshes, each in the
    /// order of their IDs, children where their parents carry them; each on its way from where it
    /// was as the latest tick started to where the tick left it, so up to a tick behind the world
    /// here.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// The world, to change as a layer attaches or detaches. At any other time it changes only at
    /// ticks, which [`Layer::tick`] is given it for, and this panics: a layer queues a command for
    /// its next tick instead.
    pub fn world_mut(&mut self) -> &mut World {
        assert!(
            !self.attached,
            "a layer changes the world only as it attaches or detaches, and at its ticks: queue a \
             command for its next tick"
        );
        &mut self.world
    }

    /// Shows `dialog`, a file dialog of the OS's own, over the main window, modal, and returns
    /// the path chosen as it closes: `None` if it was cancelled, or if there's no window, as in
    /// headless runs and tests, which show none. For tools' menus, from [`Layer::ui`], as an
    /// item's click opens it. Frames pause while it's open, the render thread finishing the frame
    /// it has, and no frame's time counts the pause: the next update gets none, as the first
    /// after the window is restored from being minimized does.
    #[cfg(feature = "dev")]
    pub fn pick_file(&mut self, dialog: &crate::FileDialog<'_>) -> Option<std::path::PathBuf> {
        let Some(window) = &self.window else {
            tracing::debug!(
                code = "engine.no_dialog",
                title = dialog.title,
                "No window to show the file dialog {:?} over, so it isn't shown",
                dialog.title
            );
            return None;
        };
        let waiting = crate::api::waiting(crate::api::Waiting::Dialog);
        let picked = dialog.show(&window.raw);
        drop(waiting);
        self.paused = true;
        tracing::debug!(
            title = dialog.title,
            picked = picked.as_ref().map(|path| path.display().to_string()),
            "File dialog closed"
        );
        picked
    }

    /// Copies where the world's entities are, as a tick starts, for the frames after it to draw
    /// them on from.
    pub(crate) fn capture(&mut self) {
        self.past.capture(&self.world);
    }

    /// Draws the world's entities, after what the layers drew this frame, each `alpha` of the way
    /// from where it was as the latest tick started to where the tick left it, and their colliders'
    /// outlines, over the rest, while they're shown; and finds where the world's camera sees from,
    /// as it draws.
    pub(crate) fn draw_world(&mut self, alpha: f32) {
        self.past.resolve(&self.world);
        let (world, past) = (&self.world, &self.past);
        let (draws, shaders) = (&mut self.draws, &mut self.shaders);
        draw::draw_world(world, past, alpha, draws, shaders, &mut self.scratch);
        self.seen = camera::seen(world, past, alpha, &mut self.tie);
        if self.colliders
            && let Some(poses) = Poses::new(world, past, alpha)
        {
            let scale = self.scale() as f32;
            physics::draw_colliders(world, &poses, scale, &mut self.over, &mut self.shaders);
        }
    }

    /// Fills in `frame` with this frame's render data, with the game's view at `viewport`, which
    /// [`view`](Self::view) gives from then on: its draws, those over the rest last, for the last
    /// frame's, which it forgets, keeping their memory for the next; the viewpoint a layer set this
    /// frame, which it forgets too, or else where the world's camera sees from, or else the
    /// default; what a layer outlined this frame, which it forgets too; what's been added since a
    /// frame last took it, after what `frame` still holds; and the picks asked this frame, each at
    /// the pixel of the view under it, the floor of where it was asked less the view's origin, but
    /// those outside the view, which the next answers to come back answer `None`.
    pub(crate) fn extract(&mut self, frame: &mut Extracted, viewport: Viewport) {
        frame.clear_color = self.clear_color;
        frame.viewport = viewport;
        self.view = (viewport.size[0] > 0 && viewport.size[1] > 0).then_some(viewport);
        frame.viewpoint = self.viewpoint.take().or(self.seen).unwrap_or_default();
        frame.outline = self.outline.take();
        self.draws.append(&mut self.over);
        mem::swap(&mut frame.draws, &mut self.draws);
        self.draws.clear();
        (frame.uploads).take(&mut self.meshes, &mut self.shaders, &mut self.textures);
        frame.picks.clear();
        let [left, top] = viewport.origin.map(f64::from);
        let [width, height] = viewport.size.map(f64::from);
        for (pick, [x, y]) in self.picks.drain(..) {
            // Not a NaN, which no range contains.
            let [x, y] = [x.floor() - left, y.floor() - top];
            if (0.0..width).contains(&x) && (0.0..height).contains(&y) {
                let pixel = [x as u32, y as u32];
                frame.picks.push(gg_renderer::Pick { tag: pick.0, pixel });
            } else {
                self.unseen.push(pick);
            }
        }
    }

    /// Takes `answers`, which came back with the frame taken back from the render thread, each the
    /// number of a pick's ticket with the ID the view showed at its pixel, in place of those
    /// before; and answers `None` the picks the last frame handed off asked outside the view.
    pub(crate) fn take_answers(&mut self, answers: &[(u64, u64)]) {
        self.picked.clear();
        for &(tag, id) in answers {
            self.picked.push((Pick(tag), EntityId::new(id)));
        }
        for pick in self.unseen.drain(..) {
            self.picked.push((pick, None));
        }
    }

    /// The shaders layers added, for hot reload to put those it compiles again in their place.
    #[cfg(feature = "dev")]
    pub(crate) fn shaders(&mut self) -> &mut Shaders {
        &mut self.shaders
    }
}

/// Adds `image`, sprite sheet `name`'s from `path`, to `textures` as a sheet of cells `cell` pixels
/// across and down, if they fit it; one that doesn't takes no texture.
fn add_sheet(
    textures: &mut Textures,
    name: &str,
    path: &Path,
    image: Image,
    cell: [u32; 2],
) -> Result<SpriteSheet> {
    let [width, height] = image.size;
    if !(1..=width).contains(&cell[0]) || !(1..=height).contains(&cell[1]) {
        return Err(Error::new(
            "engine.bad_sprite_sheet",
            format!(
                "sprite sheet {name:?} at {} is {width}x{height} pixels, but its cells are {}x{}, \
                 where each side is 1 pixel up to the image's",
                path.display(),
                cell[0],
                cell[1]
            ),
        ));
    }
    let texture = textures.add(name, image.size, image.pixels, Mipmaps::None)?;
    Ok(SpriteSheet {
        texture,
        size: image.size,
        cell,
    })
}

/// Type `T`'s name and size, such as `glam::f32::vec3::Vec3 12`: what a snapshot's assets say a
/// draw holds the bytes of, so that a draw kept in one comes back only where they're the same.
fn layout<T>() -> String {
    format!("{} {}", type_name::<T>(), size_of::<T>())
}

/// The name of type `L`, without its path or its generics: such as `Inspector`.
fn short_name<L>() -> &'static str {
    let name = type_name::<L>();
    let name = name.split('<').next().unwrap_or(name);
    name.rsplit("::").next().unwrap_or(name)
}

/// The app's window, as layers see it.
pub struct Window {
    raw: Arc<winit::window::Window>,
}

impl Window {
    pub(crate) fn new(raw: Arc<winit::window::Window>) -> Self {
        Self { raw }
    }

    /// The drawable area's width and height in physical pixels.
    pub fn size(&self) -> [u32; 2] {
        let size = self.raw.inner_size();
        [size.width, size.height]
    }

    /// Physical pixels per logical pixel, such as 2 on a high-density display.
    pub fn scale(&self) -> f64 {
        self.raw.scale_factor()
    }
}

/// Layers from the bottom up, with the overlays above the layers.
#[derive(Default)]
pub(crate) struct Stack {
    layers: Vec<Box<dyn Layer>>,
    /// The name of each layer's type, in the same order, which a recording names it by.
    names: Vec<&'static str>,
    /// The full name of each layer's type, in the same order, which tells a layer's own default
    /// `ui` from another layer's that its `ui` calls.
    types: Vec<&'static str>,
    /// Where the overlays start.
    overlays: usize,
    /// What records the run, if it's recorded.
    recorder: Option<Recorder>,
}

impl Stack {
    /// Adds a layer on top of the other layers, under the overlays.
    pub(crate) fn push_layer<L: Layer + 'static>(&mut self, layer: L) {
        #[cfg(feature = "dev")]
        let layer = Hot(layer);
        self.layers.insert(self.overlays, Box::new(layer));
        self.names.insert(self.overlays, short_name::<L>());
        self.types.insert(self.overlays, type_name::<L>());
        self.overlays += 1;
    }

    /// Adds an overlay on top of everything.
    pub(crate) fn push_overlay<L: Layer + 'static>(&mut self, overlay: L) {
        #[cfg(feature = "dev")]
        let overlay = Hot(overlay);
        self.layers.push(Box::new(overlay));
        self.names.push(short_name::<L>());
        self.types.push(type_name::<L>());
    }

    /// The names of the layers' types, from the bottom up.
    pub(crate) fn names(&self) -> &[&'static str] {
        &self.names
    }

    /// Has `recorder` record the run, from the layers' attach.
    pub(crate) fn record(&mut self, recorder: Recorder) {
        self.recorder = Some(recorder);
    }

    /// Fails, once the run's recording has stopped, as [`Recorder::check`] says.
    pub(crate) fn recorded(&self) -> Result<()> {
        self.recorder.as_ref().map_or(Ok(()), Recorder::check)
    }

    /// Loads the layers, then attaches them, each from the bottom up, in `cx`, whose world nothing
    /// has drawn an ID from: its seed is what a recording starts from.
    pub(crate) fn attach(&mut self, cx: &mut Context) {
        let _zone = gg_core::zone!();
        if let Some(recorder) = &mut self.recorder {
            recorder.start(&self.names, cx.world.stream_state());
        }
        self.load(cx);
        for layer in &mut self.layers {
            layer.attach(cx);
            cx.share_assets();
        }
        cx.attached = true;
        if let Some(recorder) = &mut self.recorder {
            recorder.attached(&cx.world);
        }
    }

    /// Loads the layers, then puts `snapshot`'s world in `cx`'s place, as a run restored from it
    /// starts, and has each layer pick up from it, each from the bottom up; returns the ticks the
    /// simulation had run as it was taken, which the run's go on from.
    ///
    /// Fails as [`Snapshot::restore`] does.
    pub(crate) fn restore(&mut self, cx: &mut Context, snapshot: &Snapshot) -> Result<u64> {
        let _zone = gg_core::zone!();
        self.load(cx);
        let ticks = snapshot.restore(&mut cx.world, &self.names)?;
        cx.attached = true;
        for layer in &mut self.layers {
            layer.restored(cx);
        }
        Ok(ticks)
    }

    /// Loads the layers, from the bottom up.
    ///
    /// # Panics
    ///
    /// If a layer spawned an entity, or changed the world's rows otherwise, as it loaded: a
    /// restored run, whose world comes from a snapshot, would spawn it again.
    fn load(&mut self, cx: &mut Context) {
        let before = cx.world.version();
        for (layer, name) in self.layers.iter_mut().zip(&self.names) {
            layer.load(cx);
            cx.share_assets();
            assert_eq!(
                cx.world.version(),
                before,
                "layer {name} changed the world's entities as it loaded, which its attach does, so \
                 that a run restored from a snapshot doesn't spawn them again"
            );
        }
    }

    /// The world a snapshot of the run saves: the one a layer holds aside, as
    /// [`Layer::saved_world`] says, or else `cx`'s.
    #[cfg(feature = "dev")]
    pub(crate) fn saved_world<'w>(&'w self, cx: &'w Context) -> &'w World {
        let held = self.layers.iter().find_map(|layer| layer.saved_world());
        held.unwrap_or(&cx.world)
    }

    /// Detaches from the top down, the reverse of attaching, once a recording has its end.
    pub(crate) fn detach(&mut self, cx: &mut Context) {
        let _zone = gg_core::zone!();
        if let Some(recorder) = &mut self.recorder {
            recorder.end();
        }
        cx.attached = false;
        for layer in self.layers.iter_mut().rev() {
            layer.detach(cx);
        }
    }

    /// Runs a tick of the simulation: each layer's, from the bottom up, after which the world
    /// shares the context's assets again, as [`Context::share_assets`] says. A recording gets each
    /// layer's commands just before its tick applies them, so that a tick that panics leaves them,
    /// and the world's hash after the last.
    pub(crate) fn tick(&mut self, cx: &mut Context, tick: Tick) {
        let _zone = gg_core::zone!();
        let Self {
            layers,
            names,
            recorder,
            ..
        } = self;
        for (index, layer) in layers.iter_mut().enumerate() {
            if let Some(recorder) = recorder.as_mut()
                && let Some(commands) = layer.commands()
            {
                recorder.queued(index, names[index], commands, &cx.world);
            }
            layer.tick(&mut cx.world, tick);
            cx.share_assets();
        }
        if let Some(recorder) = recorder {
            recorder.ticked(tick, &cx.world);
        }
    }

    /// Runs a tick as a replay does: as [`tick`](Self::tick) does, with each layer's queue emptied
    /// and filled again by `fill`, which gets the layer's index, its queue, if it has one, and the
    /// world as its tick finds it.
    pub(crate) fn replay_tick(
        &mut self,
        cx: &mut Context,
        tick: Tick,
        mut fill: impl FnMut(usize, Option<&mut dyn Commands>, &World) -> Result<()>,
    ) -> Result<()> {
        let _zone = gg_core::zone!();
        for (index, layer) in self.layers.iter_mut().enumerate() {
            fill(index, layer.commands(), &cx.world)?;
            layer.tick(&mut cx.world, tick);
            cx.share_assets();
        }
        Ok(())
    }

    pub(crate) fn update(&mut self, cx: &mut Context, dt: Duration) {
        let _zone = gg_core::zone!();
        for layer in &mut self.layers {
            layer.update(cx, dt);
        }
    }

    /// Draws UI from the bottom up, so the layers above draw over those below, and returns
    /// whether any layer draws UI: every layer does but one whose `ui` is its own type's default,
    /// [`Layer::ui`]'s. One whose `ui` calls another layer's default draws UI as far as this goes,
    /// whatever it draws.
    pub(crate) fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) -> bool {
        let _zone = gg_core::zone!();
        let mut draws = false;
        for (layer, &own) in self.layers.iter_mut().zip(&self.types) {
            layer.ui(cx, ui);
            draws |= ui.take_default_ui() != Some(own);
        }
        draws
    }

    /// Records `event` in the input state that layers poll, then hands it down from the top
    /// until a layer handles it. `Break` means one did. A release, a gamepad's stick or trigger
    /// back at rest, and the loss of focus, go on to the layers below one that handles them, as
    /// releases go past the UI, so that none is left holding what it saw pressed: a release without
    /// its press changes nothing.
    pub(crate) fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        cx.input.record(event);
        let lets_go = match *event {
            Event::Key { pressed, .. }
            | Event::MouseButton { pressed, .. }
            | Event::PadButton { pressed, .. } => !pressed,
            Event::PadStick { value, .. } => value == Vec2::ZERO,
            Event::PadTrigger { value, .. } => value == 0.0,
            Event::Focused(focused) => !focused,
            _ => false,
        };
        let mut handled = ControlFlow::Continue(());
        for layer in self.layers.iter_mut().rev() {
            if layer.event(cx, event).is_break() {
                handled = ControlFlow::Break(());
                if !lets_go {
                    break;
                }
            }
        }
        handled
    }
}

/// A layer as a development build's stack holds it, which calls each of the layer's methods
/// through subsecond's `HotFn`, as [`Layer`] says, since the stack calls what it holds through a
/// vtable, which no patch changes, while `HotFn`, in a build with debug assertions, looks each call
/// up in the jump table of the last patch applied. Each call goes through the method as a function
/// item, such as `L::tick`: `HotFn` finds a function of no size in the table by its own `call_it`
/// for it, which the game's crate makes, since `L` is the game's, and so the patch makes again; it
/// takes anything the size of a pointer, a closure that holds one among them, for a function
/// pointer, and calls it as one. A method added to `Layer` needs its call here too, or the trait's
/// default runs in place of the layer's own. The stack names the layer by `L`, and `L`'s default
/// `ui` names `L`, as the stack's types do. A call allocates nothing.
#[cfg(feature = "dev")]
struct Hot<L>(L);

#[cfg(feature = "dev")]
impl<L: Layer> Layer for Hot<L> {
    fn load(&mut self, cx: &mut Context) {
        HotFn::current(L::load).call((&mut self.0, cx));
    }

    fn attach(&mut self, cx: &mut Context) {
        HotFn::current(L::attach).call((&mut self.0, cx));
    }

    fn restored(&mut self, cx: &mut Context) {
        HotFn::current(L::restored).call((&mut self.0, cx));
    }

    fn saved_world(&self) -> Option<&World> {
        HotFn::current(L::saved_world).call((&self.0,))
    }

    fn detach(&mut self, cx: &mut Context) {
        HotFn::current(L::detach).call((&mut self.0, cx));
    }

    fn tick(&mut self, world: &mut World, tick: Tick) {
        HotFn::current(L::tick).call((&mut self.0, world, tick));
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        HotFn::current(L::commands).call((&mut self.0,))
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        HotFn::current(L::update).call((&mut self.0, cx, dt));
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        HotFn::current(L::event).call((&mut self.0, cx, event))
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        HotFn::current(L::ui).call((&mut self.0, cx, ui));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::{Key, Transform, render_thread};

    /// Records each call it gets, by layer name, and handles events if `handles` is set.
    struct Probe {
        name: &'static str,
        handles: bool,
        calls: Rc<RefCell<Vec<String>>>,
    }

    impl Layer for Probe {
        fn attach(&mut self, _: &mut Context) {
            self.calls
                .borrow_mut()
                .push(format!("attach {}", self.name));
        }
        fn detach(&mut self, _: &mut Context) {
            self.calls
                .borrow_mut()
                .push(format!("detach {}", self.name));
        }
        fn tick(&mut self, _: &mut World, _: Tick) {
            self.calls.borrow_mut().push(format!("tick {}", self.name));
        }
        fn update(&mut self, _: &mut Context, _: Duration) {
            self.calls
                .borrow_mut()
                .push(format!("update {}", self.name));
        }
        fn event(&mut self, _: &mut Context, _: &Event<'_>) -> ControlFlow<()> {
            self.calls.borrow_mut().push(format!("event {}", self.name));
            if self.handles {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }
    }

    /// Spawns an entity as it attaches, at each tick, and as it detaches; and as it updates, if
    /// `rogue`.
    struct Spawner {
        rogue: bool,
    }

    impl Layer for Spawner {
        fn attach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn(Transform::IDENTITY);
        }
        fn detach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn(Transform::IDENTITY);
        }
        fn tick(&mut self, world: &mut World, _: Tick) {
            world.spawn(Transform::IDENTITY);
        }
        fn update(&mut self, cx: &mut Context, _: Duration) {
            if self.rogue {
                cx.world_mut().spawn(Transform::IDENTITY);
            }
        }
    }

    /// Notes, for each key event it's sent, whether the input state shows that key held.
    struct Poll(Rc<RefCell<Vec<bool>>>);

    impl Layer for Poll {
        fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
            if let Event::Key { key, .. } = *event {
                self.0.borrow_mut().push(cx.input().key_down(key));
            }
            ControlFlow::Continue(())
        }
    }

    /// Pushes, in this order: overlay `o`, layer `a`, layer `b` (which handles events if
    /// `b_handles`), and returns the stack, bottom to top `a b o`, with its call record.
    fn probes(b_handles: bool) -> (Stack, Rc<RefCell<Vec<String>>>) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let probe = |name, handles| Probe {
            name,
            handles,
            calls: calls.clone(),
        };
        let mut stack = Stack::default();
        stack.push_overlay(probe("o", false));
        stack.push_layer(probe("a", false));
        stack.push_layer(probe("b", b_handles));
        (stack, calls)
    }

    #[test]
    fn lifecycle_goes_up_the_stack_and_detach_comes_back_down() {
        let (mut stack, calls) = probes(false);
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        stack.tick(&mut cx, Tick { number: 0 });
        stack.update(&mut cx, Duration::ZERO);
        stack.detach(&mut cx);
        assert_eq!(
            *calls.borrow(),
            [
                "attach a", "attach b", "attach o", "tick a", "tick b", "tick o", "update a",
                "update b", "update o", "detach o", "detach b", "detach a"
            ]
        );
        assert_eq!(stack.names(), ["Probe"; 3]);
        assert_eq!(short_name::<Vec<crate::inspector::Inspector>>(), "Vec");
    }

    /// Puts a new world in the context's place at each tick.
    struct Replacer;

    impl Layer for Replacer {
        fn tick(&mut self, world: &mut World, _: Tick) {
            *world = World::default();
        }
    }

    /// A world a tick puts in the context's place shares the context's assets after it, so that
    /// the textures the layers added stay named, and one added afterwards is named after them.
    #[test]
    fn a_world_put_in_the_contexts_place_shares_its_assets() {
        let mut stack = Stack::default();
        stack.push_layer(Replacer);
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        let add = |cx: &mut Context, path: &str| {
            let texture = cx.textures.add(path, [1, 1], &[0; 4], Mipmaps::None);
            let texture = texture.expect("a texture");
            cx.add_assets(|assets| assets.add_texture(texture, path.into()));
            texture
        };
        let first = add(&mut cx, "first.png");
        stack.tick(&mut cx, Tick { number: 0 });
        let second = add(&mut cx, "second.png");
        stack.tick(&mut cx, Tick { number: 1 });
        let assets = cx.world().assets();
        assert_eq!(
            [first, second].map(|texture| assets.path(texture)),
            [Some("first.png"), Some("second.png")]
        );
    }

    #[test]
    fn layers_change_the_world_as_they_attach_and_detach_and_at_their_ticks() {
        let mut stack = Stack::default();
        stack.push_layer(Spawner { rogue: false });
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        stack.tick(&mut cx, Tick { number: 0 });
        stack.update(&mut cx, Duration::ZERO);
        stack.detach(&mut cx);
        assert_eq!(cx.world().query::<&Transform>().count(), 3);
    }

    /// Each component type the engine registers has a doc, and so does each of its named fields,
    /// which the Inspector shows on hover.
    #[test]
    fn the_engines_components_and_their_named_fields_have_docs() {
        let cx = Context::new(None);
        let types = cx.world().types();
        let mut undocumented = Vec::new();
        for name in cx.world().component_types() {
            let info = types.info(types.find(name).expect("described"));
            if info.doc.is_empty() {
                undocumented.push(name.to_owned());
            }
            if let crate::reflect::Shape::Struct {
                fields,
                tuple: false,
            } = &info.shape
            {
                let bare = fields.iter().filter(|field| field.meta.doc.is_empty());
                undocumented.extend(bare.map(|field| format!("{name}.{}", field.name)));
            }
        }
        assert!(undocumented.is_empty(), "undocumented: {undocumented:?}");
    }

    #[test]
    #[should_panic(expected = "only as it attaches or detaches")]
    fn a_layer_changing_the_world_as_it_updates_is_a_bug() {
        let mut stack = Stack::default();
        stack.push_layer(Spawner { rogue: true });
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        stack.update(&mut cx, Duration::ZERO);
    }

    #[test]
    fn events_go_down_the_stack_until_a_layer_handles_them_but_releases_go_on() {
        let mut cx = Context::new(None);
        let (mut stack, calls) = probes(false);
        assert_eq!(
            stack.event(&mut cx, &Event::CursorEntered),
            ControlFlow::Continue(())
        );
        assert_eq!(*calls.borrow(), ["event o", "event b", "event a"]);

        // A press, and a stick or a trigger away from rest, stop at the layer that handles them.
        let pad = crate::Pad(0);
        let stick = |value| Event::PadStick {
            pad,
            stick: crate::Stick::Left,
            value,
        };
        let trigger = |value| Event::PadTrigger {
            pad,
            trigger: crate::Trigger::Left,
            value,
        };
        let south = |pressed| Event::PadButton {
            pad,
            button: crate::PadButton::South,
            pressed,
        };
        for handled in [
            Event::CursorEntered,
            south(true),
            stick(Vec2::X),
            trigger(0.5),
        ] {
            let (mut stack, calls) = probes(true);
            assert_eq!(stack.event(&mut cx, &handled), ControlFlow::Break(()));
            assert_eq!(*calls.borrow(), ["event o", "event b"], "{handled:?}");
        }

        // So that the layers below let go too.
        for lets_go in [
            Event::Key {
                key: Key::Space,
                pressed: false,
                repeat: false,
            },
            Event::MouseButton {
                button: crate::MouseButton::Left,
                pressed: false,
            },
            Event::Focused(false),
            south(false),
            stick(Vec2::ZERO),
            trigger(0.0),
        ] {
            let (mut stack, calls) = probes(true);
            assert_eq!(stack.event(&mut cx, &lets_go), ControlFlow::Break(()));
            assert_eq!(
                *calls.borrow(),
                ["event o", "event b", "event a"],
                "{lets_go:?}"
            );
        }
    }

    #[test]
    fn the_input_state_takes_each_event_before_the_layers() {
        let held = Rc::new(RefCell::new(Vec::new()));
        let mut stack = Stack::default();
        stack.push_layer(Poll(held.clone()));
        let mut cx = Context::new(None);
        for pressed in [true, false] {
            let key = Event::Key {
                key: Key::Space,
                pressed,
                repeat: false,
            };
            let _ = stack.event(&mut cx, &key);
        }
        assert_eq!(*held.borrow(), [true, false]);
    }

    /// Keeps [`Layer::ui`]'s default.
    struct Quiet;

    impl Layer for Quiet {}

    /// Draws nothing, through a `ui` of its own.
    struct Blank;

    impl Layer for Blank {
        fn ui(&mut self, _: &mut Context, _: &mut Ui<'_>) {}
    }

    /// Draws what the layer it wraps does, through a `ui` of its own.
    struct Wrapper(Quiet);

    impl Layer for Wrapper {
        fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
            self.0.ui(cx, ui);
        }
    }

    /// Whether `stack`'s layers draw UI, as a pass of egui's tells.
    fn draws_ui(mut stack: Stack) -> bool {
        let egui = crate::egui_context();
        let mut dock = crate::dock::Dock::new(None, crate::chrome::Chrome::Os, None, false, &[]);
        let mut cx = Context::new(None);
        let mut draws = false;
        let mut output = egui.run_ui(egui::RawInput::default(), |root| {
            dock.begin(root);
            draws |= stack.ui(&mut cx, &mut Ui::new(root, &mut dock));
            dock.end(root);
        });
        // As the renderer would have.
        output.textures_delta.clear();
        draws
    }

    /// A layer draws UI unless its `ui` is its own type's default: one that has a `ui` of its own
    /// draws, though it draws nothing, as does one whose `ui` calls another layer's default.
    #[test]
    fn layers_draw_ui_unless_theirs_is_the_default() {
        let stack = |push: &dyn Fn(&mut Stack)| {
            let mut stack = Stack::default();
            push(&mut stack);
            stack
        };
        assert!(!draws_ui(stack(&|_| {})), "no layers");
        assert!(!draws_ui(stack(&|stack| {
            stack.push_layer(Quiet);
            stack.push_overlay(Quiet);
        })));
        assert!(draws_ui(stack(&|stack| {
            stack.push_layer(Quiet);
            stack.push_overlay(Blank);
        })));
        assert!(draws_ui(stack(&|stack| stack.push_layer(Wrapper(Quiet)))));
    }

    #[test]
    fn a_sprite_sheet_loads_only_if_its_cells_fit_its_image() {
        let mut textures = Textures::default();
        let path = Path::new("assets/tiles.png");
        let image = || Image {
            size: [64, 32],
            pixels: vec![0; 64 * 32 * 4],
        };
        for cell in [[0, 16], [16, 0], [65, 16], [16, 33]] {
            let error = add_sheet(&mut textures, "tiles", path, image(), cell)
                .expect_err("cells that don't fit");
            assert_eq!(error.code(), "engine.bad_sprite_sheet", "{cell:?}");
            let message = error.to_string();
            assert!(
                message.contains("\"tiles\"")
                    && message.contains(&path.display().to_string())
                    && message.contains("64x32"),
                "{message}"
            );
        }
        let sheet = add_sheet(&mut textures, "tiles", path, image(), [64, 32]).expect("a sheet");
        assert_eq!((sheet.size, sheet.cell), ([64, 32], [64, 32]));
        // The sheets that didn't load took no texture, so this one's comes first after the
        // engine's white one.
        let first = Textures::default().add("first", [1, 1], &[0; 4], Mipmaps::None);
        assert_eq!(sheet.texture, first.expect("a texture"));
    }

    #[test]
    fn a_model_that_cant_be_read_fails_naming_its_file() {
        let path = Path::new("no such folder/truck.glb");
        let error = Context::seeded(0)
            .add_model("truck", path)
            .expect_err("no model");
        assert_eq!(error.code(), "engine.bad_model");
        let message = error.to_string();
        assert!(
            message.contains("\"truck\"") && message.contains(&path.display().to_string()),
            "{message}"
        );
    }

    /// What a layer outlines is the frame's it's set in alone, as `outlined` says until the frame
    /// takes it; a later call in the frame replaces an earlier one's.
    #[test]
    fn an_outline_is_its_frames_alone() {
        let viewport = Viewport {
            origin: [0, 0],
            size: [640, 360],
        };
        let mut frame = render_thread::Extracted::default();
        let mut cx = Context::seeded(1);
        let [first, later]: [EntityId; 2] =
            ["0123456789abcdef", "fedcba9876543210"].map(|id| id.parse().expect("an ID"));
        let orange = Vec4::new(1.0, 0.35, 0.02, 1.0);
        cx.outline(first, Vec4::ONE, 2.0);
        cx.outline(later, orange, 3.0);
        assert_eq!(cx.outlined(), Some((later, orange, 3.0)));
        cx.extract(&mut frame, viewport);
        let taken = Outline {
            id: later.get(),
            color: orange,
            width: 3.0,
        };
        assert_eq!(frame.outline, Some(taken));
        assert_eq!(cx.outlined(), None);
        cx.extract(&mut frame, viewport);
        assert_eq!(frame.outline, None, "the next frame outlines nothing");
    }

    #[test]
    fn layers_know_where_the_view_was_handed_off_while_it_had_an_area() {
        let mut cx = Context::new(None);
        assert_eq!(cx.view(), None);
        let mut frame = render_thread::Extracted::default();
        let shown = egui::Rect::from_min_max(egui::pos2(10.0, 40.0), egui::pos2(410.0, 340.0));
        cx.extract(&mut frame, Viewport::from_points(shown, 1.5));
        let view = Viewport {
            origin: [15, 60],
            size: [600, 450],
        };
        assert_eq!(cx.view(), Some(view));
        // As while a tab shows over a dockspace's view, whose dock then has it nowhere.
        cx.extract(&mut frame, Viewport::from_points(egui::Rect::NOTHING, 1.5));
        assert_eq!(cx.view(), None);
    }

    /// Records each call of its methods but `saved_world`, which takes it as it is, and holds a
    /// world aside and a queued command.
    #[cfg(feature = "dev")]
    #[derive(Default)]
    struct Keeper {
        calls: Vec<&'static str>,
        held: World,
        queued: Vec<u32>,
    }

    #[cfg(feature = "dev")]
    impl Layer for Keeper {
        fn load(&mut self, _: &mut Context) {
            self.calls.push("load");
        }
        fn attach(&mut self, _: &mut Context) {
            self.calls.push("attach");
        }
        fn restored(&mut self, _: &mut Context) {
            self.calls.push("restored");
        }
        fn saved_world(&self) -> Option<&World> {
            Some(&self.held)
        }
        fn detach(&mut self, _: &mut Context) {
            self.calls.push("detach");
        }
        fn tick(&mut self, _: &mut World, _: Tick) {
            self.calls.push("tick");
        }
        fn commands(&mut self) -> Option<&mut dyn Commands> {
            self.calls.push("commands");
            Some(&mut self.queued)
        }
        fn update(&mut self, _: &mut Context, _: Duration) {
            self.calls.push("update");
        }
        fn event(&mut self, _: &mut Context, _: &Event<'_>) -> ControlFlow<()> {
            self.calls.push("event");
            ControlFlow::Break(())
        }
    }

    /// A development build's stack holds each layer in a `Hot`, which calls the layer's own
    /// methods and gives back what they return; `ui` is
    /// `layers_draw_ui_unless_theirs_is_the_default`'s to check, through the stack.
    #[cfg(feature = "dev")]
    #[test]
    fn a_hot_layer_calls_the_layers_own_methods() {
        let mut hot = Hot(Keeper {
            queued: vec![7],
            ..Keeper::default()
        });
        let (mut cx, mut world) = (Context::new(None), World::default());
        hot.load(&mut cx);
        hot.attach(&mut cx);
        hot.restored(&mut cx);
        hot.tick(&mut world, Tick { number: 0 });
        let queued = hot.commands().map(|commands| commands.len());
        hot.update(&mut cx, Duration::ZERO);
        let handled = hot.event(&mut cx, &Event::CursorEntered);
        hot.detach(&mut cx);
        let held = hot.saved_world().expect("the world held aside");
        assert!(std::ptr::eq(held, &hot.0.held));
        assert_eq!((queued, handled), (Some(1), ControlFlow::Break(())));
        let calls = [
            "load", "attach", "restored", "tick", "commands", "update", "event", "detach",
        ];
        assert_eq!(hot.0.calls, calls);
    }

    /// Without a window, as in headless runs and tests, a file dialog shows nothing, so it picks
    /// nothing and pauses no frames.
    #[cfg(feature = "dev")]
    #[test]
    fn a_file_dialog_without_a_window_picks_nothing() {
        let mut cx = Context::seeded(0);
        let dialog = crate::FileDialog {
            title: "Save the scene as",
            save: true,
            filter: Some(("Scenes", &["ron"])),
            directory: Some(Path::new("scenes")),
            file_name: Some("grid.ron"),
        };
        assert_eq!(cx.pick_file(&dialog), None);
        assert!(!cx.paused);
    }
}
