//! The layer stack: how an app's code, and the engine's tools, plug into the main loop.

mod adds;
#[cfg(feature = "dev")]
mod catalog;
mod cell_stream;
mod frame;
mod named;
mod ray;
#[cfg(feature = "dev")]
mod reload;
mod run;
mod sound;
mod stack;
#[cfg(feature = "dev")]
mod through;
mod window;
mod world;

#[cfg(all(test, feature = "dev"))]
pub(crate) use adds::model_types;
#[cfg(feature = "dev")]
pub(crate) use stack::Offered;
pub(crate) use stack::Stack;
pub use window::Window;

use std::any::type_name;
#[cfg(feature = "dev")]
use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gg_core::Result;
use gg_core::math::{DVec3, Vec4};
#[cfg(doc)]
use gg_renderer::MAX_PICKS;
use gg_renderer::{
    Antialiasing, Draws, Materials, Meshes, Scene, Shaders, Streaming, Textures, Viewpoint,
    Viewport,
};

#[cfg(doc)]
use crate::App;
use crate::args::Conditions;
#[cfg(feature = "dev")]
use crate::asset_watch::AssetWatch;
#[cfg(doc)]
use crate::assets;
use crate::assets::{Assets, Loaded};
use crate::audio::Audio;
use crate::camera::{self, Tie};
use crate::canvas;
use crate::cells::CellRun;
use crate::draw::Scratch;
use crate::edits::Authors;
use crate::environment::{self, Environment};
use crate::font::Fonts;
use crate::game_ui;
#[cfg(doc)]
use crate::game_ui::GameUi;
#[cfg(feature = "dev")]
use crate::hot_reload::HotReload;
use crate::interpolation::Past;
use crate::light;
use crate::loader::Loader;
use crate::net::{Build, Session, SessionStatus};
use crate::opaque::OpaqueMeshes;
use crate::package::Files;
use crate::perf::Perf;
use crate::placed::Placed;
use crate::reflect::{Reflect, Value};
use crate::scene_files::SceneFiles;
#[cfg(feature = "dev")]
use crate::scene_files::{self, SceneFile};
use crate::skins::Skins;
use crate::{Controls, Entity, EntityId, Event, Input, Ui, World};

/// A part of an app that the main loop drives.
///
/// | When | Called |
/// |---|---|
/// | The run starts, bottom up | [`load`], then [`attach`], or [`restored`] from a snapshot |
/// | Each tick, 64 a second | [`commands`] read, then [`tick`] |
/// | Each input event, top down | [`event`] |
/// | Each frame, after its ticks | [`update`], then [`ui`] |
/// | A SyncTest reruns a tick | [`swapped`] |
/// | The run ends, top down | [`detach`] |
/// | The tool API lists or presses actions | [`actions`] |
///
/// Every method defaults to nothing; keeping `ui`'s default also tells the engine the layer draws
/// no UI.
///
/// The world changes only as layers attach and detach, and at their ticks. Otherwise a layer reads
/// it and queues commands, data of its own, for its next tick to apply, so the simulation runs at
/// its own rate whatever the frame rate. A tick changes the world only through its commands and
/// systems, so a recording of the commands replays the run. A run restored from a snapshot
/// (`--restore`), as after a crash or rebuild, goes on where it was.
///
/// In a development build the engine calls each method through subsecond's `HotFn`. A patch from
/// `gg run`, applied between ticks, reaches the method and what it calls directly, such as the
/// systems and scripts its tick runs. What it calls through a trait object or the game's own
/// function pointer waits for the next rebuild.
///
/// [`load`]: Self::load
/// [`attach`]: Self::attach
/// [`restored`]: Self::restored
/// [`commands`]: Self::commands
/// [`tick`]: Self::tick
/// [`event`]: Self::event
/// [`update`]: Self::update
/// [`ui`]: Self::ui
/// [`swapped`]: Self::swapped
/// [`detach`]: Self::detach
/// [`actions`]: Self::actions
pub trait Layer {
    /// Called once at start, before any layer attaches, in a restored run too. Makes what the layer
    /// draws with (textures, meshes, shaders) and registers its world's component types, which a
    /// restored run needs as much as a fresh one. Spawns nothing, since a restored world comes from
    /// the snapshot: a layer that spawns here panics.
    ///
    /// An `Err`, for what the layer can't run without, ends the run before any layer attaches or
    /// is restored. The layers above don't load; the run logs the error by its code, last, and its
    /// status is 1. So:
    ///
    /// - a run fresh or from a snapshot, windowed, headless or a server, ends as it starts;
    /// - `gg run` reports it `crashed` and waits for a rebuild, as for a crash before a snapshot,
    ///   and so does the editor's game, which `gg run` runs embedded;
    /// - a replay (`--replay`) ends with it.
    fn load(&mut self, _cx: &mut Context) -> Result<()> {
        Ok(())
    }

