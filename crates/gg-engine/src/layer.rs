//! The layer stack: how an app's code, and the engine's tools, plug into the main loop.

mod adds;
#[cfg(feature = "dev")]
mod catalog;
mod cell_stream;
mod frame;
mod named;
#[cfg(feature = "dev")]
mod reload;
mod run;
mod sound;
mod stack;
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
use crate::{Entity, EntityId, Event, Input, Ui, World};

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

    /// Called as a SyncTest (`--sync-test`) runs a tick again, as it does each once it has run: as
    /// a copy of the world taken as the tick started is put in the world's place, before the
    /// layers' ticks run again on it, each given back the commands it applied; and as the world
    /// comes back, once they have. The layer moves what it keeps of a world by its
    /// [serial](World::serial) to the world in place, as the Inspector moves its log, so that its
    /// tick goes on the copy as it went on the world, and on the world after as before; what it
    /// keeps by entities' IDs holds in both, which name the same entities there. It queues nothing,
    /// as the engine fills its queue. The default does nothing, for a layer that keeps nothing of
    /// a world by its serial.
    fn swapped(&mut self, _cx: &mut Context) {}

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
    /// out and tessellate an empty UI. A build without `dev`, which has no UI, never calls it: it
    /// compiles there as here, and draws nothing, as [`Ui`] has no value there to call it with.
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

    /// Whether the commands are edits of the world, as the Inspector's are, which a rollback session
    /// (`--rollback`) applies only at a tick it has both peers' inputs for, as it does the edits
    /// from outside the layers, so that each applies once: what applies them keeps a log of them,
    /// which a tick run again would add to again. The default, `false`, is a game's, whose ticks go
    /// by the world, the tick and their commands alone, and may run again.
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

/// What's still on its way to the GPU, as [`Context::streaming`] tells it: the textures whose
/// images are being decoded, off the main thread, and what the renderer is streaming, as the last
/// frame it rendered left it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Loading {
    /// The textures whose images are being decoded now, which show their stand-ins until they've
    /// decoded and streamed.
    pub decoding: u32,
    /// The meshes and textures the renderer was streaming to the GPU once it had rendered the last
    /// frame it handed back, and the bytes of them still to copy.
    pub streaming: Streaming,
}

impl Loading {
    /// Whether nothing is on its way: no image is decoding, and every texture and mesh added
    /// before the frame the renderer last handed back was drawable in that frame. What's added
    /// since counts once a frame has taken it, but for images, which count as they decode.
    pub fn is_empty(&self) -> bool {
        self.decoding == 0 && self.streaming.is_empty()
    }
}

/// Where a frame's viewpoint came from: a layer, which sets one as its camera moves; the tool
/// API's look, whichever viewpoint it set, as each is a jump; the world's primary camera, by its ID;
/// or none, the default. A frame whose viewpoint comes from elsewhere than the frame before's, or
/// whose projection is the other kind, is cut to, as [`gg_renderer::RenderData::cut`] says.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Seer {
    Layer,
    #[cfg(feature = "dev")]
    Look(Viewpoint),
    Camera(EntityId),
    Default,
}

