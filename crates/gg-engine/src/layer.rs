//! The layer stack: how an app's code, and the engine's tools, plug into the main loop.

use std::any::type_name;
#[cfg(feature = "dev")]
use std::collections::HashMap;
use std::mem;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bytemuck::NoUninit;
#[cfg(feature = "dev")]
use gg_api::asset::AssetId;
use gg_core::math::{Mat4, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{
    Blend, Circle, Draws, Line, MAX_PICKS, Mesh, Meshes, Mipmaps, ModelParams, ModelVertex,
    Outline, Shader, Shaders, Sprite, SpriteSheet, Texture, Textures, Viewpoint, Viewport,
};
#[cfg(feature = "dev")]
use subsecond::HotFn;
use tracing::{error, info, warn};

#[cfg(feature = "dev")]
use crate::assets::Catalog;
use crate::assets::{self, Assets, Loaded};
use crate::camera::{self, Camera, Tie};
use crate::draw::{self, Poses, Scratch};
use crate::edits::{Authors, Done, Edit};
use crate::gltf;
use crate::grid::{self, Plane};
use crate::image::{self, Image};
use crate::interpolation::Past;
#[cfg(feature = "dev")]
use crate::net::Ticket;
use crate::net::{Lockstep, Outside, Turn};
use crate::perf::Perf;
use crate::physics::{self, CircleCollider2D, Physics2D};
use crate::recording::Recorder;
use crate::recording::text::Text;
use crate::reflect::{Reflect, Value};
use crate::render_thread::Extracted;
use crate::scene_files::{self, SceneFile, SceneFiles};
use crate::snapshot::{Run, Snapshot};
use crate::{
    App, BoxCollider2D, Entity, EntityId, Event, Input, MeshDraw, Model, Name, RigidBody2D,
    Transform, Ui, World,
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

    /// Called as a run restored from a snapshot starts, in place of [`attach`](Self::attach), once
    /// every layer has loaded and the world is back as it was; again as the tool API's `stop`
    /// brings back the world as edited, each layer's queue emptied first; as its `scene new` and
    /// `scene open` renew the world they changed, each layer's queue kept; and as a lockstep
    /// joiner's world gives way to its host's: the layer picks up from the world, such as the
    /// values its panels show, and queues what its ticks need, such as letting go of what was held
    /// as the world it picks up from was left. The world changes only at ticks from here.
    fn restored(&mut self, _cx: &mut Context) {}

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
    /// The size of the pixels the view shows in place of what's drawn, while it shows any; the
    /// bytes [`show_pixels`](Self::show_pixels) last wrote, which the next frame handed off takes
    /// in place of those it had; and whether they're new since a frame was.
    shown: Option<[u32; 2]>,
    shown_bytes: Vec<u8>,
    shown_new: bool,
    /// When the frame being made started, by the machine's clock, as the frames of a view
    /// published to a tool say.
    #[cfg(feature = "dev")]
    pub(crate) started: std::time::SystemTime,
    /// The look the tool API's clients set, which the frames draw with over what the layers set.
    #[cfg(feature = "dev")]
    pub(crate) look: crate::api::Look,
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
    /// The project's start scene, which [`load_start_scene`](Self::load_start_scene) loads, as
    /// [`App::project`] says.
    ///
    /// [`App::project`]: crate::App::project
    pub(crate) start: Option<PathBuf>,
    /// The project's scenes folders, which the tool API's `scene` opens and saves scene files in.
    #[cfg(feature = "dev")]
    pub(crate) folders: Vec<PathBuf>,
    /// The assets the layers added: the world's, which a world that a layer puts in its place
    /// takes again, as [`share_assets`](Self::share_assets) says.
    assets: Arc<Assets>,
    /// The textures, models and shaders the layers added, by the file each resolves to, which a
    /// second add of that file, by whatever path, gives again.
    loaded: Loaded,
    /// The names given the textures the layers add, by their paths, in place of the IDs the asset
    /// database holds, as [`name_as`](Self::name_as) says: `None` while the run names its textures
    /// by the database's.
    #[cfg(feature = "dev")]
    given: Option<HashMap<Arc<str>, Arc<str>>>,
    /// Whether the run goes as another run went, as a replay or a lockstep joiner does, whose texts
    /// name textures as that run did: a texture they name that can't be loaded then stands in, as
    /// [`load_named`](Self::load_named) says, rather than refusing what names it.
    pub(crate) follows: bool,
    /// How long frames take, which the main loop measures and
    /// [`PerfPanel`](crate::PerfPanel) shows.
    pub(crate) perf: Perf,
    /// Set once a layer has shown something that paused the frames while it was open, such as a
    /// file dialog, until the main loop takes it to restart its clock, so that no frame's time
    /// counts the pause.
    pub(crate) paused: bool,
    /// The edits of the authors outside the layers, the tool API's clients and the scene files a
    /// game loaded, each undoing its own.
    pub(crate) authors: Authors,
    /// The scene files the layers loaded, which development builds watch.
    pub(crate) scenes: SceneFiles,
    /// The world as edited, while a copy of it plays in its place, as [`Stack::play`] holds it.
    #[cfg(feature = "dev")]
    pub(crate) edited: Option<Edited>,
    /// The local player, in a lockstep session.
    player: Option<u8>,
}

/// The world as edited, which the tool API's `play` holds aside while a copy of it plays in its
/// place, and `stop` brings back: with the ticks it's at, its scene files as last applied, its open
/// scene and the authors' edits of it.
#[cfg(feature = "dev")]
pub(crate) struct Edited {
    pub(crate) world: World,
    pub(crate) ticks: u64,
    pub(crate) scenes: Vec<SceneFile>,
    pub(crate) open: scene_files::Open,
    pub(crate) authors: Authors,
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
        cx.start = app.manifest().map(|manifest| manifest.start.clone());
        #[cfg(feature = "dev")]
        if let Some(manifest) = app.manifest() {
            cx.folders.clone_from(&manifest.scenes);
        }
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
    /// [`App::root`](crate::App::root) does for a run, and, in a development build, by the IDs the
    /// ID files under it hold, which it reads, writing none, as a run that serves no tool API does:
    /// for a [`seeded`](Self::seeded) context, so that the world's hash, which reads its textures'
    /// names, doesn't depend on where the game's files are, and names them as the game's runs do.
    #[doc(hidden)]
    #[must_use]
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        #[cfg(feature = "dev")]
        self.scan_assets(false);
        self
    }

    /// Makes the context's game the project whose manifest is in `dir`, as
    /// [`App::project`](crate::App::project) does for a run, naming assets from its asset root, as
    /// [`root`](Self::root) does, and loading its start scene: for a [`seeded`](Self::seeded)
    /// context.
    ///
    /// Fails as the manifest's reading does in a run that `App::project` made a project's.
    #[doc(hidden)]
    pub fn project(self, dir: impl AsRef<Path>) -> Result<Self> {
        let manifest = crate::Manifest::read(dir.as_ref())?;
        let mut cx = self.root(manifest.root);
        cx.start = Some(manifest.start);
        #[cfg(feature = "dev")]
        {
            cx.folders = manifest.scenes;
        }
        Ok(cx)
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
            shown: None,
            shown_bytes: Vec::new(),
            shown_new: false,
            #[cfg(feature = "dev")]
            started: std::time::SystemTime::UNIX_EPOCH,
            #[cfg(feature = "dev")]
            look: crate::api::Look::default(),
            tickets: 0,
            picks: Vec::new(),
            unseen: Vec::new(),
            picked: Vec::new(),
            world,
            scratch: Scratch::default(),
            past: Past::default(),
            attached: false,
            root: None,
            start: None,
            #[cfg(feature = "dev")]
            folders: Vec::new(),
            assets,
            loaded: Loaded::default(),
            #[cfg(feature = "dev")]
            given: None,
            follows: false,
            perf: Perf::default(),
            paused: false,
            authors: Authors::default(),
            scenes: SceneFiles::default(),
            #[cfg(feature = "dev")]
            edited: None,
            player: None,
        }
    }

    /// The player this run plays, in a lockstep session, which the run hosts (`--host`), as player
    /// 0, or joins (`--join`), as player 1; `None` outside one. Each peer runs every player's world,
    /// and its layers queue commands for their own player alone, naming it in them, as the other
    /// peer's queue for theirs.
    pub fn player(&self) -> Option<u8> {
        self.player
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

    /// Has the game's view show `size` pixels in place of what's drawn, from the frame being made
    /// on, which `fill` writes, given their bytes: rows of RGBA8, sRGB-encoded and tightly packed,
    /// top row first. They show from the view's top left, each onto one of its pixels, as far as
    /// they reach of it, the rest of it the clear color, and go on showing in the frames after,
    /// until the next call or [`hide_pixels`](Self::hide_pixels): the view of another process's
    /// game, as the editor shows it. Nothing drawn shows in the view meanwhile, and picks find
    /// nothing there.
    ///
    /// # Panics
    ///
    /// If a side is 0 or more than [`MAX_TEXTURE_SIZE`](gg_renderer::MAX_TEXTURE_SIZE).
    pub fn show_pixels(&mut self, size: [u32; 2], fill: impl FnOnce(&mut [u8])) {
        let sides = 1..=gg_renderer::MAX_TEXTURE_SIZE;
        assert!(
            size.iter().all(|side| sides.contains(side)),
            "{size:?} pixels; each side is 1 to {} pixels",
            gg_renderer::MAX_TEXTURE_SIZE
        );
        let bytes = size[0] as usize * size[1] as usize * 4;
        self.shown_bytes.resize(bytes, 0);
        fill(&mut self.shown_bytes);
        self.shown = Some(size);
        self.shown_new = true;
    }

    /// Has the game's view show what's drawn again, from the frame being made on, after
    /// [`show_pixels`](Self::show_pixels).
    pub fn hide_pixels(&mut self) {
        self.shown = None;
        self.shown_new = false;
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
    /// reads them by what it shows; and so does a development build's frame that a tool's capture
    /// of the game's own view makes bare. Replays, which render nothing, answer none.
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
    /// A file added again, by whatever form of its path, with the same vertex and parameter types
    /// and blend, gives the shader it gave first, which compiles and reloads once.
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
        let path = path.as_ref();
        let named = assets::name_of(self.root.as_deref(), path);
        let file = assets::file_of(path);
        let added = &self.loaded.shaders;
        let same = |by: &Path| {
            file.as_ref()
                .is_some_and(|file| added.get(by) == Some(file))
        };
        let shader = match self.shaders.find::<V, P>(blend, same) {
            Some(shader) => shader,
            None => self.shaders.add(name, path, blend)?,
        };
        if let Some(file) = file {
            self.loaded.shaders.insert(path.to_owned(), file);
        }
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
    /// A file added again, by whatever form of its path, gives the texture it gave first, without
    /// loading it again, as ARCHITECTURE.md's Rendering says; `name` is then unused, and the
    /// texture keeps the path it was first added by, which another path names too. Scenes,
    /// snapshots, recordings and the tool API name it by its asset ID, where its file has one, as
    /// ARCHITECTURE.md's Project state says, and otherwise by that path.
    ///
    /// Fails with `engine.bad_texture` if the file can't be read, isn't a regular file, is more
    /// than 1 GiB, doesn't decode as a PNG or a JPEG, or has a side of more than 8,192 pixels, the
    /// most every Vulkan 1.4 device takes, and with `renderer.too_many_textures` past the 61,440 a
    /// renderer holds, the engine's white one among them.
    pub fn add_texture(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Texture> {
        let path = path.as_ref();
        let named = assets::name_of(self.root.as_deref(), path);
        let file = assets::file_of(path);
        let texture = match file
            .as_ref()
            .and_then(|file| self.loaded.textures.get(file))
        {
            Some(&(texture, _)) => {
                self.alias(texture, &named);
                texture
            }
            None => {
                let image = image::load_image(name, path)?;
                let size = image.size;
                let texture = self.textures.add(name, size, image.pixels, Mipmaps::None)?;
                let id = self.texture_name(&named, path, file.as_deref(), None);
                self.add_assets(|assets| assets.add_texture(texture, named.clone(), id));
                if let Some(file) = file {
                    self.loaded.textures.insert(file, (texture, size));
                }
                texture
            }
        };
        self.added("texture", &named);
        Ok(texture)
    }

    /// Loads the PNG or JPEG file at `path` as a sprite sheet: a texture, as
    /// [`add_texture`](Self::add_texture) loads one, cut into cells `cell` pixels across and down,
    /// packed edge to edge from its top left, which [`SpriteSheet::sprite`] draws. A file added
    /// before, as a texture or a sprite sheet, isn't loaded again: its texture is cut into these
    /// cells.
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
        let named = assets::name_of(self.root.as_deref(), path);
        let file = assets::file_of(path);
        let sheet = match file
            .as_ref()
            .and_then(|file| self.loaded.textures.get(file))
        {
            Some(&(texture, size)) => {
                fits(name, path, size, cell)?;
                self.alias(texture, &named);
                SpriteSheet {
                    texture,
                    size,
                    cell,
                }
            }
            None => {
                let image = image::load_image(name, path)?;
                let sheet = add_sheet(&mut self.textures, name, path, image, cell)?;
                let (texture, size) = (sheet.texture, sheet.size);
                let id = self.texture_name(&named, path, file.as_deref(), None);
                self.add_assets(|assets| assets.add_texture(texture, named.clone(), id));
                if let Some(file) = file {
                    self.loaded.textures.insert(file, (texture, size));
                }
                sheet
            }
        };
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
    /// than by the types the model declares. A file added again gives the model it gave first. Its
    /// images are named by the model's asset ID, where its file has one, or else by its path, and
    /// their places in it, such as `5f3c9a1e07b2d846#0` or `assets/truck.glb#0`.
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
        let named = assets::name_of(self.root.as_deref(), path);
        let file = assets::file_of(path);
        let model = match file.as_ref().and_then(|file| self.loaded.models.get(file)) {
            Some(model) => model.clone(),
            None => {
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
                let (meshes, shaders, textures) =
                    (&mut self.meshes, &mut self.shaders, &mut self.textures);
                let (model, images) = Model::add(name, imported, meshes, shaders, textures)?;
                // Each named by the model's ID, or path, and its order in the model.
                let images: Vec<_> = (images.into_iter().enumerate())
                    .map(|(i, image)| {
                        let at = Some(i);
                        let id = self.texture_name(&named, path, file.as_deref(), at);
                        (image, format!("{named}#{i}").into(), id)
                    })
                    .collect();
                self.add_assets(|assets| {
                    for (image, path, id) in images {
                        assets.add_texture(image, path, id);
                    }
                });
                if let Some(file) = file {
                    self.loaded.models.insert(file, model.clone());
                }
                model
            }
        };
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

    /// Has `path` name `texture`, which another path to its file added before, as [`Assets::alias`]
    /// does, unless it names a texture already.
    fn alias(&mut self, texture: Texture, path: &Arc<str>) {
        if self.assets.texture(path).is_none() {
            self.add_assets(|assets| assets.alias(texture, path.clone()));
        }
    }

    /// Loads, as [`add_texture`](Self::add_texture) loads a file, the texture that each of `names`
    /// names, as text names one, that none loaded has: where a scene, the tool API, or a recorded
    /// or a lockstep peer's command or edit names it, before it's read, as ARCHITECTURE.md's
    /// Project state says. A name finds its file as [`texture_path`](Self::texture_path) says,
    /// inside the asset root. In a run that follows another, the texture takes the name its text
    /// gave it, that run's, and its file is found by the names it was given first.
    ///
    /// One that can't be loaded stands in, if `stand_in`, as for a run that follows another, so
    /// that its texts read as that run's did: a texture of magenta and black squares, named as the
    /// name, warning `asset.stand_in` with why. Otherwise it stays unloaded, and the refusal of
    /// what names it says why, as [`Assets::not_found`] does.
    pub(crate) fn load_named<'n>(
        &mut self,
        names: impl IntoIterator<Item = &'n str>,
        stand_in: bool,
    ) {
        let mut tried: Vec<&str> = Vec::new();
        let mut failed = Vec::new();
        for name in names {
            if self.assets.texture(name).is_some() || tried.contains(&name) {
                continue;
            }
            tried.push(name);
            let (path, loaded) = match self.texture_path(name, stand_in) {
                Ok(path) => {
                    let loaded = self.load_path(name, &path);
                    (Some(path), loaded)
                }
                Err(error) => (None, Err(error)),
            };
            match loaded {
                Ok(()) => {}
                Err(error) if stand_in => self.stand_in(name, path.as_deref(), &error),
                Err(error) => failed.push((name, error)),
            }
        }
        if !tried.is_empty() {
            self.add_assets(|assets| assets.failed(&failed));
        }
    }

    /// Loads, as [`load_named`](Self::load_named) does with `stand_in`, each texture whose path from
    /// the asset root `paths` gives that none loaded has: as a run restored from a snapshot loads
    /// those its `asset` lines say its run had, before it reads the world, so that its values find
    /// every texture they name, and in the order that run loaded them, so that a texture's index,
    /// which what's opaque holds, is what it was where the layers added the same before. A path
    /// outside the root, as only a layer gives one, stands in, as one text names does.
    pub(crate) fn load_restored<'p>(&mut self, paths: impl IntoIterator<Item = &'p str>) {
        self.load_named(paths, true);
    }

    /// Loads each texture scene `text` names that none loaded has, as
    /// [`load_named`](Self::load_named) does, standing in for what can't be in a run that follows
    /// another; and returns the names of those still missing.
    ///
    /// Fails, loading nothing, as [`World::load_scene`] does but for those.
    pub(crate) fn load_scene_textures(&mut self, text: &str) -> Result<Vec<String>> {
        let names = self.world.scene_textures(text)?;
        self.load_named(names.iter().map(String::as_str), self.follows);
        let missing = names
            .into_iter()
            .filter(|name| self.assets.texture(name).is_none());
        Ok(missing.collect())
    }

    /// Loads each texture `text`, a value as a recording writes it, names that none loaded has,
    /// standing in for what can't be, as [`load_named`](Self::load_named) does: a value that a
    /// recording or a lockstep peer gives, which names textures as the run that wrote it did.
    pub(crate) fn load_followed(&mut self, text: &Text) {
        let mut names = Vec::new();
        text.textures(&mut names);
        if !names.is_empty() {
            self.load_named(names, true);
        }
    }

    /// The path from the asset root of the file whose texture text names `name`, as
    /// [`load_named`](Self::load_named) loads it: in a run that follows another, the path given
    /// that name, as [`name_as`](Self::name_as) gives them; in a development build, the asset's
    /// whose ID it is, as the asset database holds it; and else `name` itself, as a path. A run
    /// that follows another and isn't to stand in for what it can't load, as a lockstep joiner
    /// asked by the tool API, finds only what it was given, so that it names the texture as its
    /// host would.
    ///
    /// Fails with `asset.model_image` for a model's image, which loads with its model;
    /// `asset.not_given` for what such a run wasn't given; `asset.unknown_id` for an ID no asset
    /// has this run, or any ID in a build without `dev`, which reads no ID file; and
    /// `asset.outside_root` for an absolute path.
    fn texture_path(&self, name: &str, stand_in: bool) -> Result<Arc<str>> {
        if let Some((model, image)) = name.rsplit_once('#')
            && !image.is_empty()
            && image.bytes().all(|b| b.is_ascii_digit())
        {
            let message = format!(
                "`{name}` names image {image} of model `{model}`, which loads with its model, as a \
                 layer adds it with `Context::add_model`"
            );
            return Err(Error::new("asset.model_image", message));
        }
        #[cfg(feature = "dev")]
        if let Some(given) = &self.given {
            if let Some((path, _)) = given.iter().find(|(_, given)| ***given == *name) {
                return Ok(path.clone());
            }
            if self.follows && !stand_in && !given.contains_key(name) {
                let message = format!(
                    "`{name}` isn't a name or a path that the lockstep host has given a texture, \
                     and a joiner loads only those, so that both peers name it alike: name it \
                     through the host"
                );
                return Err(Error::new("asset.not_given", message));
            }
        }
        #[cfg(not(feature = "dev"))]
        let _ = stand_in;
        if assets::is_id(name) {
            #[cfg(feature = "dev")]
            {
                let id = name.parse::<AssetId>().ok();
                if let Some(path) = id.and_then(|id| self.assets.catalog().path(id)) {
                    return Ok(path.into());
                }
                let message = format!("no asset under the asset root has the ID {name} this run");
                return Err(Error::new("asset.unknown_id", message));
            }
            #[cfg(not(feature = "dev"))]
            {
                let message = format!(
                    "{name} is an asset ID, but a build without `dev` reads no ID file, so it finds \
                     a texture by its path alone"
                );
                return Err(Error::new("asset.unknown_id", message));
            }
        }
        let path = Path::new(name);
        let prefixed = matches!(
            path.components().next(),
            Some(std::path::Component::Prefix(_))
        );
        if path.has_root() || prefixed {
            let message = format!(
                "`{name}` is an absolute path, which would name a file on this machine alone: a \
                 texture is named by its ID or by its path from the asset root"
            );
            return Err(Error::new("asset.outside_root", message));
        }
        Ok(name.into())
    }

    /// Loads the texture at `path` from the asset root, which text names `name`, as
    /// [`add_texture`](Self::add_texture) loads a file, its name `name` too: a run that follows
    /// another names it so, as that run did.
    ///
    /// Fails with `asset.outside_root` for a file outside the root, as through `..` or a link, and
    /// as `add_texture` does.
    fn load_path(&mut self, name: &str, path: &str) -> Result<()> {
        // Before anything can fail, so that a stand-in is named so too.
        #[cfg(feature = "dev")]
        if let Some(given) = &mut self.given
            && name != path
            && !given.contains_key(path)
        {
            given.insert(path.into(), name.into());
        }
        let (file, within) = match &self.root {
            Some(root) => (root.join(path), root.as_path()),
            None => (PathBuf::from(path), Path::new(".")),
        };
        if let (Some(resolved), Some(within)) = (assets::file_of(&file), assets::file_of(within))
            && !resolved.starts_with(&within)
        {
            let message = format!(
                "`{path}` is outside the asset root, {}: a texture is named by its ID or by its \
                 path from the asset root",
                within.display()
            );
            return Err(Error::new("asset.outside_root", message));
        }
        let texture = self.add_texture(path, &file)?;
        self.alias(texture, &name.into());
        Ok(())
    }

    /// Has a stand-in, a texture of magenta and black squares, take the place of the texture text
    /// names `name`, of the file at `path` from the asset root, if its name led to one, which
    /// couldn't be loaded, as `why` says, warning `asset.stand_in`: named as the file would have
    /// been, or else as `name`.
    fn stand_in(&mut self, name: &str, path: Option<&str>, why: &Error) {
        // 8 pixels across and down, in squares of 4.
        let pixels: Vec<u8> = (0..64)
            .flat_map(|i| match (i % 8 / 4 + i / 8 / 4) % 2 {
                0 => [255, 0, 255, 255],
                _ => [0, 0, 0, 255],
            })
            .collect();
        let texture = match self.textures.add(name, [8, 8], pixels, Mipmaps::None) {
            Ok(texture) => texture,
            Err(error) => {
                warn!(
                    code = error.code(),
                    name, "Can't stand in for texture `{name}`: {error:#}"
                );
                return;
            }
        };
        let (path, named): (Arc<str>, Arc<str>) = match path {
            Some(path) => {
                let file = self
                    .root
                    .as_deref()
                    .map_or(PathBuf::from(path), |root| root.join(path));
                (path.into(), self.texture_name(path, &file, None, None))
            }
            None => (name.into(), name.into()),
        };
        self.add_assets(|assets| assets.add_texture(texture, path.clone(), named));
        self.alias(texture, &name.into());
        self.added("texture", &path);
        warn!(
            code = "asset.stand_in",
            name,
            path = &*path,
            cause = why.code(),
            "Texture `{name}` can't be loaded, so a stand-in of magenta and black squares takes \
             its place, as the run follows another, which names it: {why:#}"
        );
    }

    /// What names the texture loaded from `file`, which a layer gave as `path`, whose path among
    /// assets is `named`, as [`Assets`] says, or with `image`, its image of that place in the
    /// model: in a development build, the name given its path, if the run is given its textures'
    /// names, as [`name_as`](Self::name_as) says, and else its file's ID in the asset database,
    /// with an image's place after the model's; and otherwise its path, as a build without `dev`,
    /// which reads no ID file, names every one.
    fn texture_name(
        &self,
        named: &str,
        path: &Path,
        file: Option<&Path>,
        image: Option<usize>,
    ) -> Arc<str> {
        let at: Arc<str> = match image {
            Some(i) => format!("{named}#{i}").into(),
            None => named.into(),
        };
        #[cfg(feature = "dev")]
        {
            let given = match &self.given {
                Some(given) => given.get(&at).cloned(),
                None => self.asset_id(path, file).map(|id| match image {
                    Some(i) => format!("{id}#{i}").into(),
                    None => id.to_string().into(),
                }),
            };
            if let Some(name) = given {
                return name;
            }
        }
        #[cfg(not(feature = "dev"))]
        let _ = (path, file);
        at
    }

    /// The ID the asset database holds for `file`, which a layer gave as `path`: by that path
    /// from the root, if it's given inside it, as layers spell a file's path as its disk does, or
    /// else by the path from the root of the file the OS resolves it to, as through `..`, a link
    /// or, where names ignore it, case.
    #[cfg(feature = "dev")]
    fn asset_id(&self, path: &Path, file: Option<&Path>) -> Option<AssetId> {
        let root = self.root.as_deref()?;
        let catalog = self.assets.catalog();
        let spelled = path.strip_prefix(root).ok();
        let spelled = spelled.and_then(|within| catalog.id(&assets::name_of(None, within)));
        spelled.or_else(|| {
            let within = file?.strip_prefix(assets::file_of(root)?).ok()?;
            catalog.id(&assets::name_of(None, within))
        })
    }

    /// Has the run name the textures its layers add as `names`, each a name and the path it names,
    /// names their paths, and every other by its path, in place of the IDs the asset database
    /// holds, renaming those added already: as a replay names them as its recording's run did, and
    /// a lockstep session's joiner as its host does, whose ID files its own disk may not hold.
    #[cfg(feature = "dev")]
    pub(crate) fn name_as(&mut self, names: impl IntoIterator<Item = (Arc<str>, Arc<str>)>) {
        let given = names.into_iter().map(|(name, path)| (path, name)).collect();
        self.rename(given);
    }

    /// As [`name_as`](Self::name_as), but with `names` beside the names given already: as a
    /// lockstep host names the textures it adds after its joiner has joined.
    #[cfg(feature = "dev")]
    pub(crate) fn name_also(&mut self, names: impl IntoIterator<Item = (Arc<str>, Arc<str>)>) {
        let mut given = self.given.take().unwrap_or_default();
        given.extend(names.into_iter().map(|(name, path)| (path, name)));
        self.rename(given);
    }

    /// Names the textures as `given` names their paths, and every other by its path, from now on.
    #[cfg(feature = "dev")]
    fn rename(&mut self, given: HashMap<Arc<str>, Arc<str>>) {
        self.add_assets(|assets| assets.rename(&given));
        self.given = Some(given);
    }

    /// Has `add` change the assets the layers added, which the world, and the world as edited while
    /// a copy plays, share from then on: the context's, so that a world that took the world's place
    /// doesn't lose them, and copied only where a copy of the world shares them.
    fn add_assets(&mut self, add: impl FnOnce(&mut Assets)) {
        // The worlds let go of their shares for the change.
        self.world.share_assets(&Arc::default());
        #[cfg(feature = "dev")]
        if let Some(edited) = &mut self.edited {
            edited.world.share_assets(&Arc::default());
        }
        add(Arc::make_mut(&mut self.assets));
        self.world.share_assets(&self.assets);
        #[cfg(feature = "dev")]
        if let Some(edited) = &mut self.edited {
            edited.world.share_assets(&self.assets);
        }
    }

    /// Has the world share the assets the layers added again, if a layer put another world in its
    /// place, as it loaded, attached or ticked, as [`World::share_assets`] says.
    pub(crate) fn share_assets(&mut self) {
        self.world.share_assets(&self.assets);
    }

    /// Scans the assets under the app's root, as [`assets::scan`] says, writing the ID files
    /// missing if `writes`, and warning of each missing otherwise, and has the worlds share what it
    /// found: as a run starts, before the layers load, as a test's context is given its root, and
    /// as the tool API lists them. A context without a root has none. A texture is named by the ID
    /// the database holds for it as it's added, so that a scan after names none afresh.
    ///
    /// Each scan walks the whole root and reads every ID file on the main thread, O(files), and
    /// clones the catalog, O(assets): about 70 ms for 1,000 assets on Windows, but a few
    /// milliseconds apiece for ID files just copied in, the first time they're read; and each ID
    /// file it writes is flushed to the disk, about 6 ms apiece. So an `assets` after copying in
    /// about 1,500 assets without ID files, or 3,000 with them, stops frames past `gg run`'s 10 s
    /// `--hang`, and `gg run` restarts the game from its last snapshot, losing play since, while
    /// the restart's own scan, within the 120 s a run has to start, writes the rest. Phase 7's
    /// fourth milestone, renames that keep IDs, watches the root on a thread of its own instead,
    /// leaving a run's start the only scan.
    #[cfg(feature = "dev")]
    pub(crate) fn scan_assets(&mut self, writes: bool) {
        let Some(root) = self.root.as_deref() else {
            return;
        };
        let _zone = gg_core::zone!();
        if let Some(catalog) = assets::scan(self.assets.catalog(), root, writes) {
            self.add_assets(|assets| assets.set_catalog(catalog));
        }
    }

    /// The asset database, as the last scan left it.
    #[cfg(feature = "dev")]
    pub(crate) fn catalog(&self) -> &Catalog {
        self.assets.catalog()
    }

    /// Whether a layer has added the file at `path` from the app's root, by whatever form of its
    /// path, as a texture, a sprite sheet or a model: resolving the file, some 0.1 ms on Windows.
    #[cfg(feature = "dev")]
    pub(crate) fn has_added(&self, path: &str) -> bool {
        let file = (self.root.as_deref()).and_then(|root| assets::file_of(&root.join(path)));
        file.is_some_and(|file| {
            self.loaded.textures.contains_key(&file) || self.loaded.models.contains_key(&file)
        })
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

    /// Draws a grid this frame as `viewpoint` sees it, fitted to the game's view as the last frame
    /// handed it off, which [`view`](Self::view) gives: lines a power of ten apart on the world's
    /// axis plane `plane`, or on the one an orthographic viewpoint along an axis faces, as
    /// [`grid::shown`] says, and the plane's two axes in their colors, [`grid::RED`],
    /// [`grid::GREEN`] and [`grid::BLUE`], as the editor's view shows them. The lines show once
    /// they're 8 pixels apart, are crisp, a pixel each, through an orthographic viewpoint that's
    /// [`grid::square`], and fade toward grazing angles. It draws by straight alpha over the meshes
    /// drawn before it but where an opaque one is nearer, and is of no entity, so drawn before the
    /// world, which the engine draws after the layers' draws, it leaves picks to what's on top.
    /// `viewpoint` is the one the frame sees from, as [`set_viewpoint`](Self::set_viewpoint) sets
    /// it. Nothing draws while the view has no area, as in a dockspace's first frame, or while the
    /// plane is too near edge-on to show.
    pub fn draw_grid(&mut self, viewpoint: &Viewpoint, plane: Plane) {
        let Some(view) = self.view else {
            return;
        };
        if let Some((transform, grid)) = grid::quad(viewpoint, view, grid::shown(viewpoint, plane))
        {
            (self.draws).push_grid(&mut self.meshes, &mut self.shaders, transform, grid);
        }
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

    /// Loads the scene file at `path` into the world as the layer attaches, as [`World::load_scene`]
    /// loads the text [`read_scene`](crate::read_scene) reads, and returns its roots; first loading
    /// each texture it names, by its ID or its path from the asset root, that no layer has added,
    /// as [`add_texture`](Self::add_texture) loads a file. A development build watches the file
    /// from then on: saved while the game runs, it applies to the world as the operations that take
    /// the world's entities from what the file held to what it holds, between ticks, which keeps
    /// what the file doesn't change, as where the game has moved an entity; an edit whose author is
    /// the file's path, as the app's root names it, such as `scenes/level.ron`, which the tool
    /// API's `undo` takes back. A run restored from a snapshot watches the files its run had
    /// loaded. The project's start scene, as [`load_start_scene`](Self::load_start_scene) loads it,
    /// is the open scene too.
    ///
    /// Fails as `read_scene` does, and as `load_scene` does, the path beginning its message, as for
    /// a texture that can't be loaded, `asset.no_texture`, with why: a texture of a model, or
    /// named by an absolute path or one leading outside the asset root, by an ID no asset has, or
    /// by a path to a file that doesn't load as `add_texture` loads it. In a replay or a lockstep
    /// joiner, which goes as another run went, such a texture stands in, warning `asset.stand_in`.
    ///
    /// # Panics
    ///
    /// Outside the layers' attach and detach, as [`world_mut`](Self::world_mut) does.
    pub fn load_scene_file(&mut self, path: impl AsRef<Path>) -> Result<Vec<Entity>> {
        let path = path.as_ref();
        let text = crate::read_scene(path)?;
        let mut loaded = self.world_mut().load_scene(&text);
        // Read again once what it names that isn't loaded is, which is seldom, so that a scene that
        // names what's loaded is read once.
        if loaded
            .as_ref()
            .is_err_and(|error| error.code() == "asset.no_texture")
        {
            loaded =
                (self.load_scene_textures(&text)).and_then(|_| self.world_mut().load_scene(&text));
        }
        let roots = loaded.map_err(|error| {
            let message = format!("{}:{error}", path.display());
            error.with_message(message)
        })?;
        let author = assets::name_of(self.root.as_deref(), path).to_string();
        let start = self.start.as_deref();
        if start.is_some_and(|start| scene_files::same_file(start, path)) {
            self.scenes.open.author = Some(author.clone());
        }
        self.scenes
            .loaded(SceneFile::new(path.to_owned(), author, text));
        Ok(roots)
    }

    /// Loads the project's start scene as the layer attaches, as
    /// [`load_scene_file`](Self::load_scene_file) loads a scene file, and returns its roots: in a
    /// development build, watched, its edits' author its path from the asset root, such as
    /// `scenes/arena.ron`. It's the open scene, which the tool API's `scene` saves, empties and
    /// opens another scene file in place of, owning the entities its file holds and those clients'
    /// edits make into it, never those the game's code makes.
    ///
    /// Fails with `engine.no_project` for a game that isn't a project's, as
    /// [`App::project`](crate::App::project) makes it, and as `load_scene_file` does.
    ///
    /// # Panics
    ///
    /// Outside the layers' attach and detach, as [`world_mut`](Self::world_mut) does.
    pub fn load_start_scene(&mut self) -> Result<Vec<Entity>> {
        let Some(start) = self.start.clone() else {
            let message = "the game isn't a project's, which has a start scene: `App::project` \
                           makes it one";
            return Err(Error::new("engine.no_project", message));
        };
        self.load_scene_file(start)
    }

    /// Applies `edit`, an author's from outside the layers, to the world, between ticks, unrecorded,
    /// as a replay does: [`Stack::edit`] records it first.
    ///
    /// Fails as [`Authors::edit`] does.
    pub(crate) fn apply_edit(&mut self, edit: Edit) -> Result<Done> {
        #[cfg(feature = "dev")]
        self.scenes.changed();
        self.authors.edit(&mut self.world, edit)
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
    /// outlines, over the rest, while they're shown, or the look shows them; and finds where the
    /// world's camera sees from. Before the entities, a development build draws the look's grid,
    /// unless the frame is bare, as the viewpoint the frame sees from so far sees it.
    pub(crate) fn draw_world(&mut self, alpha: f32) {
        self.past.resolve(&self.world);
        self.seen = camera::seen(&self.world, &self.past, alpha, &mut self.tie);
        #[cfg(feature = "dev")]
        if let Some(plane) = self.look.grid.filter(|_| !self.look.bare) {
            let viewpoint = self.look.viewpoint.or(self.viewpoint).or(self.seen);
            self.draw_grid(&viewpoint.unwrap_or_default(), plane);
        }
        let (world, past) = (&self.world, &self.past);
        let (draws, shaders) = (&mut self.draws, &mut self.shaders);
        draw::draw_world(world, past, alpha, draws, shaders, &mut self.scratch);
        if self.colliders_drawn()
            && let Some(poses) = Poses::new(world, past, alpha)
        {
            let scale = self.scale() as f32;
            physics::draw_colliders(world, &poses, scale, &mut self.over, &mut self.shaders);
        }
    }

    /// Whether the frame draws the colliders' outlines: as the look shows them, where it says, in
    /// a frame that isn't bare, or else as the layers set.
    fn colliders_drawn(&self) -> bool {
        #[cfg(feature = "dev")]
        if !self.look.bare
            && let Some(shown) = self.look.colliders
        {
            return shown;
        }
        self.colliders
    }

    /// Fills in `frame` with this frame's render data, with the game's view at `viewport`, which
    /// [`view`](Self::view) gives from then on: its draws, those over the rest last, for the last
    /// frame's, which it forgets, keeping their memory for the next; the viewpoint a layer set this
    /// frame, which it forgets too, or else where the world's camera sees from, or else the
    /// default; what a layer outlined this frame, which it forgets too; the pixels the view shows,
    /// if it shows any, swapping new ones for those `frame` had; what's been added since a frame
    /// last took it, after what `frame` still holds; and the picks asked this frame, each at
    /// the pixel of the view under it, the floor of where it was asked less the view's origin, but
    /// those outside the view, which the next answers to come back answer `None`. In a development
    /// build, the look's viewpoint and outline go in place of the layers', and the frame carries
    /// the look's revision; but a bare frame leaves the look out, and the picks to the next frame
    /// that isn't bare, both those asked this frame and those the renderer holds, so that none
    /// reads a view a client isn't shown.
    pub(crate) fn extract(&mut self, frame: &mut Extracted, viewport: Viewport) {
        frame.clear_color = self.clear_color;
        frame.viewport = viewport;
        self.view = (viewport.size[0] > 0 && viewport.size[1] > 0).then_some(viewport);
        let (viewpoint, outline) = (self.viewpoint.take(), self.outline.take());
        #[cfg(feature = "dev")]
        let (viewpoint, outline) = if self.look.bare {
            (viewpoint, outline)
        } else {
            (
                self.look.viewpoint.or(viewpoint),
                self.look.outline.or(outline),
            )
        };
        frame.viewpoint = viewpoint.or(self.seen).unwrap_or_default();
        frame.outline = outline;
        #[cfg(feature = "dev")]
        {
            self.look.seen = Some(frame.viewpoint);
            frame.look = self.look.revision;
            frame.bare = self.look.bare;
        }
        frame.pixels = self.shown;
        if mem::take(&mut self.shown_new) {
            mem::swap(&mut frame.pixel_bytes, &mut self.shown_bytes);
            frame.pixels_new = true;
        }
        self.draws.append(&mut self.over);
        mem::swap(&mut frame.draws, &mut self.draws);
        self.draws.clear();
        (frame.uploads).take(&mut self.meshes, &mut self.shaders, &mut self.textures);
        frame.picks.clear();
        #[cfg(feature = "dev")]
        if self.look.bare {
            return;
        }
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
/// across and down, if they fit it, as [`fits`] checks; one that doesn't takes no texture.
fn add_sheet(
    textures: &mut Textures,
    name: &str,
    path: &Path,
    image: Image,
    cell: [u32; 2],
) -> Result<SpriteSheet> {
    fits(name, path, image.size, cell)?;
    let texture = textures.add(name, image.size, image.pixels, Mipmaps::None)?;
    Ok(SpriteSheet {
        texture,
        size: image.size,
        cell,
    })
}

/// Checks that cells `cell` pixels across and down fit sprite sheet `name`'s image, from `path`,
/// `size` pixels across and down.
///
/// Fails with `engine.bad_sprite_sheet` if a side of `cell` is 0 or more than the image's.
fn fits(name: &str, path: &Path, size: [u32; 2], cell: [u32; 2]) -> Result<()> {
    let [width, height] = size;
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
    Ok(())
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
    /// The lockstep session the run is a peer of, if it's networked.
    net: Option<Lockstep>,
}

/// What became of an edit from outside the layers, as [`Stack::offer`] says.
#[cfg(feature = "dev")]
pub(crate) enum Offered {
    /// It applied, and did this.
    Now(Result<Done>),
    /// It applies before a tick to come, and [`Stack::offered`] gives what it did with this ticket.
    Later(Ticket),
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

    /// The lockstep session the run is a peer of: for tests.
    #[cfg(test)]
    pub(crate) fn net_mut(&mut self) -> Option<&mut Lockstep> {
        self.net.as_mut()
    }

    /// The queue of the layer at `index`, from the bottom, if it queues commands: for tests.
    #[cfg(test)]
    pub(crate) fn commands_of(&mut self, index: usize) -> Option<&mut dyn Commands> {
        self.layers.get_mut(index)?.commands()
    }

    /// Has the run be a peer of `net`'s lockstep session.
    pub(crate) fn lockstep(&mut self, net: Lockstep) {
        self.net = Some(net);
    }

    /// Whether the run waits on its lockstep peer: a joiner without the host's world, or a tick
    /// without the other peer's inputs, `next` being the next to run.
    pub(crate) fn net_waits(&self, next: u64) -> bool {
        self.net.as_ref().is_some_and(|net| !net.ready(next))
    }

    /// Between ticks, as a frame starts, the next tick to run being `next`: takes what the lockstep
    /// peer sent since the last turn, sending the joiner the world, if the host owes it, or
    /// restoring the host's in place of the joiner's, as [`Lockstep::turn`] says, the joiner's
    /// textures taking the names the host's gave them, as they do the names the host gave those it
    /// added since. Returns the tick the joiner's world is at once it restores the host's, which
    /// its ticks go on from.
    pub(crate) fn net_turn(&mut self, cx: &mut Context, next: u64) -> Option<u64> {
        let mut net = self.net.take()?;
        let mut restored = None;
        match net.turn() {
            Turn::None => {}
            Turn::Send => {
                let text = crate::snapshot::write(
                    &cx.world,
                    &cx.scenes.files,
                    &cx.scenes.open,
                    &Run::Playing,
                    net.app(),
                    &self.names,
                    next,
                );
                match text {
                    Ok(text) => net.send_world(&text, cx.world.assets().count(), next),
                    Err(error) => {
                        error!(
                            code = error.code(),
                            "Can't send the joiner the world: {error:#}"
                        );
                    }
                }
            }
            Turn::Restore((tick, text)) => {
                let from = format!("the lockstep host's world at tick {tick}");
                let snapshot = Snapshot::parse(text, from, net.app());
                let ticks = snapshot.and_then(|snapshot| {
                    // Its textures go by the host's names, whatever ID files this peer's disk holds.
                    #[cfg(feature = "dev")]
                    cx.name_as(
                        (snapshot.names.iter()).map(|(path, name)| (name.clone(), path.clone())),
                    );
                    cx.load_restored(snapshot.textures());
                    self.restore_in_place(cx, &snapshot)
                });
                match ticks {
                    Ok(ticks) => {
                        net.restored(ticks);
                        restored = Some(ticks);
                    }
                    Err(error) => {
                        error!(code = error.code(), "{error:#}");
                        net.lose("its world can't be restored");
                    }
                }
            }
        }
        #[cfg(feature = "dev")]
        {
            let names = net.take_names();
            if !names.is_empty() {
                cx.name_also(names);
            }
        }
        self.net = Some(net);
        restored
    }

    /// Whether the run hosts or plays alone, rather than joining a lockstep session, whose host
    /// alone watches scene files.
    #[cfg(feature = "dev")]
    pub(crate) fn watches_scenes(&self) -> bool {
        !self.joins()
    }

    /// Whether the run joins a lockstep session, going as its host goes.
    fn joins(&self) -> bool {
        self.net.as_ref().is_some_and(|net| !net.hosts())
    }

    /// Whether the run is a peer of a lockstep session, hosting or joining, whose scene files the
    /// other peer's must stay as.
    #[cfg(feature = "dev")]
    pub(crate) fn is_peer(&self) -> bool {
        self.net.is_some()
    }

    /// Applies `edit`, an author's from outside the layers, between ticks, as [`edit`](Self::edit)
    /// does; or, in a lockstep session, offers it for the tick both peers apply it before, giving
    /// its ticket.
    #[cfg(feature = "dev")]
    pub(crate) fn offer(&mut self, cx: &mut Context, edit: Edit) -> Offered {
        match &mut self.net {
            Some(net) => Offered::Later(net.offer(edit)),
            None => {
                let apply = |cx: &mut Context, edit| self.edit(cx, edit);
                Offered::Now(scene_files::client_edit(cx, edit, apply))
            }
        }
    }

    /// Applies the save of the scene file whose author is `author`, its text now `text`, between
    /// ticks, as [`scene_files::apply_save`] does; or, in a lockstep session, offers it for the
    /// tick both peers apply it before. Either way it logs what it did.
    #[cfg(feature = "dev")]
    pub(crate) fn offer_save(&mut self, cx: &mut Context, author: String, text: String) {
        match &mut self.net {
            Some(net) => {
                // What it names that isn't loaded is loaded now, so that the names the host gives
                // those go out before it. One naming a texture that can't be, which the joiner
                // might stand in for, isn't offered, warning as a save that doesn't load does.
                if let Ok(missing) = cx.load_scene_textures(&text)
                    && !missing.is_empty()
                    && let Err(error) = cx.world().scene_loads(&text)
                {
                    warn!(
                        code = error.code(),
                        path = author,
                        "{author}: {error:#}; the world stays as it was, and the file's next save \
                         is compared with it as last applied"
                    );
                    return;
                }
                net.offer_save(author, text);
            }
            None => {
                let edit = &mut |cx: &mut Context, edit| self.edit(cx, edit);
                let _ = crate::scene_files::apply_save(cx, edit, &author, text);
            }
        }
    }

    /// Offers the patch `table` names, as [`Lockstep::offer_patch`] does, the next tick to run
    /// being `next`; `None` outside a lockstep session, where it applies now.
    #[cfg(feature = "dev")]
    pub(crate) fn offer_patch(
        &mut self,
        table: subsecond::JumpTable,
        next: u64,
    ) -> std::result::Result<Ticket, subsecond::JumpTable> {
        match &mut self.net {
            Some(net) => Ok(net.offer_patch(table, next)),
            None => Err(table),
        }
    }

    /// What the edit or the patch offered with `ticket` did, and the tick it applied before, once
    /// it has, which it gives once.
    #[cfg(feature = "dev")]
    pub(crate) fn offered(&mut self, ticket: Ticket) -> Option<(u64, Result<Done>)> {
        self.net.as_mut()?.done(ticket)
    }

    /// Whether the edit or the patch offered with `ticket` has applied, or been refused.
    #[cfg(feature = "dev")]
    pub(crate) fn offered_is_done(&self, ticket: Ticket) -> bool {
        self.net.as_ref().is_some_and(|net| net.is_done(ticket))
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
        cx.player = self.net.as_ref().map(Lockstep::player);
        cx.follows |= self.joins();
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
    /// simulation had run as it was taken, which the run's go on from, and whether its run was
    /// stopped, as the run then is. A development build's run that a tool API `serves`, and that
    /// can stop, as [`refuses_stop`](Self::refuses_stop) says, goes on as the snapshot's run was:
    /// one taken while a copy played holds the world as edited too, which comes back held aside, as
    /// [`play`](Self::play) holds it, the authors' edits afresh, as after any restore. Any other
    /// run plays on with the world that played, warning `snapshot.plays`, with why, of a snapshot
    /// whose run was stopped or held a world as edited.
    ///
    /// Fails as [`Snapshot::restore`] does.
    pub(crate) fn restore(
        &mut self,
        cx: &mut Context,
        snapshot: &Snapshot,
        serves: bool,
    ) -> Result<(u64, bool)> {
        let _zone = gg_core::zone!();
        cx.player = self.net.as_ref().map(Lockstep::player);
        cx.follows |= self.joins();
        self.load(cx);
        cx.attached = true;
        cx.load_restored(snapshot.textures());
        #[cfg(feature = "dev")]
        if serves && self.refuses_stop().is_none() {
            // As the layers loaded it, with their types, assets and resources, for the world as
            // edited to load into.
            let loaded = snapshot.held.as_ref().map(|held| (held, cx.world.clone()));
            let ticks = self.restore_in_place(cx, snapshot)?;
            if let Some((held, mut world)) = loaded {
                snapshot.restore_held(&mut world)?;
                world.share_assets(&cx.assets);
                let start = cx.start.as_deref();
                let open =
                    (held.open.clone()).unwrap_or_else(|| scene_files::opened(start, &held.scenes));
                cx.edited = Some(Edited {
                    world,
                    ticks: held.ticks,
                    scenes: held.scenes.clone(),
                    open,
                    authors: Authors::default(),
                });
            }
            return Ok((ticks, snapshot.stopped));
        }
        let _ = serves;
        let ticks = self.restore_in_place(cx, snapshot)?;
        if snapshot.stopped || snapshot.held.is_some() {
            #[cfg(feature = "dev")]
            let why = self
                .refuses_stop()
                .unwrap_or("the run serves no tool API, whose `play` alone plays a stopped run");
            #[cfg(not(feature = "dev"))]
            let why = "the build serves no tool API, whose `play` alone plays a stopped run";
            warn!(
                code = "snapshot.plays",
                path = %snapshot.from,
                why,
                "The snapshot's run was stopped, or held a world as edited, but this run plays \
                 from start to end: it goes on with the world that played"
            );
        }
        Ok((ticks, false))
    }

    /// Whether the tool API's `stop` can't stop the run, and if so, why: a recorded run's replay
    /// couldn't follow a world brought back between ticks, its ticks taken back, and a lockstep
    /// peer's world would part from the other's. Such a run plays from start to end.
    #[cfg(feature = "dev")]
    pub(crate) fn refuses_stop(&self) -> Option<&'static str> {
        if self.recorder.is_some() {
            Some(
                "the run is recorded, and its replay couldn't follow a world brought back between \
                 ticks, its ticks taken back",
            )
        } else if self.net.is_some() {
            Some("the run is a lockstep peer, whose world would part from the other peer's")
        } else {
            None
        }
    }

    /// Has a copy of `cx`'s world, the world as edited, which is at `ticks`, play in its place, as
    /// the tool API's `play` asks of a stopped run: holds the world as edited aside, with its
    /// ticks, its scene files, its open scene and the authors' edits of it, whose logs the copy
    /// starts afresh, until [`stop`](Self::stop); the copy's open scene starts as a copy of it.
    /// The layers are told nothing: what they hold of the world holds of the copy, which goes on
    /// exactly as the world would.
    #[cfg(feature = "dev")]
    pub(crate) fn play(&self, cx: &mut Context, ticks: u64) {
        let _zone = gg_core::zone!();
        let copy = cx.world.clone();
        let world = mem::replace(&mut cx.world, copy);
        let entities = world.entities(None).map_or(0, |entities| entities.len());
        cx.edited = Some(Edited {
            world,
            ticks,
            scenes: cx.scenes.files.clone(),
            open: cx.scenes.open.clone(),
            authors: mem::take(&mut cx.authors),
        });
        info!(ticks, entities, "Playing a copy of the world as edited");
    }

    /// Brings back the world as edited that [`play`](Self::play) holds aside, if it holds one, as
    /// the tool API's `stop` asks: the very world, with its scene files, its open scene and the
    /// authors' edits of it, the copy and all done to it going; empties each layer's queue, which
    /// the copy's ticks were to apply; has each layer pick up from the world, from the bottom up,
    /// as [`Layer::restored`] says; and returns the ticks the world as edited is at. A run playing
    /// from its start holds none: `None`, and nothing changes.
    #[cfg(feature = "dev")]
    pub(crate) fn stop(&mut self, cx: &mut Context) -> Option<u64> {
        let edited = cx.edited.take()?;
        let _zone = gg_core::zone!();
        cx.world = edited.world;
        cx.share_assets();
        cx.authors = edited.authors;
        cx.scenes.files = edited.scenes;
        cx.scenes.open = edited.open;
        for layer in &mut self.layers {
            if let Some(queue) = layer.commands() {
                queue.clear();
            }
        }
        for layer in &mut self.layers {
            layer.restored(cx);
        }
        info!(ticks = edited.ticks, "Stopped: the world as edited is back");
        Some(edited.ticks)
    }

    /// Has `apply` change `cx`'s world, between ticks, through a log of its own, which no author
    /// undoes, as the tool API's `scene open` and `scene new` change it; and then starts every
    /// author's edits afresh, since none of them could undo past it: the world is renewed, as
    /// [`World::renew`] says, refusing every log's entries from before, the authors' logs go, and
    /// each layer picks up from the world, from the bottom up, as at [`stop`](Self::stop), so that
    /// the in-process Inspector lets go of its own. Refused, what `apply` applied comes undone,
    /// and nothing else changes.
    ///
    /// Fails as `apply` does.
    #[cfg(feature = "dev")]
    pub(crate) fn rewrite(
        &mut self,
        cx: &mut Context,
        apply: impl FnOnce(&mut World, &mut crate::Log) -> Result<()>,
    ) -> Result<()> {
        let mut log = crate::Log::default();
        if let Err(error) = apply(&mut cx.world, &mut log) {
            // What applied before the refusal, which a refusal of the log's own undoes already.
            let _ = log.undo(&mut cx.world);
            log.clear(&mut cx.world);
            return Err(error);
        }
        // Dropped, as the renewed world lets go of what it kept.
        drop(log);
        cx.world.renew();
        cx.authors = Authors::default();
        cx.scenes.changed();
        for layer in &mut self.layers {
            layer.restored(cx);
        }
        Ok(())
    }

    /// Puts `snapshot`'s world in `cx`'s place, the layers having loaded, with its scene files and
    /// its open scene, or the start scene's file open for a snapshot that holds none, and has each
    /// layer pick up from it, from the bottom up; returns the ticks the simulation had run as it
    /// was taken.
    ///
    /// Fails as [`Snapshot::restore`] does.
    fn restore_in_place(&mut self, cx: &mut Context, snapshot: &Snapshot) -> Result<u64> {
        let ticks = snapshot.restore(&mut cx.world, &self.names)?;
        cx.scenes.files.clone_from(&snapshot.scenes);
        cx.scenes.open = (snapshot.open.clone())
            .unwrap_or_else(|| scene_files::opened(cx.start.as_deref(), &snapshot.scenes));
        #[cfg(feature = "dev")]
        cx.scenes.changed();
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

    /// Detaches from the top down, the reverse of attaching, once a recording has its end.
    pub(crate) fn detach(&mut self, cx: &mut Context) {
        let _zone = gg_core::zone!();
        if let Some(recorder) = &mut self.recorder {
            recorder.end();
        }
        cx.attached = false;
        cx.authors.clear(&mut cx.world);
        for layer in self.layers.iter_mut().rev() {
            layer.detach(cx);
        }
    }

    #[cfg(feature = "dev")]
    /// Applies `edit`, an author's from outside the layers, to `cx`'s world, between ticks, and says
    /// what it did. A recording gets it first, as it was asked for, before the commands of the tick
    /// after, so that a replay applies it there, and is refused where the run refused it.
    ///
    /// Fails as [`Authors::edit`] does.
    pub(crate) fn edit(&mut self, cx: &mut Context, edit: Edit) -> Result<Done> {
        if let Some(recorder) = &mut self.recorder {
            recorder.edited(&edit.to_value(), &cx.world);
        }
        cx.apply_edit(edit)
    }

    /// Runs a tick of the simulation: each layer's, from the bottom up, after which the world
    /// shares the context's assets again, as [`Context::share_assets`] says. A recording gets each
    /// layer's commands just before its tick applies them, so that a tick that panics leaves them,
    /// and the world's hash after the last. In a lockstep session, the tick waits for the other
    /// peer's inputs, returning `false` without running until they've come; once they have, this
    /// peer's queued commands go out as its inputs for a tick to come, and the tick runs on both
    /// peers' for it, as [`Lockstep::start_tick`] says, the edits among them applying first, as a
    /// recording keeps them.
    pub(crate) fn tick(&mut self, cx: &mut Context, tick: Tick) -> bool {
        let _zone = gg_core::zone!();
        let Self {
            layers,
            names,
            recorder,
            net,
            ..
        } = self;
        let merged = match net {
            Some(net) if !net.ready(tick.number) => return false,
            Some(net) => {
                let queues = layers.iter_mut().enumerate();
                let queues = queues.filter_map(|(index, layer)| Some((index, layer.commands()?)));
                let merged = net.start_tick(tick.number, queues, cx);
                for (ticket, input) in &merged.inputs {
                    let done = match input {
                        // A client's, on either peer.
                        Outside::Edit(edit) => {
                            let apply = |cx: &mut Context, edit: Edit| {
                                if let Some(recorder) = recorder.as_mut() {
                                    recorder.edited(&edit.to_value(), &cx.world);
                                }
                                cx.apply_edit(edit)
                            };
                            scene_files::client_edit(cx, edit.clone(), apply)
                        }
                        #[cfg(feature = "dev")]
                        Outside::Scene { author, text } => {
                            let mut edit = |cx: &mut Context, edit: Edit| {
                                if let Some(recorder) = recorder.as_mut() {
                                    recorder.edited(&edit.to_value(), &cx.world);
                                }
                                cx.apply_edit(edit)
                            };
                            crate::scene_files::apply_save(cx, &mut edit, author, text.clone())
                        }
                        // Only a development build's host watches scene files.
                        #[cfg(not(feature = "dev"))]
                        Outside::Scene { author, text } => Err(Error::new(
                            "net.bad_command",
                            format!(
                                "a peer sent {author}'s save, {} bytes, which only development \
                                 builds apply",
                                text.len()
                            ),
                        )),
                    };
                    if let Some(ticket) = ticket {
                        net.did(*ticket, tick.number, done);
                    }
                }
                #[cfg(feature = "dev")]
                for (ticket, table) in merged.patches.iter().cloned() {
                    let patched = crate::patch::apply(table, tick.number);
                    net.did(
                        ticket,
                        tick.number,
                        patched.map(|()| Done::Applied(Vec::new())),
                    );
                }
                Some(merged)
            }
            None => None,
        };
        for (index, layer) in layers.iter_mut().enumerate() {
            if let Some(merged) = &merged
                && let Some(queue) = layer.commands()
                && let Err(error) = merged.fill(index, queue, cx)
            {
                error!(code = error.code(), layer = names[index], "{error:#}");
            }
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
        if let Some(net) = net {
            net.end_tick(tick.number, &cx.world);
        }
        true
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
            cx.add_assets(|assets| assets.add_texture(texture, path.into(), path.into()));
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

    /// A texture's file added again, by another form of its path, gives the texture it gave first
    /// without loading it again, as a sprite sheet's does, whichever comes first, cut by the image's
    /// size; the texture keeps its first path's name, which the other path names too, and each add
    /// notes its own path, as a snapshot's asset lines count them.
    #[test]
    fn a_texture_added_twice_loads_once_however_its_path_is_written() {
        let scratch = crate::dirs::tests::Scratch::new("texture-once");
        let root = scratch.0.clone();
        std::fs::create_dir_all(root.join("assets")).expect("a directory");
        let png = image::tests::png(
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            4,
            &[9; 16],
            None,
        );
        std::fs::write(root.join("assets/tiles.png"), png).expect("a PNG");
        let plain = root.join("assets").join("tiles.png");
        // A path through `..`, which only resolving the file finds the same.
        let dotted = root.join("./assets/../assets/tiles.png");
        let mut cx = Context::seeded(0).root(root.clone());
        let texture = cx.add_texture("tiles", &plain).expect("a texture");
        let count = cx.textures.count();
        assert_eq!(cx.add_texture("again", &dotted).expect("it again"), texture);
        assert_eq!(cx.textures.count(), count, "loaded once");
        let sheet = cx
            .add_sprite_sheet("tiles", &dotted, [2, 1])
            .expect("a sheet");
        assert_eq!(
            (sheet.texture, sheet.size, sheet.cell),
            (texture, [4, 1], [2, 1])
        );
        let error = (cx.add_sprite_sheet("tiles", &plain, [8, 1])).expect_err("cells too wide");
        assert_eq!(error.code(), "engine.bad_sprite_sheet");
        assert_eq!(cx.textures.count(), count);
        let (first, other) = ("assets/tiles.png", "assets/../assets/tiles.png");
        assert_eq!(cx.assets.path(texture), Some(first));
        assert_eq!(cx.assets.texture(other), Some(texture));
        let note = |name: &str| format!("texture {} 1 {count} {name}", cx.meshes.count());
        let added: Vec<&str> = cx.assets.added().iter().map(|note| &**note).collect();
        assert_eq!(added, [note(first), note(other), note(other)]);

        let mut sheet_first = Context::seeded(0).root(root);
        let sheet = (sheet_first.add_sprite_sheet("tiles", &dotted, [2, 1])).expect("a sheet");
        assert_eq!(sheet.size, [4, 1]);
        let texture = sheet_first
            .add_texture("tiles", &plain)
            .expect("its texture");
        assert_eq!(texture, sheet.texture);
        assert_eq!(sheet_first.textures.count(), count);
    }

    /// A texture whose ID file holds an ID is named by it, as a test's context reads it from its
    /// root, writing none: found by it and by its path, however that's written; one without an ID
    /// file by its path; and a model's images by the model's ID and their places. So a world hashes
    /// alike once a texture's file is renamed with its ID file, but not without it.
    #[cfg(feature = "dev")]
    #[test]
    fn a_texture_is_named_by_its_id_so_its_file_renamed_with_it_hashes_alike() {
        use gg_api::asset::Meta;

        let scratch = crate::dirs::tests::Scratch::new("named-by-id");
        let root = scratch.0.clone();
        std::fs::create_dir_all(root.join("assets")).expect("a directory");
        let png = image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        let write = |name: &str, bytes: &[u8]| {
            std::fs::write(root.join("assets").join(name), bytes).expect("written");
        };
        let meta = |id: &str| {
            Meta {
                id: id.parse().expect("an ID"),
            }
            .text()
        };
        let (stone, model) = ("5f3c9a1e07b2d846", "1d2c3b4a59687f0e");
        write("stone.png", &png);
        write("stone.png.meta", meta(stone).as_bytes());
        write("plain.png", &png);
        write("m.glb", &gltf::tests::model());
        write("m.glb.meta", meta(model).as_bytes());
        let mut cx = Context::seeded(0).root(root.clone());
        assert!(!root.join("assets/plain.png.meta").exists(), "none written");
        let dotted = "assets/./stone.png";
        let textured = cx
            .add_texture("stone", root.join(dotted))
            .expect("a texture");
        let plain = cx.add_texture("plain", root.join("assets/plain.png"));
        let plain = plain.expect("a texture");
        cx.add_model("m", root.join("assets/m.glb"))
            .expect("a model");
        let assets = &cx.assets;
        assert_eq!(assets.name(textured), Some(stone));
        assert_eq!(assets.path(textured), Some(dotted));
        assert_eq!(
            [stone, dotted].map(|name| assets.texture(name)),
            [Some(textured); 2]
        );
        assert_eq!(assets.name(plain), Some("assets/plain.png"));
        let named: Vec<_> = assets.named_from(0).collect();
        let images = &named[1..];
        assert_eq!(named[0], (stone, dotted));
        assert!(!images.is_empty(), "{named:?}");
        for (i, &image) in images.iter().enumerate() {
            let (id, path) = (format!("{model}#{i}"), format!("assets/m.glb#{i}"));
            assert_eq!(image, (&*id, &*path));
        }
        // A file outside the root, given by a path from the working directory spelled as one in the
        // root is from it, has none of that one's ID.
        let stone_id = Some(stone.parse().expect("an ID"));
        assert_eq!(cx.asset_id(&root.join("assets/stone.png"), None), stone_id);
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let outside = assets::file_of(&manifest).expect("gg-engine's manifest");
        let spelled = Path::new("assets/stone.png");
        assert_eq!(cx.asset_id(spelled, Some(&outside)), None);

        let hash = |file: &str| {
            let mut cx = Context::seeded(0).root(root.clone());
            let texture = cx.add_texture("stone", root.join(file)).expect("a texture");
            let world = cx.world_mut();
            let sprite = Sprite {
                texture: Some(texture),
                ..Sprite::default()
            };
            world.spawn((Transform::IDENTITY, sprite));
            world.hash()
        };
        let before = hash("assets/stone.png");
        for name in ["stone.png", "stone.png.meta"] {
            let to = name.replacen("stone", "rock", 1);
            let assets = root.join("assets");
            std::fs::rename(assets.join(name), assets.join(to)).expect("renamed");
        }
        assert_eq!(hash("assets/rock.png"), before, "renamed with its ID file");
        std::fs::remove_file(root.join("assets/rock.png.meta")).expect("removed");
        assert_ne!(hash("assets/rock.png"), before, "by its path, without one");
    }

    /// A scene file loads the textures it names that no layer added, by an ID or by a path from the
    /// root, once each, named as a layer's add would name them. One naming a texture that can't be
    /// loaded is refused where it names it, saying why, spawning nothing: a file that isn't an
    /// image or isn't there, one outside the root, by an absolute path or through `..`, an ID no
    /// asset has, and a model's image. A run that follows another loads them, and stands in for
    /// those, named as named; and asked by the tool API, as a lockstep joiner, loads only what it
    /// was given.
    #[cfg(feature = "dev")]
    #[test]
    fn a_scene_loads_the_textures_it_names_and_refuses_those_it_cant() {
        use gg_api::asset::Meta;

        let logged = crate::testing::logged();
        let scratch = crate::dirs::tests::Scratch::new("loaded-where-named");
        let root = scratch.0.join("game");
        for dir in ["assets", "scenes"] {
            std::fs::create_dir_all(root.join(dir)).expect("a directory");
        }
        let png = image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        let meta = |id: &str| Meta {
            id: id.parse().expect("an ID"),
        };
        let (stone, model) = ("5f3c9a1e07b2d846", "1d2c3b4a59687f0e");
        let write = |name: &str, bytes: &[u8]| std::fs::write(root.join(name), bytes);
        write("assets/stone.png", &png).expect("written");
        write("assets/stone.png.meta", meta(stone).text().as_bytes()).expect("written");
        write("assets/plain.png", &png).expect("written");
        write("assets/bad.png", b"not a PNG").expect("written");
        write("assets/m.glb", &gltf::tests::model()).expect("written");
        write("assets/m.glb.meta", meta(model).text().as_bytes()).expect("written");
        std::fs::write(scratch.0.join("outside.png"), &png).expect("written");
        // A scene of a sprite of each texture named, their IDs from `first` on.
        let scene = |first: usize, names: &[&str]| -> PathBuf {
            let mut text = "(\n    entities: {\n".to_owned();
            for (i, name) in names.iter().enumerate() {
                text += &format!(
                    "        \"{:016x}\": (\n            Sprite: (\n                texture: \
                     Some(\"{name}\"),\n            ),\n        ),\n",
                    first + i
                );
            }
            text += "    },\n)\n";
            let path = root.join("scenes/s.ron");
            std::fs::write(&path, text).expect("written");
            path
        };
        let shown = |cx: &Context| -> Vec<String> {
            let sprites = cx.world().query::<&Sprite>();
            let names = sprites.filter_map(|sprite| cx.assets.name(sprite.texture?));
            names.map(str::to_owned).collect()
        };

        let mut cx = Context::seeded(0).root(root.clone());
        let count = cx.textures.count();
        let path = scene(0x1000, &[stone, "assets/plain.png", "assets/./stone.png"]);
        cx.load_scene_file(&path).expect("loaded");
        assert_eq!(cx.textures.count(), count + 2, "each once");
        let mut names = shown(&cx);
        names.sort_unstable();
        assert_eq!(names, [stone, stone, "assets/plain.png"]);

        let absolute = root
            .join("assets/plain.png")
            .to_string_lossy()
            .replace('\\', "/");
        let image = format!("{model}#0");
        let refused = [
            ("assets/bad.png", "engine.bad_texture"),
            ("assets/none.png", "engine.bad_texture"),
            ("../outside.png", "asset.outside_root"),
            (absolute.as_str(), "asset.outside_root"),
            ("0a1b2c3d4e5f6071", "asset.unknown_id"),
            (image.as_str(), "asset.model_image"),
        ];
        let entities = cx.world().entities(None).map_or(0, |e| e.len());
        let count = cx.textures.count();
        for &(name, why) in &refused {
            let error = cx
                .load_scene_file(scene(0x2000, &["assets/plain.png", name]))
                .expect_err(name);
            let message = format!("{error:#}");
            assert_eq!(error.code(), "asset.no_texture", "{message}");
            assert!(
                message.contains(why) && message.contains(".ron:"),
                "{message}"
            );
        }
        assert_eq!(cx.world().entities(None).map_or(0, |e| e.len()), entities);
        assert_eq!(cx.textures.count(), count, "nothing loaded");

        let mut following = Context::seeded(0).root(root.clone());
        following.name_as([]);
        following.follows = true;
        let _ = logged.codes.try_iter().count();
        let names: Vec<&str> = refused.iter().map(|&(name, _)| name).collect();
        following
            .load_scene_file(scene(0x1000, &names))
            .expect("stood in");
        let mut shown = shown(&following);
        shown.sort_unstable();
        let mut expected = names.clone();
        expected.sort_unstable();
        assert_eq!(shown, expected);
        let codes = logged
            .codes
            .try_iter()
            .filter(|code| code == "asset.stand_in");
        assert_eq!(codes.count(), refused.len());

        // Asked by the tool API, it loads what it was given alone.
        let given = [(Arc::from(stone), Arc::from("assets/stone.png"))];
        let mut joiner = Context::seeded(0).root(root);
        joiner.name_as(given);
        joiner.follows = true;
        joiner.load_named([stone, "assets/plain.png"], false);
        assert!(joiner.assets.texture(stone).is_some());
        let plain = joiner.assets.not_found("assets/plain.png");
        assert!(plain.contains("asset.not_given"), "{plain}");
    }

    /// A model's file added again gives the model it gave first, adding none of its meshes or
    /// images again; each add notes it.
    #[test]
    fn a_model_added_twice_loads_once() {
        let scratch = crate::dirs::tests::Scratch::new("model-once");
        std::fs::create_dir_all(&scratch.0).expect("a directory");
        let path = scratch.0.join("m.glb");
        std::fs::write(&path, gltf::tests::model()).expect("a model");
        let mut cx = Context::seeded(0);
        cx.add_model("m", &path).expect("a model");
        let counts = (cx.meshes.count(), cx.textures.count());
        assert!(counts.0 > 0 && counts.1 > 1, "{counts:?}");
        cx.add_model("again", &path).expect("it again");
        assert_eq!((cx.meshes.count(), cx.textures.count()), counts);
        let models = cx
            .assets
            .added()
            .iter()
            .filter(|note| note.starts_with("model "));
        assert_eq!(models.count(), 2);
    }

    /// A shader's file added again with the same vertex and parameter types and blend gives the
    /// shader it gave first, by another form of its path inside the root too, compiled once; with
    /// another blend or type of parameters, of the same size, it's another shader.
    #[test]
    fn a_shader_added_twice_loads_once_for_its_types_and_blend() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = root.join("tests/shaders/flat_color.slang");
        let dotted = root.join("tests/./shaders/flat_color.slang");
        let mut cx = Context::seeded(0).root(root);
        let added = |cx: &mut Context, path: &Path, blend| {
            let shader = cx.add_shader::<[f32; 3], Vec4>("flat", path, blend);
            format!("{:?}", shader.expect("the shader compiles"))
        };
        let flat = added(&mut cx, &path, Blend::Opaque);
        let count = cx.shaders.count();
        assert_eq!(added(&mut cx, &path, Blend::Opaque), flat);
        assert_eq!(added(&mut cx, &dotted, Blend::Opaque), flat);
        assert_eq!((cx.shaders.sources().len(), cx.shaders.count()), (1, count));
        assert_ne!(added(&mut cx, &path, Blend::Alpha), flat);
        assert_eq!(cx.shaders.sources().len(), 2);
        let array = cx.add_shader::<[f32; 3], [f32; 4]>("flat", &path, Blend::Opaque);
        array.expect("the shader compiles");
        assert_eq!(cx.shaders.sources().len(), 3);
        let shaders = cx
            .assets
            .added()
            .iter()
            .filter(|note| note.starts_with("shader "));
        assert_eq!(shaders.count(), 5);
    }

    /// Two files that a layer's paths name alike are two files, each loaded: here one outside the
    /// root, by a path relative to the working directory, gg-engine's, where cargo runs its tests,
    /// and one inside it.
    #[test]
    fn files_named_alike_load_apart() {
        let scratch = crate::dirs::tests::Scratch::new("named-alike");
        let shaders = scratch.0.join("tests").join("shaders");
        std::fs::create_dir_all(&shaders).expect("a directory");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/flat_color.slang");
        std::fs::copy(source, shaders.join("flat_color.slang")).expect("copied");
        let mut cx = Context::seeded(0).root(scratch.0.clone());
        let relative = Path::new("tests/shaders/flat_color.slang");
        for path in [relative, &shaders.join("flat_color.slang")] {
            let shader = cx.add_shader::<[f32; 3], Vec4>("flat", path, Blend::Opaque);
            shader.expect("the shader compiles");
        }
        assert_eq!(cx.shaders.sources().len(), 2);
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

    /// Records each call of its methods, and holds a queued command.
    #[cfg(feature = "dev")]
    #[derive(Default)]
    struct Keeper {
        calls: Vec<&'static str>,
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
        assert_eq!((queued, handled), (Some(1), ControlFlow::Break(())));
        let calls = [
            "load", "attach", "restored", "tick", "commands", "update", "event", "detach",
        ];
        assert_eq!(hot.0.calls, calls);
    }

    /// Holds a queue of commands, and counts the calls of its `restored`.
    #[cfg(feature = "dev")]
    struct Restorable {
        queued: Vec<u32>,
        restored: Rc<std::cell::Cell<u32>>,
    }

    #[cfg(feature = "dev")]
    impl Layer for Restorable {
        fn restored(&mut self, _: &mut Context) {
            self.restored.set(self.restored.get() + 1);
        }
        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.queued)
        }
    }

    /// `play` holds the world as edited aside, with its ticks, its scene files and the authors'
    /// edits of it, while a copy plays in its place, whose edits start afresh; `stop` brings back
    /// the very world, with them, empties each layer's queue, has each pick up from the world,
    /// once, and returns its ticks; a `stop` that finds none held changes nothing; and a recorded
    /// run refuses `stop`.
    #[cfg(feature = "dev")]
    #[test]
    fn stop_brings_back_the_world_as_edited_that_play_held() {
        let restored = Rc::default();
        let mut stack = Stack::default();
        stack.push_layer(Restorable {
            queued: Vec::new(),
            restored: Rc::clone(&restored),
        });
        let mut cx = Context::seeded(1);
        let hero = cx.world_mut().spawn(Transform::IDENTITY);
        let file = SceneFile::new(
            "scenes/a.ron".into(),
            "scenes/a.ron".to_owned(),
            "(entities: {})\n".to_owned(),
        );
        cx.scenes.loaded(file.clone());
        stack.attach(&mut cx);
        let id = cx.world().id(hero).expect("alive");
        let set = |x: f32| Edit::Apply {
            author: "test".to_owned(),
            operations: vec![crate::Operation::Set {
                entity: id,
                component: "Transform".into(),
                path: "translation.x".to_owned(),
                value: Value::F32(x),
            }],
        };
        let queued = |stack: &mut Stack| stack.commands_of(0).expect("a queue").len();
        cx.apply_edit(set(1.0)).expect("set");
        let queue = stack.commands_of(0).expect("a queue");
        queue.push(&Value::UInt(7)).expect("queued");
        assert_eq!(stack.stop(&mut cx), None, "nothing held");
        assert_eq!((queued(&mut stack), restored.get()), (1, 0));
        let (edited, hash) = (cx.world().serial(), cx.world().hash());
        stack.play(&mut cx, 40);
        let held = cx.edited.as_ref().expect("the world as edited, held");
        assert_eq!((held.world.serial(), held.ticks), (edited, 40));
        assert_eq!(held.scenes, std::slice::from_ref(&file));
        assert_ne!(cx.world().serial(), edited, "a copy plays");
        assert_eq!(cx.authors.logs(cx.world()).count(), 0);
        cx.apply_edit(set(2.0)).expect("set in the copy");
        cx.scenes.files.clear();
        assert_eq!(stack.stop(&mut cx), Some(40));
        assert!(cx.edited.is_none());
        assert_eq!((cx.world().serial(), cx.world().hash()), (edited, hash));
        assert_eq!(cx.scenes.files, [file]);
        let logs = cx.authors.logs(cx.world());
        let edits: Vec<usize> = logs.map(|(_, log)| log.edits().count()).collect();
        assert_eq!(edits, [1], "the edit made before `play` alone");
        assert_eq!((queued(&mut stack), restored.get()), (0, 1));
        assert_eq!(stack.refuses_stop(), None);
        let dir = crate::dirs::tests::Scratch::new("stop-recorded");
        std::fs::create_dir_all(&dir.0).expect("a directory");
        let recorder = Recorder::create(&dir.0.join("run.recording"), "Tested");
        stack.record(recorder.expect("created"));
        assert!(stack.refuses_stop().is_some());
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