    /// Called once at start, after every layer has loaded, before the first tick and update.
    /// Builds the world the simulation starts from.
    fn attach(&mut self, _cx: &mut Context) {}

    /// Called instead of [`attach`](Self::attach) when a restored run starts, once every layer has
    /// loaded and the world is back as it was. Also called when:
    ///
    /// - the tool API's `stop` brings back the world as edited, each layer's queue emptied first;
    /// - its `scene new` and `scene open` renew the world, each layer's queue kept;
    /// - a lockstep joiner's world gives way to its host's.
    ///
    /// The layer picks up from the world, such as the values its panels show, and queues what its
    /// ticks need, such as letting go of what was held. The world changes only at ticks from here.
    fn restored(&mut self, _cx: &mut Context) {}

    /// Called when a SyncTest (`--sync-test`) runs a tick again, as it does for each tick once run:
    /// as a copy of the world from the tick's start replaces the world, before the layers' ticks
    /// run again on it, each given back the commands it applied; and again as the world comes back.
    ///
    /// The layer moves what it keeps by a world's [serial](World::serial) to the world in place, as
    /// the Inspector moves its log, so its tick goes the same on the copy and on the world after.
    /// What it keeps by entity IDs holds in both. It queues nothing: the engine fills its queue.
    /// The default does nothing, for a layer that keeps nothing by a world's serial.
    fn swapped(&mut self, _cx: &mut Context) {}

    /// Called once as the app ends, after the last tick.
    fn detach(&mut self, _cx: &mut Context) {}

    /// Called at each tick, [64 a second](Tick::DT) whatever the frame rate, before the frame's
    /// updates. The layer applies the commands it queued since its last tick, in queue order, then
    /// runs its systems in order. It gets the world alone, so a tick goes by the world, the tick,
    /// its commands and what the layer set up as it attached, never by the frame's time, input or
    /// drawing.
    fn tick(&mut self, _world: &mut World, _tick: Tick) {}

    /// The commands queued since the layer's last tick, which its tick applies. A recording keeps
    /// them, read just before each tick, and a replay gives them back in place of what the layer
    /// queued. `None`, the default, for a layer that queues none. A layer whose tick goes by more
    /// than the world, the tick, its commands and what it set up as it attached (a frame's time,
    /// input, what its UI keeps) replays another way.
    fn commands(&mut self) -> Option<&mut dyn Commands> {
        None
    }

    /// The layer's controls, as [`Actions::controls`](crate::Actions::controls) gives them: its
    /// actions' bindings, what each does, and the controls it reads apart from them. The tool API's
    /// `actions` lists them, so an agent learns the game's controls without reading its code, and
    /// its `input` presses an action by name through them. None, the default.
    fn actions(&self) -> Controls {
        Controls::default()
    }

    /// Called every frame, after the frame's ticks, with the time since the last frame. The time is
    /// zero for the first frame and the first after frames pause (a minimized window, an open file
    /// dialog), so a pause isn't one long frame. For what's presented, such as the viewpoint,
    /// rather than the world, which changes at ticks.
    fn update(&mut self, _cx: &mut Context, _dt: Duration) {}