impl fmt::Display for Seer {
    /// What it names a frame's viewpoint by, in a diagnostic.
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

/// Entities and every entity under each to outline in the view, in a color and a width, as the
/// look's outline says; a frame turns it into those entities' IDs, the renderer's outline. Kept
/// as the look sets it, so that frames don't build it again.
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
    /// Whether layers want the cursor grabbed, as [`set_cursor_grabbed`](Self::set_cursor_grabbed)
    /// says.
    cursor_grabbed: bool,
    /// The most frames a second a windowed run renders, as [`set_max_fps`](Self::set_max_fps)
    /// says.
    max_fps: Option<NonZeroU32>,
    clear_color: [f32; 4],
    /// How a layer set the view to be antialiased, in place of the world's environment's, as
    /// [`set_antialiasing`](Self::set_antialiasing) says.
    antialiasing: Option<Antialiasing>,
    /// What a layer set the view to see from this frame, in place of the world's camera.
    viewpoint: Option<Viewpoint>,
    /// What the frame's draws are relative to, as [`origin`](Self::origin) says.
    origin: DVec3,
    /// Where the world's camera saw from as the world was last drawn, if it has one, and its ID.
    seen: Option<(EntityId, Viewpoint)>,
    /// Where the view's viewpoint came from in the last frame handed off, and whether it had
    /// perspective, which tell whether the next is cut to from elsewhere.
    seer: Option<(Seer, bool)>,
    /// The tie among the world's cameras last warned of, while they've tied since.
    tie: Option<Tie>,
    /// What the view's projection was in the last frame of its source whose projection could
    /// project, and whether this streak of frames whose can't has warned: as [`camera::checked`]
    /// says.
    projection: camera::Projected<Seer>,
    /// Whether the fog's current streak of frames with numbers that aren't finite has warned.
    fog_warned: bool,
    /// The world's environment as the world was last drawn, if it has one.
    environment: Option<Environment>,
    /// The tie among the world's environments last warned of, while they've tied since.
    environment_tie: Option<environment::Tie>,
    /// The world's lights as the world was last drawn, which keep their memory.
    lit: light::Lit,
    /// The simulation's time in seconds as the world was last drawn.
    time: f32,
    meshes: Meshes,
    shaders: Shaders,
    textures: Textures,
    materials: Materials,
    /// What decodes the textures' images, off the main thread in a run's context.
    loader: Loader,
    /// What the renderer was streaming once it had rendered the last frame it handed back.
    pub(crate) streamed: Streaming,
    fonts: Fonts,
    /// The sounds and music the layers loaded, the buses' volumes, and the output that plays them,
    /// in a run that plays sound.
    audio: Audio,
    /// What to draw this frame.
    draws: Draws,
    /// What to draw this frame over the rest, after the world: lines.
    over: Draws,
    /// What the layers drew on the canvas this frame, which draws over all of it.
    canvas: canvas::Drawn,
    /// The UIs the layers added from their files, and what each holds, as [`GameUi`] says.
    uis: game_ui::Uis,
    /// Whether the world's colliders' outlines draw, over the rest.
    colliders: bool,
    /// Whether the world's navmesh and its agents' paths draw, over the rest.
    navmesh: bool,
    /// The color and width a layer outlines in the view this frame, if it does, of the entity in
    /// `outline_root`.
    outline: Option<(Vec4, f32)>,
    /// The entity a layer outlines: at most one, kept as a list for the walk, so that frames don't
    /// allocate.
    outline_root: Vec<EntityId>,
    /// Scratch space for walking the outlined entities' subtrees, kept so that frames don't
    /// allocate.
    outline_walk: Vec<Entity>,
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
    /// Where the world's entities are, kept from frame to frame, and where those on their way
    /// draw this frame.
    placed: Placed,
    /// The world's change tick as the latest tick started, at and after which the entities whose
    /// transforms changed draw on their way between ticks.
    tick_started: u64,
    /// What the renderer's scene holds of the world's opaque meshes, and what's changed of it
    /// since a frame last took it.
    opaque: OpaqueMeshes,
    /// The skinned entities' own meshes, which the scene draws them by, and their joints.
    skins: Skins,
    scene: Scene,
    /// Set from when the layers have attached until they detach: the world then changes only at
    /// ticks.
    attached: bool,
    /// What the files of the assets the layers add are named from, as [`App::root`] says.
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
    /// The project's scenes folders, which the tool API's `scene` opens and saves scene files in.
    #[cfg(feature = "dev")]
    pub(crate) folders: Vec<PathBuf>,
    /// The project's directory, where its manifest is, and its fetch list, which an ID no asset
    /// has is looked for in, as [`asset_path`](Self::asset_path) says: a project's game's alone.
    #[cfg(feature = "dev")]
    project_dir: Option<PathBuf>,
    /// The directory of what the app keeps for the user from run to run, beside the dock's layout,
    /// as [`read_state`](Self::read_state) says: a windowed run's alone.
    state: Option<PathBuf>,
    /// The assets the layers added: the world's, which a world that a layer puts in its place
    /// takes again, as [`share_assets`](Self::share_assets) says.
    assets: Arc<Assets>,
    /// The textures, models, fonts, shaders, sounds and music the layers added, by the file each
    /// resolves to, which a second add of that file, by whatever path, gives again.
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
    /// Where the run keeps the prints of what its assets hold between runs, as [`assets::cache`]
    /// says, if it can: a run's, never a test's.
    #[cfg(feature = "dev")]
    asset_cache: Option<PathBuf>,
    /// The thread that keeps the asset database as the root changes, in a run that serves the tool
    /// API, as [`start_assets`](Self::start_assets) starts it.
    #[cfg(feature = "dev")]
    asset_watch: Option<AssetWatch>,
    /// The thread that reads the layers' shaders, textures, models, fonts, prefabs, sounds and
    /// music again as their files change, in a run that serves the tool API, as
    /// [`start_assets`](Self::start_assets) starts it.
    #[cfg(feature = "dev")]
    hot_reload: Option<HotReload>,
    /// How long frames take, which the main loop measures and
    /// [`PerfPanel`](crate::PerfPanel) shows.
    pub(crate) perf: Perf,
    /// Set once a layer has shown something that paused the frames while it was open, such as a
    /// file dialog, until the main loop takes it to restart its clock, so that no frame's time
    /// counts the pause.
    pub(crate) paused: bool,
    /// Set once a layer has asked the run to end, as [`exit`](Self::exit) says.
    exiting: bool,
    /// Set once a layer has ended the run with an error, as [`fail`](Self::fail) says.
    failed: bool,
    /// When the engine started, until the run logs `engine.loaded`, once it has loaded, as
    /// [`take_loaded`](Self::take_loaded) says: `None` from then on, and in a test's or a replay's
    /// context, which logs none.
    pub(crate) load_started: Option<Instant>,
    /// The edits of the authors outside the layers, the tool API's clients and the scene files a
    /// game loaded, each undoing its own.
    pub(crate) authors: Authors,
    /// The scene files the layers loaded, which development builds watch.
    pub(crate) scenes: SceneFiles,
    /// The script files the layers loaded, which development builds watch.
    #[cfg(feature = "dev")]
    pub(crate) scripts: crate::luau::Files,
    /// What runs the layers' scripts, which a script reloads through as an edit asks, and a world
    /// brought back migrates through, as [`Scripts`](crate::Scripts) says.
    pub(crate) hosts: crate::luau::Hosts,
    /// The run's mods: where they come from, and those the layers loaded, as
    /// [`Mods`](crate::Mods) says.
    pub(crate) mods: crate::luau::RunMods,
    /// The world as edited, while a copy of it plays in its place, as [`Stack::play`] holds it.
    #[cfg(feature = "dev")]
    pub(crate) edited: Option<Edited>,
    /// The local player, in a lockstep session.
    player: Option<u8>,
    /// Whether the run takes part in a networked session, as the layers attach or pick up from a
    /// restore, or the session a layer asked for starts or ends.
    networked: bool,
    /// How the sessions the layers host and join start, as [`host`](Self::host) says.
    pub(crate) own: OwnSessions,
    /// A session [`host`](Self::host) or [`join`](Self::join) started, or the end
    /// [`leave`](Self::leave) asked for, which the stack takes up as the next frame starts.
    pub(crate) asked: Option<Asked>,
    /// The run's session as the frame's turn left it, as [`session`](Self::session) gives it.
    pub(crate) session: Option<SessionStatus>,
    /// What a layer asked put on the OS's clipboard since the window last took it, as
    /// [`copy`](Self::copy) says.
    pub(crate) copied: Option<String>,
    /// Whether the run is a server's or one of its clients, as the layers attach or pick up from a
    /// restore: their worlds name only the assets the layers loaded, so that a client reads its
    /// server's states, and a scene change loads none, as [`change_scene`](Self::change_scene)
    /// says.
    pub(crate) served: bool,
    /// Whether the run is a server's client, as the layers attach or pick up from a restore, whose
    /// world's cells come in with its server's states alone, as
    /// [`stream_cells`](Self::stream_cells) says.
    pub(crate) client: bool,
    /// How many scene changes the world had been through as a tick last took them as final, as
    /// [`confirm_scene`](Self::confirm_scene) says.
    scene_seen: u64,
    /// How many scene changes the world's ticks had asked for that were refused, as a tick last
    /// took them as final.
    refused_seen: u64,
    /// What the run keeps of its worlds' cells beside the world, as
    /// [`stream_cells`](Self::stream_cells) says, and what it prepares of them, as
    /// [`cells_ready`](Self::cells_ready) says.
    pub(crate) cells: CellRun,
    /// Whether a SyncTest runs each tick again (`--sync-test`), as the layers attach or pick up
    /// from a restore.
    #[cfg(feature = "dev")]
    sync_tested: bool,
}