    /// Called for each event the layers above didn't handle. `Break` handles it, so the layers
    /// below don't see it. But a key or button release, a gamepad stick or trigger back at rest,
    /// and the loss of focus reach every layer, so none is left holding what it saw pressed. A
    /// stick or trigger a layer handles away from rest stays, for the layers below, as they last
    /// saw it until it rests.
    fn event(&mut self, _cx: &mut Context, _event: &Event<'_>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    /// Called every frame, after the updates, to draw UI: dockable panels and anything else egui
    /// draws. An app whose layers all keep this default, with OS-framed windows and no
    /// [view panel](crate::App::view_panel), runs without egui from its second frame, so its frames
    /// neither collect egui's input nor tessellate an empty UI. A build without `dev` never calls
    /// it: it compiles there and draws nothing, as [`Ui`] has no value there to call it with.
    fn ui(&mut self, _cx: &mut Context, ui: &mut Ui<'_>) {
        ui.default_ui(type_name::<Self>());
    }
}

/// A layer's type-erased queue of commands, from [`Layer::commands`].
///
/// The engine reads it each tick to record and fills it to replay, each command as a reflection
/// [`Value`]. A `Vec` of a [`Reflect`] type, such as an enum that derives it, is one.
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
    /// Fails like [`Reflect::from_value`] if it makes none.
    fn push(&mut self, value: &Value) -> Result<()>;

    /// Checks that [`push`](Self::push) would queue the command `value` makes, without queuing it.
    /// A recording checks each command it records, so it holds none its replay would refuse. The
    /// default checks nothing, for a queue whose `push` takes every value its
    /// [`value`](Self::value) gives, as the Inspector's does.
    ///
    /// Fails as `push` would.
    fn check(&self, value: &Value) -> Result<()> {
        let _ = value;
        Ok(())
    }

    /// Whether the commands are edits of the world, as the Inspector's are. A rollback session
    /// (`--rollback`) applies them only at a tick it has both peers' inputs for, like edits from
    /// outside the layers, so each applies once: what applies them logs them, and a tick run again
    /// would log again. The default, `false`, is a game's, whose ticks go by the world, the tick
    /// and their commands alone, and may run again.
    fn edits(&self) -> bool {
        false
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

    /// Fails, saying where, for a command that holds something opaque, which no value makes.
    fn check(&self, value: &Value) -> Result<()> {
        C::from_value(value).map(drop)
    }

    fn clear(&mut self) {
        Vec::clear(self);
    }
}

/// A step of the simulation, 64 a second whatever the frame rate: what [`Layer::tick`] gets.
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

/// A ticket [`Context::pick`] gives; its answer in [`Context::picked`] carries it back. A later
/// pick's is greater.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pick(u64);

/// What's still on its way to the GPU: [`Context::streaming`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Loading {
    /// The textures whose images are decoding off the main thread. They show stand-ins until
    /// decoded and streamed.
    pub decoding: u32,
    /// What the renderer was still streaming to the GPU after the last frame it handed back, and
    /// the bytes left to copy.
    pub streaming: Streaming,
}

impl Loading {
    /// Whether nothing is on its way: no image is decoding, and every texture and mesh added
    /// before the renderer's last handed-back frame was drawable in it. Later additions count once
    /// a frame has taken them, but images count as they decode.
    pub fn is_empty(&self) -> bool {
        self.decoding == 0 && self.streaming.is_empty()
    }
}

/// Where a frame's viewpoint came from: a layer, the tool API's look (each viewpoint it sets is a
/// jump), the world's primary camera by ID, or none. A frame whose source or projection kind
/// differs from the previous frame's is cut to, as [`gg_renderer::RenderData::cut`] says.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Seer {
    Layer,
    #[cfg(feature = "dev")]
    Look(Viewpoint),
    Camera(EntityId),
    Default,
}

impl fmt::Display for Seer {
    /// Names a frame's viewpoint in a diagnostic.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Layer => f.write_str("a layer's viewpoint"),
            #[cfg(feature = "dev")]
            Self::Look(_) => f.write_str("the look's viewpoint"),
            Self::Camera(id) => write!(f, "camera {id}"),
            Self::Default => f.write_str("the default viewpoint"),
        }
    }
}

/// Entities, with everything under each, to outline in the view in a color and width, as the
/// look's outline says. A frame turns it into the renderer's outline of IDs. Kept as the look
/// sets it, so frames don't rebuild it.
#[cfg(feature = "dev")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Outlining {
    pub(crate) ids: Vec<EntityId>,
    pub(crate) color: Vec4,
    pub(crate) width: f32,
}

/// What layers can see and change of the running app.
pub struct Context {
    window: Option<Window>,
    input: Input,
    cursor_grabbed: bool,
    max_fps: Option<NonZeroU32>,
    /// Whether a windowed run presents at the display's refresh rate, as `--vsync` or the title
    /// bar's settings' Match monitor set; the window hands it to the renderer each frame.
    vsync: bool,
    clear_color: [f32; 4],
    /// Set in place of the world's environment's, as
    /// [`set_antialiasing`](Self::set_antialiasing) says.
    antialiasing: Option<Antialiasing>,
    /// As [`set_render_scale`](Self::set_render_scale) says.
    render_scale: f32,
    /// Set by a layer for this frame, in place of the world's camera.
    viewpoint: Option<Viewpoint>,
    origin: DVec3,
    /// Where the world's camera saw from as the world was last drawn, and its ID.
    seen: Option<(EntityId, Viewpoint)>,
    /// Where the last handed-off frame's viewpoint came from, and whether it was perspective:
    /// together they tell whether the next frame is cut to.
    seer: Option<(Seer, bool)>,
    /// The tie among the world's cameras last warned of, while they've tied since.
    tie: Option<Tie>,
    /// The view's projection in the last frame of its source whose projection could project, and
    /// whether this streak of frames whose can't has warned: as [`camera::checked`] says.
    projection: camera::Projected<Seer>,
    /// Whether the fog's current streak of non-finite frames has warned.
    fog_warned: bool,
    /// The world's environment as the world was last drawn.
    environment: Option<Environment>,
    /// The tie among the world's environments last warned of, while they've tied since.
    environment_tie: Option<environment::Tie>,
    /// The world's lights as last drawn, which keep their memory.
    lit: light::Lit,
    /// The simulation's time in seconds as the world was last drawn.
    time: f32,
    meshes: Meshes,
    shaders: Shaders,
    textures: Textures,
    materials: Materials,
    /// Decodes the textures' images, off the main thread in a run's context.
    loader: Loader,
    /// What the renderer was streaming after the last frame it handed back.
    pub(crate) streamed: Streaming,
    fonts: Fonts,
    audio: Audio,
    /// What to draw this frame.
    draws: Draws,
    /// What to draw this frame over the rest, after the world: lines.
    over: Draws,
    /// What the layers drew on the canvas this frame, over everything.
    canvas: canvas::Drawn,
    uis: game_ui::Uis,
    /// Whether the world's colliders' outlines draw, over the rest.
    colliders: bool,
    /// Whether the world's navmesh and its agents' paths draw, over the rest.
    navmesh: bool,
    /// The color and width a layer outlines `outline_root` in this frame.
    outline: Option<(Vec4, f32)>,
    /// The entity a layer outlines: at most one, a list so frames don't allocate.
    outline_root: Vec<EntityId>,
    /// Scratch for walking the outlined subtrees, so frames don't allocate.
    outline_walk: Vec<Entity>,
    /// Where the game's view was in the last handed-off frame, while it had an area.
    view: Option<Viewport>,
    /// The viewpoint the last handed-off frame drew from, once one has.
    drawn: Option<Viewpoint>,
    /// The size of the pixels the view shows in place of what's drawn, while it shows any; the
    /// bytes [`show_pixels`](Self::show_pixels) last wrote, which the next handed-off frame takes;
    /// and whether they're new since a frame was.
    shown: Option<[u32; 2]>,
    shown_bytes: Vec<u8>,
    shown_new: bool,
    pub(crate) dev: Dev,
    /// The number of the next pick's ticket.
    tickets: u64,
    /// The picks asked this frame, at most [`MAX_PICKS`], each with where, in the main window's
    /// pixels.
    picks: Vec<(Pick, [f64; 2])>,
    /// Picks the last handed-off frame asked outside the view, which no frame reads: answered
    /// `None` with the next answers.
    unseen: Vec<Pick>,
    /// The answers that came back with the frame last taken back.
    picked: Vec<(Pick, Option<EntityId>)>,
    world: World,
    /// Sorted and computed while drawing the world, kept between frames.
    scratch: Scratch,
    /// Where the world's entities were as the latest tick started, which drawing draws them on
    /// from.
    past: Past,
    /// Where the world's entities are, kept between frames, and where those on their way draw this
    /// frame.
    placed: Placed,
    /// The world's change tick as the latest tick started: entities whose transforms changed at or
    /// after it draw on their way between ticks.
    tick_started: u64,
    /// What the renderer's scene holds of the world's opaque meshes, and what's changed since a
    /// frame last took it.
    opaque: OpaqueMeshes,
    /// The skinned entities' own meshes, which the scene draws them by, and their joints.
    skins: Skins,
    scene: Scene,
    /// Set from when the layers have attached until they detach: the world then changes only at
    /// ticks.
    attached: bool,
    /// What the added assets' files are named from: [`App::root`].
    ///
    /// [`App::root`]: crate::App::root
    pub(crate) root: Option<PathBuf>,
    /// Where the files under the root are read: the game's package, if the run reads one, as
    /// [`App::project`] says, or the disk.
    pub(crate) files: Files,
    /// The project's start scene, which [`load_start_scene`](Self::load_start_scene) loads, as
    /// [`App::project`] says.
    ///
    /// [`App::project`]: crate::App::project
    pub(crate) start: Option<PathBuf>,
    /// As [`read_state`](Self::read_state) says: a windowed run's alone.
    state: Option<PathBuf>,
    /// The assets the layers added: the world's, taken again by a world a layer puts in its
    /// place ([`share_assets`](Self::share_assets)).
    assets: Arc<Assets>,
    /// The assets the layers added, by the file each resolves to: a second add of that file, by
    /// whatever path, gives the same one.
    loaded: Loaded,
    /// Whether the run follows another's, as a replay or lockstep joiner does, whose texts name
    /// textures as that run did. A texture they name that can't load then stands in rather than
    /// refusing what names it: see [`load_named`](Self::load_named).
    pub(crate) follows: bool,
    /// Measured by the main loop, shown by [`PerfPanel`](crate::PerfPanel).
    pub(crate) perf: Perf,
    /// Set once a layer has shown something that paused the frames, such as a file dialog, until
    /// the main loop takes it to restart its clock, so no frame counts the pause.
    pub(crate) paused: bool,
    exiting: bool,
    failed: bool,
    /// When the engine started, until the run logs `engine.loaded`, as
    /// [`take_loaded`](Self::take_loaded) says: `None` after, and in a test's or replay's context,
    /// which logs none.
    pub(crate) load_started: Option<Instant>,
    /// The edits of the authors outside the layers, the tool API's clients and the scene files a
    /// game loaded, each undoing its own.
    pub(crate) authors: Authors,
    /// The scene files the layers loaded, which development builds watch.
    pub(crate) scenes: SceneFiles,
    pub(crate) hosts: crate::luau::Hosts,
    pub(crate) mods: crate::luau::RunMods,
    /// The local player, in a lockstep session.
    player: Option<u8>,
    /// Whether the run takes part in a networked session, as the layers attach or pick up from a
    /// restore, or as a session a layer asked for starts or ends.
    networked: bool,
    pub(crate) own: OwnSessions,
    /// A session [`host`](Self::host) or [`join`](Self::join) started, or the end
    /// [`leave`](Self::leave) asked for, which the stack takes up as the next frame starts.
    pub(crate) asked: Option<Asked>,
    pub(crate) session: Option<SessionStatus>,
    pub(crate) copied: Option<String>,
    /// Whether the run is a server's or one of its clients, as the layers attach or pick up from a
    /// restore. Their worlds name only the assets the layers loaded, so a client reads its
    /// server's states and a scene change loads none ([`change_scene`](Self::change_scene)).
    pub(crate) served: bool,
    /// Whether the run is a server's client, as the layers attach or pick up from a restore. Its
    /// world's cells come in with its server's states alone, as
    /// [`stream_cells`](Self::stream_cells) says.
    pub(crate) client: bool,
    scene_seen: u64,
    /// How many refused scene changes the world's ticks had asked for, as a tick last took them as
    /// final.
    refused_seen: u64,
    pub(crate) cells: CellRun,
}