/// How the sessions a game hosts and joins from its own UI start, as [`Context::host`] says.
#[derive(Clone, Default)]
pub(crate) struct OwnSessions {
    /// The app, its build and its layers' names, from the bottom up, which a session's hello names.
    pub(crate) app: &'static str,
    pub(crate) build: Build,
    pub(crate) layers: Vec<&'static str>,
    /// The relay they meet through, as `--relay` given alone names it.
    pub(crate) relay: Option<String>,
    /// What `--link` has their links do to what comes.
    pub(crate) link: Option<Conditions>,
    /// Whether their peers predict each other's inputs, as [`App::predicting`] says.
    pub(crate) predicting: bool,
    /// Whether the run is recorded, which a session started partway can't be, as a recording holds
    /// a run alone or a session from its start.
    pub(crate) recorded: bool,
}

/// What a layer asked of the run's session, which the stack takes up as the next frame starts.
pub(crate) enum Asked {
    /// A session [`Context::host`] or [`Context::join`] started, to take part in.
    Start(Box<Session>),
    /// The end of the run's session, as [`Context::leave`] asks.
    Leave,
}

/// The world as edited, which the tool API's `play` holds aside while a copy of it plays in its
/// place, and `stop` brings back: with the ticks it's at, its scene files as last applied and the
/// texts kept for scene changes, its cells' files among them, so that a file saved while the copy
/// plays applies to it after, its open scene and the authors' edits of it.
#[cfg(feature = "dev")]
pub(crate) struct Edited {
    pub(crate) world: World,
    pub(crate) ticks: u64,
    pub(crate) scenes: Vec<SceneFile>,
    pub(crate) cached: Vec<SceneFile>,
    pub(crate) open: scene_files::Open,
    pub(crate) authors: Authors,
}

#[cfg(test)]
mod tests;