/// How the sessions a game hosts and joins from its own UI start: [`Context::host`].
#[derive(Clone, Default)]
pub(crate) struct OwnSessions {
    /// The app, its build and its layers' names, bottom up, which a session's hello names.
    pub(crate) app: &'static str,
    pub(crate) build: Build,
    pub(crate) layers: Vec<&'static str>,
    /// The relay they meet through, as `--relay` given alone names it.
    pub(crate) relay: Option<String>,
    /// What `--link` has their links do to what comes.
    pub(crate) link: Option<Conditions>,
    /// Whether their peers predict each other's inputs ([`App::predicting`]).
    pub(crate) predicting: bool,
    /// Whether the run is recorded. A recording holds a run alone or a session from its start, so
    /// a session started partway can't be.
    pub(crate) recorded: bool,
}

/// What a layer asked of the run's session, which the stack takes up as the next frame starts.
pub(crate) enum Asked {
    /// A session [`Context::host`] or [`Context::join`] started, to take part in.
    Start(Box<Session>),
    /// The end of the run's session: [`Context::leave`].
    Leave,
}

/// What a development build's context keeps for its tools. A build without `dev` has a twin that
/// holds nothing, whose methods do nothing.
#[cfg(feature = "dev")]
pub(crate) struct Dev {
    /// When the frame being made started, by the machine's clock, as the frames of a view
    /// published to a tool say.
    pub(crate) started: std::time::SystemTime,
    /// The look the tool API's clients set, which frames draw with over what the layers set.
    pub(crate) look: crate::api::Look,
    /// Which debug overlays are on, which frames draw under the look.
    pub(crate) overlays: crate::debug_overlays::State,
    /// What the tool API handed the window since egui's last pass, for that pass.
    pub(crate) handed: crate::api::Handed,
    /// The simulation as the main loop left it this frame, for the Simulation overlay.
    pub(crate) sim: Sim,
    /// The session's numbers as the main loop read them this frame, for the Network overlay;
    /// `None` without one.
    pub(crate) net: Option<crate::net::NetStats>,
    /// The project's scenes folders, which the tool API's `scene` opens and saves scene files in.
    pub(crate) folders: Vec<PathBuf>,
    /// The project's directory, where its manifest is, and its fetch list, which an ID no asset
    /// has is looked for in, as [`asset_path`](Context::asset_path) says: a project's game's alone.
    project_dir: Option<PathBuf>,
    /// The names given the textures the layers add, by path, in place of the asset database's IDs,
    /// as [`name_as`](Context::name_as) says: `None` while the run uses the database's.
    given: Option<HashMap<Arc<str>, Arc<str>>>,
    /// Where the run keeps its assets' prints between runs, as [`assets::cache`] says, if it can:
    /// a run's, never a test's.
    asset_cache: Option<PathBuf>,
    /// The thread that keeps the asset database current as the root changes, in a run that serves
    /// the tool API, as [`start_assets`](Context::start_assets) starts it.
    asset_watch: Option<AssetWatch>,
    /// The thread that rereads the layers' assets as their files change, in a run that serves the
    /// tool API, as [`start_assets`](Context::start_assets) starts it.
    hot_reload: Option<HotReload>,
    /// The script files the layers loaded, which development builds watch.
    pub(crate) scripts: crate::luau::Files,
    /// The world as edited, while a copy plays in its place ([`Stack::play`]).
    pub(crate) edited: Option<Edited>,
    /// Whether a SyncTest runs each tick again (`--sync-test`), as the layers attach or pick up
    /// from a restore.
    sync_tested: bool,
    /// Another process's game's frames, as [`show_frame_times`](Context::show_frame_times) gave
    /// them, and the number of the last.
    pub(crate) game_perf: Option<(Perf, u64)>,
    /// Whether the app shows a game it embeds ([`App::embeds_game`](crate::App::embeds_game)), and
    /// the cap and the render scale the settings set on that game.
    pub(crate) embeds_game: bool,
    pub(crate) game_max_fps: Option<std::num::NonZeroU32>,
    pub(crate) game_render_scale: Option<f32>,
    /// Whether that cap follows the window's monitor's refresh rate, the settings' Match monitor,
    /// until another cap replaces it.
    pub(crate) game_matches_monitor: bool,
    /// The window's monitor's refresh rate, in whole frames a second, if the system says.
    pub(crate) refresh_rate: Option<std::num::NonZeroU32>,
}

#[cfg(feature = "dev")]
impl Dev {
    pub(crate) fn new() -> Self {
        Self {
            started: std::time::SystemTime::UNIX_EPOCH,
            look: crate::api::Look::default(),
            overlays: crate::debug_overlays::State::default(),
            handed: crate::api::Handed::default(),
            sim: Sim::default(),
            net: None,
            folders: Vec::new(),
            project_dir: None,
            given: None,
            asset_cache: None,
            asset_watch: None,
            hot_reload: None,
            scripts: crate::luau::Files::default(),
            edited: None,
            sync_tested: false,
            game_perf: None,
            embeds_game: false,
            game_max_fps: None,
            game_render_scale: None,
            game_matches_monitor: false,
            refresh_rate: None,
        }
    }

    /// Sets [`started`](Self::started) to now.
    pub(crate) fn start_frame(&mut self) {
        self.started = std::time::SystemTime::now();
    }
}

/// Where the simulation stands, copied from the stepper each frame.
#[cfg(feature = "dev")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sim {
    /// The number of the next tick: those run so far in this world.
    pub(crate) tick: u64,
    pub(crate) state: SimState,
}

/// Whether the simulation steps in real time, as the tool API's `pause` and `stop` set.
#[cfg(feature = "dev")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SimState {
    #[default]
    Running,
    Paused,
    Stopped,
}

/// A build without `dev`'s [`Dev`]: nothing.
#[cfg(not(feature = "dev"))]
pub(crate) struct Dev;

#[cfg(not(feature = "dev"))]
impl Dev {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn start_frame(&mut self) {}
}

/// The world as edited, which the tool API's `play` holds aside while a copy plays in its place
/// and `stop` brings back. It holds the world's ticks, its scene files as last applied, the texts
/// kept for scene changes (cells' files among them, so a file saved while the copy plays applies
/// after), its open scene and the authors' edits of it.
#[cfg(feature = "dev")]
pub(crate) struct Edited {
    pub(crate) world: World,
    pub(crate) ticks: u64,
    pub(crate) scenes: Vec<SceneFile>,
    pub(crate) cached: Vec<SceneFile>,
    pub(crate) open: scene_files::Open,
    pub(crate) authors: Authors,
    /// Whether the copy is of the world the layers' `attach` built, played as the run started:
    /// its first tick applies what they queued there, which a snapshot before it would miss.
    pub(crate) attached: bool,
    /// The edits authors made of the copy alone (the API's `play`), each by its author and its
    /// place in the author's log in the copy, counted from the first done: undoing or redoing one
    /// passes nothing through ([`Through`](through::Through)).
    pub(crate) played: Vec<(String, usize)>,
}

#[cfg(test)]
mod tests;
